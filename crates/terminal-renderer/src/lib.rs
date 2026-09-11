//! GPU-backed terminal renderer with system-font discovery and fallback shaping.

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    mem,
    sync::Arc,
    time::Instant,
};

use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Cache, Color as GlyphColor, Family, FontSystem, Metrics, Resolution, Shaping,
    Style, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use glyphon::{cosmic_text::UnderlineStyle, fontdb};
use terminal_core::{Cell, CellWidth, Color, CursorShape, Rgb, Screen};
use thiserror::Error;
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

pub const CELL_WIDTH: f64 = 9.0;
pub const CELL_HEIGHT: f64 = 18.0;
pub const TAB_BAR_HEIGHT: f64 = 32.0;
pub const MIN_TAB_WIDTH: f64 = 120.0;
const DETACHED_TAB_PREFERRED_WIDTH: f32 = 240.0;
const DETACHED_TAB_MIN_WIDTH: f32 = 160.0;
const DETACHED_TAB_MAX_WIDTH: f32 = 320.0;
const TAB_DROP_DURATION: std::time::Duration = std::time::Duration::from_millis(140);
pub const NEW_TAB_WIDTH: f64 = 36.0;
pub const PANE_BORDER_WIDTH: f64 = 1.0;
const TAB_CLOSE_WIDTH: f32 = 24.0;
const TAB_CLOSE_RIGHT_INSET: f32 = 4.0;
const TAB_BUTTON_VERTICAL_INSET: f32 = 4.0;
const NEW_TAB_VERTICAL_INSET: f32 = 2.0;
const TAB_TITLE_LEFT_PADDING: f32 = 10.0;
const REQUESTED_FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT_SCALE: f32 = 1.08;
const PADDING: f64 = 8.0;
const DEFAULT_FOREGROUND: Rgb = Rgb::new(218, 218, 218);
const DEFAULT_BACKGROUND: Rgb = Rgb::new(18, 18, 18);
const SELECTION_BACKGROUND: Rgb = Rgb::new(55, 82, 110);
const SEARCH_BACKGROUND: Rgb = Rgb::new(130, 96, 20);

#[derive(Clone, Debug)]
pub struct RenderTheme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub ansi: [Rgb; 16],
    pub cursor: Rgb,
    pub selection_foreground: Rgb,
    pub selection_background: Rgb,
    pub tab_bar: Rgb,
    pub inactive_tab: Rgb,
    pub active_tab: Rgb,
    pub pane_border: Rgb,
}

impl Default for RenderTheme {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
            ansi: default_ansi(),
            cursor: Rgb::new(244, 244, 244),
            selection_foreground: Rgb::new(255, 255, 255),
            selection_background: SELECTION_BACKGROUND,
            tab_bar: Rgb::new(25, 25, 25),
            inactive_tab: Rgb::new(34, 34, 34),
            active_tab: Rgb::new(48, 48, 48),
            pane_border: Rgb::new(69, 69, 69),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub font_family: String,
    pub fallback_families: Vec<String>,
    pub font_size: f32,
    pub line_height: Option<f32>,
    pub padding: f64,
    pub opacity: f32,
    pub theme: RenderTheme,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            font_family: "Menlo".into(),
            fallback_families: Vec::new(),
            font_size: REQUESTED_FONT_SIZE,
            line_height: None,
            padding: PADDING,
            opacity: 1.0,
            theme: RenderTheme::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ViewportRect {
    pub fn contains(self, position: winit::dpi::PhysicalPosition<f64>) -> bool {
        let x = position.x as f32;
        let y = position.y as f32;
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LogicalRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl LogicalRect {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.x + self.width)
            && y < f64::from(self.y + self.height)
    }

    fn physical(self, scale: f32) -> ViewportRect {
        ViewportRect {
            x: self.x * scale,
            y: self.y * scale,
            width: self.width * scale,
            height: self.height * scale,
        }
    }
}

pub struct PaneView<'a> {
    pub id: u64,
    pub screen: &'a Screen,
    pub rect: ViewportRect,
    pub focused: bool,
}

#[derive(Clone, Debug, Hash)]
pub struct TabLabel {
    pub title: String,
    pub active: bool,
}

pub struct CommandPaletteRow<'a> {
    pub title: &'a str,
    pub category: &'a str,
    pub shortcut: String,
}
#[derive(Clone, Debug)]
pub struct PaletteLayout {
    pub outer: ViewportRect,
    pub input: ViewportRect,
    pub results: ViewportRect,
    pub footer: ViewportRect,
    pub rows: Vec<ViewportRect>,
}
pub struct CommandPaletteView<'a> {
    pub query: &'a str,
    pub rows: &'a [CommandPaletteRow<'a>],
    pub selected: usize,
    pub scroll_offset: usize,
    pub layout: &'a PaletteLayout,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TabDragVisual {
    source: usize,
    target: usize,
    x: f32,
    y: f32,
    outside_tab_bar: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HoverTarget {
    #[default]
    None,
    Tab(usize),
    TabClose(usize),
    NewTab,
    DraggedTab,
    PaneDivider {
        x: f32,
        y: f32,
        vertical: bool,
        active: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct TabDropAnimation {
    index: usize,
    from_x: f32,
    started_at: Instant,
}

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("could not create GPU surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("no suitable GPU adapter was found")]
    Adapter,
    #[error("could not create GPU device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("text preparation failed: {0}")]
    TextPrepare(#[from] glyphon::PrepareError),
    #[error("text render failed: {0}")]
    TextRender(#[from] glyphon::RenderError),
    #[error("the GPU surface reported a validation error")]
    SurfaceValidation,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    origin: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Debug)]
struct FontLayout {
    family: String,
    font_size: f32,
    cell_width: f64,
    cell_height: f64,
    baseline: f64,
    ascent: f64,
    descent: f64,
}

#[derive(Debug)]
struct TextSegment {
    column: usize,
    buffer: Buffer,
}

#[derive(Debug, Default)]
struct RowBuffers {
    segments: Vec<TextSegment>,
}

#[derive(Debug, Default)]
struct PaneBuffers {
    rows: Vec<RowBuffers>,
    row_signatures: Vec<u64>,
    prepared_generation: u64,
}

impl FontLayout {
    fn metrics(&self, scale_factor: f64) -> Metrics {
        Metrics::new(
            self.font_size * scale_factor as f32,
            self.cell_height as f32 * scale_factor as f32,
        )
    }
}

pub struct Renderer {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    rect_pipeline: wgpu::RenderPipeline,
    rect_buffer: wgpu::Buffer,
    rect_capacity: usize,
    rect_scratch: Vec<RectInstance>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    pane_buffers: HashMap<u64, PaneBuffers>,
    ui_buffers: Vec<Buffer>,
    palette_buffers: Vec<Buffer>,
    ui_signature: u64,
    palette_signature: u64,
    layout_signature: u64,
    layout_dirty: bool,
    tab_scroll: f64,
    tab_drag: Option<TabDragVisual>,
    tab_drop: Option<TabDropAnimation>,
    hover: HoverTarget,
    tab_drag_dirty: bool,
    interaction_dirty: bool,
    profile_render: bool,
    font_layout: FontLayout,
    options: RenderOptions,
    size: PhysicalSize<u32>,
    scale_factor: f64,
    window: Arc<Window>,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, RendererError> {
        Self::new_with_options(window, RenderOptions::default()).await
    }

    pub async fn new_with_options(
        window: Arc<Window>,
        options: RenderOptions,
    ) -> Result<Self, RendererError> {
        let size = window.inner_size();
        let scale_factor = window.scale_factor();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|_| RendererError::Adapter)?;
        let mut required_limits = wgpu::Limits::downlevel_defaults();
        required_limits.max_texture_dimension_2d = adapter.limits().max_texture_dimension_2d;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("terminal GPU device"),
                required_features: wgpu::Features::empty(),
                // A Retina window easily exceeds the conservative 2048px
                // downlevel default. Negotiate the adapter's 2D limit so
                // ordinary full-screen windows remain valid surfaces.
                required_limits,
                ..Default::default()
            })
            .await?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let present_mode = capabilities
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::AutoVsync)
            .unwrap_or(wgpu::PresentMode::Fifo);
        let alpha_mode = if options.opacity < 1.0 {
            capabilities
                .alpha_modes
                .iter()
                .copied()
                .find(|mode| {
                    matches!(
                        mode,
                        wgpu::CompositeAlphaMode::PreMultiplied
                            | wgpu::CompositeAlphaMode::PostMultiplied
                    )
                })
                .unwrap_or(capabilities.alpha_modes[0])
        } else {
            capabilities.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal rectangle shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminal rectangle pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminal rectangle pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: mem::size_of::<RectInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let rect_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terminal rectangles"),
            contents: bytemuck::bytes_of(&RectInstance::zeroed()),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let mut font_system = FontSystem::new();
        let font_layout = configure_font(&mut font_system, &options);
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        debug_metrics(size, scale_factor, &font_layout);

        Ok(Self {
            instance,
            surface,
            device,
            queue,
            config,
            rect_pipeline,
            rect_buffer,
            rect_capacity: 1,
            rect_scratch: Vec::new(),
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            pane_buffers: HashMap::new(),
            ui_buffers: Vec::new(),
            palette_buffers: Vec::new(),
            ui_signature: 0,
            palette_signature: 0,
            layout_signature: 0,
            layout_dirty: true,
            tab_scroll: 0.0,
            tab_drag: None,
            tab_drop: None,
            hover: HoverTarget::None,
            tab_drag_dirty: false,
            interaction_dirty: false,
            profile_render: std::env::var_os("GRIN_PROFILE_RENDER").is_some(),
            font_layout,
            options,
            size,
            scale_factor,
            window,
        })
    }

    pub fn grid_size(&self) -> (u16, u16) {
        self.grid_size_for(self.content_rect())
    }

    pub fn cell_at(&self, position: winit::dpi::PhysicalPosition<f64>) -> (usize, usize) {
        self.cell_at_in(self.content_rect(), position)
    }

    pub fn content_rect(&self) -> ViewportRect {
        let tab_height = TAB_BAR_HEIGHT * self.scale_factor;
        ViewportRect {
            x: 0.0,
            y: tab_height as f32,
            width: self.size.width as f32,
            height: (self.size.height as f64 - tab_height).max(1.0) as f32,
        }
    }

    pub fn grid_size_for(&self, rect: ViewportRect) -> (u16, u16) {
        grid_size_for_layout(
            rect,
            self.scale_factor,
            self.options.padding,
            &self.font_layout,
        )
    }

    pub fn cell_at_in(
        &self,
        rect: ViewportRect,
        position: winit::dpi::PhysicalPosition<f64>,
    ) -> (usize, usize) {
        let padding = self.options.padding * self.scale_factor;
        let column = ((position.x - f64::from(rect.x) - padding).max(0.0)
            / (self.font_layout.cell_width * self.scale_factor))
            .floor() as usize;
        let row = ((position.y - f64::from(rect.y) - padding).max(0.0)
            / (self.font_layout.cell_height * self.scale_factor))
            .floor() as usize;
        (row, column)
    }

    pub fn tab_bar_at(&self, position: winit::dpi::PhysicalPosition<f64>) -> bool {
        position.y >= 0.0 && position.y < TAB_BAR_HEIGHT * self.scale_factor
    }

    pub fn tab_rect(&self, index: usize, count: usize) -> Option<ViewportRect> {
        self.tab_logical_rect(index, count)
            .map(|rect| rect.physical(self.scale_factor as f32))
    }

    pub fn tab_at(
        &self,
        position: winit::dpi::PhysicalPosition<f64>,
        count: usize,
    ) -> Option<usize> {
        if count == 0 || !self.tab_bar_at(position) {
            return None;
        }

        let scale = self.scale_factor.max(1.0);
        let x = position.x / scale;
        let strip_width = self.tab_strip_width();

        // Правая область зарезервирована под "+".
        if x < 0.0 || x >= strip_width {
            return None;
        }

        let tab_width = self.tab_width(count);
        let scroll = self.effective_tab_scroll(count);

        let index = ((x + scroll) / tab_width).floor() as usize;

        (index < count).then_some(index)
    }

    pub fn new_tab_at(&self, position: winit::dpi::PhysicalPosition<f64>, _count: usize) -> bool {
        if !self.tab_bar_at(position) {
            return false;
        }

        let scale = self.scale_factor.max(1.0);
        let x = position.x / scale;
        let y = position.y / scale;
        self.new_tab_logical_rect().contains(x, y)
    }

    pub fn tab_close_at(
        &self,
        position: winit::dpi::PhysicalPosition<f64>,
        index: usize,
        count: usize,
    ) -> bool {
        if index >= count || !self.tab_bar_at(position) {
            return false;
        }

        let scale = self.scale_factor.max(1.0);
        let x = position.x / scale;
        let y = position.y / scale;
        self.tab_close_logical_rect(index, count)
            .is_some_and(|rect| rect.contains(x, y))
    }

    pub fn scroll_tabs(&mut self, delta: f64, count: usize) -> bool {
        let maximum = self.max_tab_scroll(count);
        let current = self.effective_tab_scroll(count);

        let next = (current + delta).clamp(0.0, maximum);

        if (next - self.tab_scroll).abs() <= f64::EPSILON {
            return false;
        }

        self.tab_scroll = next;
        self.layout_dirty = true;

        true
    }

    pub fn ensure_tab_visible(&mut self, index: usize, count: usize) -> bool {
        if index >= count {
            return false;
        }

        let strip_width = self.tab_strip_width();
        let tab_width = self.tab_width(count);
        let maximum = self.max_tab_scroll(count);

        let tab_left = index as f64 * tab_width;
        let tab_right = tab_left + tab_width;

        let mut next = self.effective_tab_scroll(count);

        if tab_left < next {
            next = tab_left;
        } else if tab_right > next + strip_width {
            next = tab_right - strip_width;
        }

        next = next.clamp(0.0, maximum);

        if (next - self.tab_scroll).abs() <= f64::EPSILON {
            return false;
        }

        self.tab_scroll = next;
        self.layout_dirty = true;

        true
    }

    pub fn set_tab_drag_visual(
        &mut self,
        source: usize,
        target: usize,
        position: winit::dpi::PhysicalPosition<f64>,
        grab_offset_x: f64,
    ) {
        self.tab_drop = None;
        let scale = self.scale_factor.max(1.0) as f32;
        let bar_height = TAB_BAR_HEIGHT as f32 * scale;

        let x = position.x as f32 - grab_offset_x as f32;

        // Пока мышка внутри tab bar, tab слегка "поднят".
        // Если мышка уходит вниз — ghost следует за ней.
        let outside_tab_bar = !(position.y >= 0.0 && position.y < f64::from(bar_height));
        let y = if outside_tab_bar {
            position.y as f32 - bar_height / 2.0
        } else {
            2.0 * scale
        };

        let next = Some(TabDragVisual {
            source,
            target,
            x,
            y,
            outside_tab_bar,
        });

        if self.tab_drag != next {
            self.tab_drag = next;
            self.tab_drag_dirty = true;
        }
    }

    pub fn clear_tab_drag_visual(&mut self) {
        if self.tab_drag.take().is_some() {
            self.tab_drag_dirty = true;
        }
    }

    pub fn complete_tab_drag(&mut self, index: usize) {
        if let Some(drag) = self.tab_drag.take() {
            self.tab_drop = Some(TabDropAnimation {
                index,
                from_x: drag.x,
                started_at: Instant::now(),
            });
            self.tab_drag_dirty = true;
        }
    }

    pub fn set_hover(&mut self, hover: HoverTarget) -> bool {
        if self.hover == hover {
            return false;
        }
        self.hover = hover;
        self.interaction_dirty = true;
        true
    }

    fn logical_width(&self) -> f64 {
        self.size.width as f64 / self.scale_factor.max(1.0)
    }

    fn tab_logical_rect(&self, index: usize, count: usize) -> Option<LogicalRect> {
        if index >= count {
            return None;
        }
        let tab_width = self.tab_width(count) as f32;
        let scroll = self.effective_tab_scroll(count) as f32;
        Some(LogicalRect {
            x: index as f32 * tab_width - scroll,
            y: 0.0,
            width: tab_width,
            height: TAB_BAR_HEIGHT as f32,
        })
    }

    fn tab_close_logical_rect(&self, index: usize, count: usize) -> Option<LogicalRect> {
        self.tab_logical_rect(index, count).map(tab_close_rect)
    }

    fn new_tab_logical_rect(&self) -> LogicalRect {
        new_tab_rect(self.tab_strip_width() as f32)
    }

    fn button_glyph_position(&self, rect: ViewportRect) -> (f32, f32) {
        let scale = self.scale_factor as f32;
        let glyph_width = self.font_layout.cell_width as f32 * scale;
        let glyph_height = self.font_layout.cell_height as f32 * scale;
        (
            rect.x + (rect.width - glyph_width) / 2.0,
            rect.y + (rect.height - glyph_height) / 2.0,
        )
    }

    fn tab_strip_width(&self) -> f64 {
        (self.logical_width() - NEW_TAB_WIDTH).max(1.0)
    }

    fn tab_width(&self, count: usize) -> f64 {
        let strip_width = self.tab_strip_width();

        if count == 0 {
            return strip_width;
        }

        (strip_width / count as f64).max(MIN_TAB_WIDTH)
    }

    fn max_tab_scroll(&self, count: usize) -> f64 {
        if count == 0 {
            return 0.0;
        }

        let total_width = self.tab_width(count) * count as f64;

        (total_width - self.tab_strip_width()).max(0.0)
    }

    fn effective_tab_scroll(&self, count: usize) -> f64 {
        self.tab_scroll.clamp(0.0, self.max_tab_scroll(count))
    }

    pub fn set_theme(&mut self, theme: RenderTheme) {
        self.options.theme = theme;
        for pane in self.pane_buffers.values_mut() {
            pane.row_signatures.fill(0);
        }
        self.layout_dirty = true;
        self.ui_signature = 0;
    }

    pub fn remove_pane(&mut self, id: u64) {
        self.pane_buffers.remove(&id);
        self.layout_dirty = true;
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        if size == self.size && scale_factor == self.scale_factor {
            return;
        }
        let scale_changed = scale_factor != self.scale_factor;
        self.size = size;
        self.scale_factor = scale_factor;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        if scale_changed {
            let metrics = self.font_layout.metrics(scale_factor);
            for pane in self.pane_buffers.values_mut() {
                for row in &mut pane.rows {
                    for segment in &mut row.segments {
                        segment.buffer.set_metrics(metrics);
                    }
                }
                pane.row_signatures.fill(0);
            }
            for buffer in &mut self.ui_buffers {
                buffer.set_metrics(metrics);
            }
            for buffer in &mut self.palette_buffers {
                buffer.set_metrics(metrics);
            }
        }
        self.layout_dirty = true;
        debug_metrics(size, scale_factor, &self.font_layout);
    }

    pub fn render(&mut self, screen: &Screen) -> Result<(), RendererError> {
        self.render_panes(
            &[PaneView {
                id: 0,
                screen,
                rect: self.content_rect(),
                focused: true,
            }],
            &[],
            None,
        )
    }

    pub fn command_palette_layout(&self, row_count: usize) -> PaletteLayout {
        let scale = self.scale_factor as f32;
        let logical_width = self.logical_width() as f32;
        let width = logical_width
            .clamp(320.0, 640.0)
            .min((logical_width - 24.0).max(1.0));
        let outer = LogicalRect {
            x: ((logical_width - width) / 2.0).max(0.0),
            y: TAB_BAR_HEIGHT as f32 + 20.0,
            width,
            height: (96.0 + row_count.min(9) as f32 * 28.0).max(130.0),
        }
        .physical(scale);
        let input = ViewportRect {
            x: outer.x + 10.0 * scale,
            y: outer.y + 8.0 * scale,
            width: (outer.width - 20.0 * scale).max(1.0),
            height: 32.0 * scale,
        };
        let footer = ViewportRect {
            x: input.x,
            y: outer.y + outer.height - 44.0 * scale,
            width: input.width,
            height: 36.0 * scale,
        };
        let results = ViewportRect {
            x: input.x,
            y: input.y + input.height + 4.0 * scale,
            width: input.width,
            height: (footer.y - (input.y + input.height + 4.0 * scale)).max(1.0),
        };
        let rows = (0..row_count.min(9))
            .map(|index| ViewportRect {
                x: results.x,
                y: results.y + index as f32 * 28.0 * scale,
                width: results.width,
                height: 28.0 * scale,
            })
            .collect();
        PaletteLayout {
            outer,
            input,
            results,
            footer,
            rows,
        }
    }

    pub fn render_panes(
        &mut self,
        panes: &[PaneView<'_>],
        tabs: &[TabLabel],
        palette: Option<&CommandPaletteView<'_>>,
    ) -> Result<(), RendererError> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }
        self.prepare_content(panes, tabs, palette)?;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(Arc::clone(&self.window))?;
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RendererError::SurfaceValidation);
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terminal frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu_color(
                            self.options.theme.tab_bar,
                            self.options.opacity,
                        )),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.rect_pipeline);
            pass.set_vertex_buffer(0, self.rect_buffer.slice(..));
            pass.draw(0..6, 0..self.rect_scratch.len() as u32);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.atlas.trim();
        if self.tab_drop.is_some() {
            self.window.request_redraw();
        }
        Ok(())
    }

    fn prepare_content(
        &mut self,
        panes: &[PaneView<'_>],
        tabs: &[TabLabel],
        palette: Option<&CommandPaletteView<'_>>,
    ) -> Result<(), RendererError> {
        if self
            .tab_drop
            .is_some_and(|drop| drop.started_at.elapsed() >= TAB_DROP_DURATION)
        {
            self.tab_drop = None;
            self.tab_drag_dirty = true;
        }
        let animating_drop = self.tab_drop.is_some();
        let layout_signature = pane_layout_signature(panes);
        let ui_signature = tab_signature(tabs);
        let palette_signature = command_palette_signature(palette);
        let content_changed = panes.iter().any(|pane| {
            self.pane_buffers
                .get(&pane.id)
                .is_none_or(|buffers| buffers.prepared_generation != pane.screen.generation())
        });
        if !self.layout_dirty
            && !self.tab_drag_dirty
            && !self.interaction_dirty
            && !animating_drop
            && !content_changed
            && self.layout_signature == layout_signature
            && self.ui_signature == ui_signature
            && self.palette_signature == palette_signature
        {
            return Ok(());
        }

        let profile_started_at = Instant::now();
        let mut dirty_rows = 0;
        self.rect_scratch.clear();
        // Сначала terminal panes.
        for pane in panes {
            self.build_pane_rectangles(pane);
        }
        // Потом UI chrome.
        // Это важно: floating tab должен рисоваться ПОВЕРХ terminal background.
        self.build_chrome_rectangles(tabs, palette);
        self.ensure_rect_capacity(self.rect_scratch.len());
        if !self.rect_scratch.is_empty() {
            self.queue.write_buffer(
                &self.rect_buffer,
                0,
                bytemuck::cast_slice(&self.rect_scratch),
            );
        }

        let metrics = self.font_layout.metrics(self.scale_factor);
        let theme = &self.options.theme;
        for pane in panes {
            let buffers = self.pane_buffers.entry(pane.id).or_default();
            sync_row_buffers(buffers, pane.screen.rows());
            for row in 0..pane.screen.rows() {
                let signature = row_signature(pane.screen, row);
                if !self.layout_dirty && buffers.row_signatures[row] == signature {
                    continue;
                }
                dirty_rows += 1;
                let cursor = pane.screen.display_cursor();
                let last_visible = pane
                    .screen
                    .display_line(row)
                    .and_then(|line| line.iter().rposition(cell_needs_glyph_span));
                let row_segments = &mut buffers.rows[row].segments;
                let mut segment_index = 0;
                if let (Some(line), Some(last_visible)) =
                    (pane.screen.display_line(row), last_visible)
                {
                    let mut column = 0;
                    while column <= last_visible {
                        if line[column].width == CellWidth::Continuation {
                            column += 1;
                            continue;
                        }
                        let start = column;
                        let end = if line[column].text.is_ascii() {
                            column += 1;
                            while column <= last_visible
                                && line[column].width != CellWidth::Continuation
                                && line[column].text.is_ascii()
                            {
                                column += 1;
                            }
                            column
                        } else {
                            column += 1;
                            column
                        };
                        if segment_index == row_segments.len() {
                            let mut buffer = Buffer::new(&mut self.font_system, metrics);
                            buffer.set_wrap(Wrap::None);
                            row_segments.push(TextSegment {
                                column: start,
                                buffer,
                            });
                        }
                        let segment = &mut row_segments[segment_index];
                        segment.column = start;
                        let default_attrs = Attrs::new()
                            .family(Family::Monospace)
                            .color(glyph_color(theme.foreground));
                        let spans = line[start..end]
                            .iter()
                            .enumerate()
                            .filter(|(_, cell)| cell.width != CellWidth::Continuation)
                            .map(|(offset, cell)| {
                                let column = start + offset;
                                (
                                    cell.text.as_str(),
                                    glyph_attrs(
                                        cell,
                                        pane.screen.selected_at(row, column),
                                        pane.screen.search_match_at(row, column),
                                        cursor.is_some_and(|cursor| {
                                            pane.focused
                                                && cursor.visible
                                                && cursor.shape == CursorShape::Block
                                                && cursor.row == row
                                                && cursor.column == column
                                        }),
                                        theme,
                                    ),
                                )
                            });
                        segment.buffer.set_rich_text(
                            spans,
                            &default_attrs,
                            Shaping::Advanced,
                            None,
                        );
                        segment
                            .buffer
                            .shape_until_scroll(&mut self.font_system, false);
                        segment_index += 1;
                    }
                }
                row_segments.truncate(segment_index);
                buffers.row_signatures[row] = signature;
            }
            buffers.prepared_generation = pane.screen.generation();
        }

        let tab_width = self.tab_width(tabs.len()) as f32;
        if self.layout_dirty || self.interaction_dirty || self.ui_signature != ui_signature {
            self.prepare_ui_buffers(tabs, metrics, tab_width);
        }
        self.prepare_palette_buffers(palette, metrics);
        self.layout_signature = layout_signature;
        self.ui_signature = ui_signature;
        self.palette_signature = palette_signature;
        self.layout_dirty = false;
        self.tab_drag_dirty = false;
        self.interaction_dirty = false;
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        let scale = self.scale_factor as f32;
        let padding = self.options.padding as f32 * scale;
        let row_height = self.font_layout.cell_height as f32 * scale;
        let cell_width = self.font_layout.cell_width as f32 * scale;

        let foreground = self.options.theme.foreground;

        let tab_strip_width = self.tab_strip_width() as f32;

        let tab_scroll = self.effective_tab_scroll(tabs.len()) as f32;

        let tab_drag_width = self.tab_drag_width(tab_width);
        let tab_drag = self.tab_drag;
        let tab_drop = self.tab_drop_position(tab_width, tab_scroll);

        let mut ui_areas = Vec::with_capacity(self.ui_buffers.len());
        for (buffer_index, buffer) in self.ui_buffers.iter().enumerate() {
            if buffer_index < tabs.len() * 2 {
                let index = buffer_index / 2;
                let is_close = buffer_index % 2 == 1;
                if let Some(drag) = tab_drag.filter(|drag| drag.source == index) {
                    let tab = LogicalRect {
                        x: drag.x / scale,
                        y: drag.y / scale,
                        width: tab_drag_width,
                        height: TAB_BAR_HEIGHT as f32,
                    };
                    let rect = if is_close {
                        tab_close_rect(tab)
                    } else {
                        tab_title_rect(tab)
                    }
                    .physical(scale);
                    let (left, top) = if is_close {
                        self.button_glyph_position(rect)
                    } else {
                        (rect.x, rect.y + (rect.height - row_height) / 2.0)
                    };
                    ui_areas.push(TextArea {
                        buffer,
                        left,
                        top,
                        scale: 1.02,
                        bounds: text_bounds(rect),
                        default_color: glyph_color(foreground),
                        custom_glyphs: &[],
                    });
                    continue;
                }

                let display_index = tab_preview_index(index, tab_drag);
                let tab_left = tab_drop
                    .filter(|(drop_index, _)| *drop_index == index)
                    .map_or(display_index as f32 * tab_width - tab_scroll, |(_, x)| x);
                let tab = LogicalRect {
                    x: tab_left,
                    y: 0.0,
                    width: tab_width,
                    height: TAB_BAR_HEIGHT as f32,
                };
                let rect = if is_close {
                    tab_close_rect(tab)
                } else {
                    tab_title_rect(tab)
                }
                .physical(scale);
                if rect.x + rect.width <= 0.0 || rect.x >= tab_strip_width * scale {
                    continue;
                }
                let (left, top) = if is_close {
                    self.button_glyph_position(rect)
                } else {
                    (rect.x, rect.y + (rect.height - row_height) / 2.0)
                };
                ui_areas.push(TextArea {
                    buffer,
                    left,
                    top,
                    scale: 1.0,
                    bounds: text_bounds(rect),
                    default_color: glyph_color(foreground),
                    custom_glyphs: &[],
                });
            } else {
                // "+" всегда фиксирован справа и не scroll'ится.
                let rect = self.new_tab_logical_rect().physical(scale);
                let (left, top) = self.button_glyph_position(rect);
                ui_areas.push(TextArea {
                    buffer,
                    left,
                    top,
                    scale: 1.0,
                    bounds: text_bounds(rect),
                    default_color: glyph_color(foreground),
                    custom_glyphs: &[],
                });
            }
        }
        if let Some(palette) = palette {
            for (index, buffer) in self.palette_buffers.iter().enumerate() {
                let text_rect = if index == 0 {
                    palette.layout.input
                } else if index <= palette.layout.rows.len() {
                    palette.layout.rows[index - 1]
                } else {
                    palette.layout.footer
                };
                ui_areas.push(TextArea {
                    buffer,
                    left: text_rect.x + 8.0 * scale,
                    top: text_rect.y
                        + if index + 1 == self.palette_buffers.len() {
                            9.0 * scale
                        } else {
                            5.0 * scale
                        },
                    scale: 1.0,
                    bounds: text_bounds(text_rect),
                    default_color: glyph_color(foreground),
                    custom_glyphs: &[],
                });
            }
        }
        let palette_rect = palette.map(|palette| palette.layout.outer);
        let pane_buffers = &self.pane_buffers;
        let pane_areas = panes.iter().flat_map(move |pane| {
            pane_buffers
                .get(&pane.id)
                .into_iter()
                .flat_map(move |buffers| {
                    buffers
                        .rows
                        .iter()
                        .enumerate()
                        .flat_map(move |(row, row_buffers)| {
                            row_buffers.segments.iter().filter_map(move |segment| {
                                let top = pane.rect.y + padding + row as f32 * row_height;
                                if palette_rect.is_some_and(|rect| {
                                    top < rect.y + rect.height && top + row_height > rect.y
                                }) {
                                    return None;
                                }
                                Some(TextArea {
                                    buffer: &segment.buffer,
                                    left: pane.rect.x
                                        + padding
                                        + segment.column as f32 * cell_width,
                                    top,
                                    scale: 1.0,
                                    bounds: TextBounds {
                                        left: pane.rect.x.ceil() as i32,
                                        top: pane.rect.y.ceil() as i32,
                                        right: (pane.rect.x + pane.rect.width).floor() as i32,
                                        bottom: (pane.rect.y + pane.rect.height).floor() as i32,
                                    },
                                    default_color: glyph_color(foreground),
                                    custom_glyphs: &[],
                                })
                            })
                        })
                })
        });
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            pane_areas.chain(ui_areas),
            &mut self.swash_cache,
        )?;

        if self.profile_render {
            eprintln!(
                "render-prepare: {:.2} ms, panes={}, dirty_rows={dirty_rows}, glyph_segments={}, rectangles={}",
                profile_started_at.elapsed().as_secs_f64() * 1_000.0,
                panes.len(),
                self.pane_buffers
                    .values()
                    .flat_map(|pane| &pane.rows)
                    .map(|row| row.segments.len())
                    .sum::<usize>(),
                self.rect_scratch.len()
            );
        }
        Ok(())
    }

    fn prepare_ui_buffers(&mut self, tabs: &[TabLabel], metrics: Metrics, tab_width: f32) {
        let buffer_count = tabs.len() * 2 + 1;
        self.ui_buffers.truncate(buffer_count);
        while self.ui_buffers.len() < buffer_count {
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            buffer.set_wrap(Wrap::None);
            self.ui_buffers.push(buffer);
        }
        for (index, tab) in tabs.iter().enumerate() {
            let title_width_pixels =
                tab_width - TAB_TITLE_LEFT_PADDING - TAB_CLOSE_RIGHT_INSET - TAB_CLOSE_WIDTH;
            let available = (title_width_pixels.max(0.0) / self.font_layout.cell_width as f32)
                .floor()
                .max(1.0) as usize;
            let title_width = available.saturating_sub(1);
            let title = truncate_title(&tab.title, title_width);
            let title_color = if tab.active {
                self.options.theme.foreground
            } else {
                muted(self.options.theme.foreground, self.options.theme.tab_bar)
            };
            let close_color = if self.hover == HoverTarget::TabClose(index) {
                self.options.theme.foreground
            } else {
                mix_color(title_color, self.options.theme.tab_bar, 0.12)
            };
            let title = format!("{title:<title_width$} ");
            let default_attrs = Attrs::new().family(Family::Monospace);
            let title_buffer = index * 2;
            self.ui_buffers[title_buffer].set_text(
                &title,
                &Attrs::new()
                    .family(Family::Monospace)
                    .color(glyph_color(title_color)),
                Shaping::Advanced,
                None,
            );
            self.ui_buffers[title_buffer].shape_until_scroll(&mut self.font_system, false);
            self.ui_buffers[title_buffer + 1].set_rich_text(
                [(
                    "×",
                    Attrs::new()
                        .family(Family::Monospace)
                        .color(glyph_color(close_color)),
                )],
                &default_attrs,
                Shaping::Advanced,
                None,
            );
            self.ui_buffers[title_buffer + 1].shape_until_scroll(&mut self.font_system, false);
        }
        let plus = self.ui_buffers.last_mut().expect("new-tab buffer exists");
        plus.set_text(
            "+",
            &Attrs::new()
                .family(Family::Monospace)
                .color(glyph_color(self.options.theme.foreground)),
            Shaping::Advanced,
            None,
        );
        plus.shape_until_scroll(&mut self.font_system, false);
    }

    fn prepare_palette_buffers(
        &mut self,
        palette: Option<&CommandPaletteView<'_>>,
        metrics: Metrics,
    ) {
        let Some(palette) = palette else {
            self.palette_buffers.clear();
            return;
        };
        let count = palette.rows.len().min(9).max(1) + 2;
        self.palette_buffers.truncate(count);
        while self.palette_buffers.len() < count {
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            buffer.set_wrap(Wrap::None);
            self.palette_buffers.push(buffer);
        }
        let attrs = Attrs::new()
            .family(Family::Monospace)
            .color(glyph_color(self.options.theme.foreground));
        self.palette_buffers[0].set_text(
            &format!("> {}", palette.query),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.palette_buffers[0].shape_until_scroll(&mut self.font_system, false);
        if palette.rows.is_empty() {
            self.palette_buffers[1].set_text(
                "No matching commands",
                &attrs,
                Shaping::Advanced,
                None,
            );
            self.palette_buffers[1].shape_until_scroll(&mut self.font_system, false);
        }
        for (index, row) in palette.rows.iter().take(9).enumerate() {
            let text = if row.shortcut.is_empty() {
                format!("{:<28}{:<14}", row.title, row.category)
            } else {
                format!("{:<28}{:<14}{}", row.title, row.category, row.shortcut)
            };
            self.palette_buffers[index + 1].set_text(&text, &attrs, Shaping::Advanced, None);
            self.palette_buffers[index + 1].shape_until_scroll(&mut self.font_system, false);
        }
        let footer = self
            .palette_buffers
            .last_mut()
            .expect("palette footer exists");
        footer.set_text(
            "↑↓ Navigate    Enter Run    Esc Close",
            &attrs,
            Shaping::Advanced,
            None,
        );
        footer.shape_until_scroll(&mut self.font_system, false);
    }

    fn build_chrome_rectangles(
        &mut self,
        tabs: &[TabLabel],
        palette: Option<&CommandPaletteView<'_>>,
    ) {
        let scale = self.scale_factor as f32;

        let tab_width = self.tab_width(tabs.len()) as f32;

        let tab_scroll = self.effective_tab_scroll(tabs.len()) as f32;

        let strip_width = self.tab_strip_width() as f32;
        let tab_drag = self.tab_drag;
        let tab_drop = self.tab_drop_position(tab_width, tab_scroll);

        for (index, tab) in tabs.iter().enumerate() {
            let dragging_source = tab_drag.is_some_and(|drag| drag.source == index);
            if dragging_source && tab_drag.is_some_and(|drag| !drag.outside_tab_bar) {
                continue;
            }

            let display_index = tab_preview_index(index, tab_drag);
            let left = tab_drop
                .filter(|(drop_index, _)| *drop_index == index)
                .map_or(display_index as f32 * tab_width - tab_scroll, |(_, x)| x);

            let right = left + tab_width;

            let visible_left = left.max(0.0);
            let visible_right = right.min(strip_width);

            if visible_right <= visible_left {
                continue;
            }

            push_rect(
                &mut self.rect_scratch,
                ViewportRect {
                    x: visible_left * scale,
                    y: 0.0,
                    width: ((visible_right - visible_left) * scale - scale).max(0.0),
                    height: TAB_BAR_HEIGHT as f32 * scale,
                },
                if dragging_source {
                    // На старом месте остаётся placeholder.
                    self.options.theme.tab_bar
                } else if self.hover == HoverTarget::TabClose(index) {
                    mix_color(
                        if tab.active {
                            self.options.theme.active_tab
                        } else {
                            self.options.theme.inactive_tab
                        },
                        self.options.theme.foreground,
                        0.16,
                    )
                } else if self.hover == HoverTarget::Tab(index) {
                    mix_color(
                        if tab.active {
                            self.options.theme.active_tab
                        } else {
                            self.options.theme.inactive_tab
                        },
                        self.options.theme.foreground,
                        0.08,
                    )
                } else if tab.active {
                    self.options.theme.active_tab
                } else {
                    self.options.theme.inactive_tab
                },
                if dragging_source {
                    self.options.opacity * 0.65
                } else {
                    self.options.opacity
                },
                self.size,
            );

            if self.hover == HoverTarget::TabClose(index) {
                let tab_surface = if tab.active {
                    self.options.theme.active_tab
                } else {
                    self.options.theme.inactive_tab
                };
                if let Some(rect) = self.tab_close_logical_rect(index, tabs.len()) {
                    push_rect(
                        &mut self.rect_scratch,
                        rect.physical(scale),
                        mix_color(tab_surface, self.options.theme.foreground, 0.22),
                        self.options.opacity,
                        self.size,
                    );
                }
            }
        }

        if self.hover == HoverTarget::NewTab {
            let rect = self.new_tab_logical_rect().physical(scale);
            push_rect(
                &mut self.rect_scratch,
                rect,
                mix_color(
                    self.options.theme.tab_bar,
                    self.options.theme.foreground,
                    0.14,
                ),
                self.options.opacity,
                self.size,
            );
        }

        if let Some(drag) = tab_drag.filter(|drag| !drag.outside_tab_bar) {
            let left = drag.target as f32 * tab_width - tab_scroll;
            push_rect(
                &mut self.rect_scratch,
                ViewportRect {
                    x: left * scale,
                    y: 3.0 * scale,
                    width: 2.0 * scale,
                    height: (TAB_BAR_HEIGHT as f32 - 6.0) * scale,
                },
                self.options.theme.foreground,
                self.options.opacity,
                self.size,
            );
        }

        if let Some(palette) = palette {
            let rect = palette.layout.outer;
            push_rect(
                &mut self.rect_scratch,
                rect,
                mix_color(
                    self.options.theme.tab_bar,
                    self.options.theme.background,
                    0.55,
                ),
                1.0,
                self.size,
            );
            push_rect(
                &mut self.rect_scratch,
                palette.layout.input,
                mix_color(
                    self.options.theme.inactive_tab,
                    self.options.theme.background,
                    0.35,
                ),
                1.0,
                self.size,
            );
            if let Some(row) = palette.layout.rows.get(palette.selected) {
                push_rect(
                    &mut self.rect_scratch,
                    *row,
                    mix_color(
                        self.options.theme.active_tab,
                        self.options.theme.foreground,
                        0.16,
                    ),
                    self.options.opacity,
                    self.size,
                );
            }
        }

        // Floating/dragged tab.
        if let Some(drag) = self.tab_drag {
            let lift_scale = 1.02;
            let ghost_width = self.tab_drag_width(tab_width) * scale * lift_scale;
            let ghost_height = (TAB_BAR_HEIGHT as f32 - 4.0) * scale * lift_scale;
            let ghost_x = drag.x - (ghost_width - self.tab_drag_width(tab_width) * scale) / 2.0;
            let ghost_y = drag.y - (ghost_height - (TAB_BAR_HEIGHT as f32 - 4.0) * scale) / 2.0;

            // Shadow.
            push_rect(
                &mut self.rect_scratch,
                ViewportRect {
                    x: ghost_x + 3.0 * scale,
                    y: ghost_y + 4.0 * scale,
                    width: ghost_width,
                    height: ghost_height,
                },
                self.options.theme.tab_bar,
                self.options.opacity * 0.55,
                self.size,
            );

            // Сам поднятый tab.
            push_rect(
                &mut self.rect_scratch,
                ViewportRect {
                    x: ghost_x,
                    y: ghost_y,
                    width: ghost_width,
                    height: ghost_height,
                },
                self.options.theme.active_tab,
                self.options.opacity,
                self.size,
            );
        }
    }

    fn tab_drag_width(&self, tab_width: f32) -> f32 {
        if self.tab_drag.is_some_and(|drag| drag.outside_tab_bar) {
            DETACHED_TAB_PREFERRED_WIDTH.clamp(DETACHED_TAB_MIN_WIDTH, DETACHED_TAB_MAX_WIDTH)
        } else {
            tab_width
        }
    }

    fn tab_drop_position(&self, tab_width: f32, tab_scroll: f32) -> Option<(usize, f32)> {
        let drop = self.tab_drop?;
        let progress = (drop.started_at.elapsed().as_secs_f32() / TAB_DROP_DURATION.as_secs_f32())
            .clamp(0.0, 1.0);
        let eased = ease_out_cubic(progress);
        let destination = drop.index as f32 * tab_width - tab_scroll;
        let from = drop.from_x / self.scale_factor.max(1.0) as f32;
        Some((drop.index, from + (destination - from) * eased))
    }

    fn build_pane_rectangles(&mut self, pane: &PaneView<'_>) {
        let theme = &self.options.theme;
        push_rect(
            &mut self.rect_scratch,
            pane.rect,
            theme.background,
            self.options.opacity,
            self.size,
        );

        // Тонкая граница присутствует у каждого pane.
        // Поэтому соседние panes визуально всегда разделены.
        let border = (PANE_BORDER_WIDTH * self.scale_factor).max(1.0) as f32;
        let border_color = if pane.focused {
            mix_color(theme.pane_border, theme.foreground, 0.22)
        } else {
            theme.pane_border
        };

        for rect in border_rectangles(pane.rect, border) {
            push_rect(
                &mut self.rect_scratch,
                rect,
                border_color,
                self.options.opacity,
                self.size,
            );
        }

        if let HoverTarget::PaneDivider {
            x,
            y,
            vertical,
            active,
        } = self.hover
        {
            let distance = 7.0 * self.scale_factor as f32;
            let color = mix_color(
                theme.pane_border,
                theme.foreground,
                if active { 0.65 } else { 0.38 },
            );
            let thickness = if active { 2.0 } else { 1.5 } * self.scale_factor as f32;
            if vertical {
                for edge in [pane.rect.x, pane.rect.x + pane.rect.width] {
                    if (edge - x).abs() <= distance {
                        push_rect(
                            &mut self.rect_scratch,
                            ViewportRect {
                                x: edge - thickness / 2.0,
                                y: pane.rect.y,
                                width: thickness,
                                height: pane.rect.height,
                            },
                            color,
                            self.options.opacity,
                            self.size,
                        );
                    }
                }
            } else {
                for edge in [pane.rect.y, pane.rect.y + pane.rect.height] {
                    if (edge - y).abs() <= distance {
                        push_rect(
                            &mut self.rect_scratch,
                            ViewportRect {
                                x: pane.rect.x,
                                y: edge - thickness / 2.0,
                                width: pane.rect.width,
                                height: thickness,
                            },
                            color,
                            self.options.opacity,
                            self.size,
                        );
                    }
                }
            }
        }

        let width = self.size.width as f32;
        let height = self.size.height as f32;
        let cell_width = (self.font_layout.cell_width * self.scale_factor) as f32;
        let cell_height = (self.font_layout.cell_height * self.scale_factor) as f32;
        let padding = (self.options.padding * self.scale_factor) as f32;
        let full_size = [2.0 * cell_width / width, 2.0 * cell_height / height];
        let cursor = pane.screen.display_cursor();
        for row in 0..pane.screen.rows() {
            for column in 0..pane.screen.columns() {
                let Some(cell) = pane.screen.display_cell(row, column) else {
                    continue;
                };
                let is_block_cursor = cursor.is_some_and(|cursor| {
                    pane.focused
                        && cursor.visible
                        && cursor.shape == CursorShape::Block
                        && cursor.row == row
                        && cursor.column == column
                });
                let (_, mut background) = effective_colors(cell, is_block_cursor, theme);
                if pane.screen.selected_at(row, column) {
                    background = theme.selection_background;
                } else if pane.screen.search_match_at(row, column) {
                    background = SEARCH_BACKGROUND;
                }
                if background != theme.background {
                    self.rect_scratch.push(RectInstance {
                        origin: absolute_origin(
                            pane.rect.x + padding + column as f32 * cell_width,
                            pane.rect.y + padding + row as f32 * cell_height,
                            width,
                            height,
                        ),
                        size: full_size,
                        color: linear_color(background, self.options.opacity),
                    });
                }
            }
        }
        if let Some(cursor) = cursor
            .filter(|cursor| cursor.visible && cursor.shape != CursorShape::Block && pane.focused)
        {
            let (x_size, y_size, y_offset) = match cursor.shape {
                CursorShape::Underline => (
                    cell_width,
                    2.0 * self.scale_factor as f32,
                    cell_height - 2.0 * self.scale_factor as f32,
                ),
                CursorShape::Bar => (2.0 * self.scale_factor as f32, cell_height, 0.0),
                CursorShape::Block => unreachable!(),
            };
            self.rect_scratch.push(RectInstance {
                origin: absolute_origin(
                    pane.rect.x + padding + cursor.column as f32 * cell_width,
                    pane.rect.y + padding + cursor.row as f32 * cell_height + y_offset,
                    width,
                    height,
                ),
                size: [2.0 * x_size / width, 2.0 * y_size / height],
                color: linear_color(theme.cursor, self.options.opacity),
            });
        }
    }

    fn ensure_rect_capacity(&mut self, required: usize) {
        if required <= self.rect_capacity {
            return;
        }
        self.rect_capacity = required.next_power_of_two();
        self.rect_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminal rectangles"),
            size: (self.rect_capacity * mem::size_of::<RectInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
}

fn tab_preview_index(index: usize, drag: Option<TabDragVisual>) -> usize {
    let Some(drag) = drag.filter(|drag| !drag.outside_tab_bar) else {
        return index;
    };
    if drag.source < drag.target && index > drag.source && index <= drag.target {
        index - 1
    } else if drag.source > drag.target && index >= drag.target && index < drag.source {
        index + 1
    } else {
        index
    }
}

fn tab_close_rect(tab: LogicalRect) -> LogicalRect {
    LogicalRect {
        x: tab.x + tab.width - TAB_CLOSE_RIGHT_INSET - TAB_CLOSE_WIDTH,
        y: tab.y + TAB_BUTTON_VERTICAL_INSET,
        width: TAB_CLOSE_WIDTH,
        height: tab.height - TAB_BUTTON_VERTICAL_INSET * 2.0,
    }
}

fn tab_title_rect(tab: LogicalRect) -> LogicalRect {
    let close = tab_close_rect(tab);
    LogicalRect {
        x: tab.x + TAB_TITLE_LEFT_PADDING,
        y: tab.y,
        width: (close.x - (tab.x + TAB_TITLE_LEFT_PADDING)).max(0.0),
        height: tab.height,
    }
}

fn new_tab_rect(tab_strip_width: f32) -> LogicalRect {
    LogicalRect {
        x: tab_strip_width,
        y: NEW_TAB_VERTICAL_INSET,
        width: NEW_TAB_WIDTH as f32,
        height: TAB_BAR_HEIGHT as f32 - NEW_TAB_VERTICAL_INSET * 2.0,
    }
}

fn text_bounds(rect: ViewportRect) -> TextBounds {
    TextBounds {
        left: rect.x.ceil() as i32,
        top: rect.y.ceil() as i32,
        right: (rect.x + rect.width).floor() as i32,
        bottom: (rect.y + rect.height).floor() as i32,
    }
}

fn ease_out_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - progress).powi(3)
}

pub fn grid_size(size: PhysicalSize<u32>, scale_factor: f64) -> (u16, u16) {
    let columns = ((size.width as f64 - 2.0 * PADDING * scale_factor)
        / (CELL_WIDTH * scale_factor))
        .floor()
        .max(1.0);
    let rows = ((size.height as f64 - 2.0 * PADDING * scale_factor) / (CELL_HEIGHT * scale_factor))
        .floor()
        .max(1.0);
    (
        columns.min(u16::MAX as f64) as u16,
        rows.min(u16::MAX as f64) as u16,
    )
}

fn grid_size_for_layout(
    rect: ViewportRect,
    scale_factor: f64,
    padding: f64,
    layout: &FontLayout,
) -> (u16, u16) {
    let content_width = (f64::from(rect.width) - 2.0 * padding * scale_factor).max(1.0);
    let content_height = (f64::from(rect.height) - 2.0 * padding * scale_factor).max(1.0);
    (
        (content_width / (layout.cell_width * scale_factor))
            .floor()
            .clamp(1.0, u16::MAX as f64) as u16,
        (content_height / (layout.cell_height * scale_factor))
            .floor()
            .clamp(1.0, u16::MAX as f64) as u16,
    )
}

fn sync_row_buffers(buffers: &mut PaneBuffers, rows: usize) {
    buffers.rows.truncate(rows);
    buffers.row_signatures.resize(rows, 0);
    while buffers.rows.len() < rows {
        buffers.rows.push(RowBuffers::default());
    }
}

fn pane_layout_signature(panes: &[PaneView<'_>]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for pane in panes {
        pane.id.hash(&mut hasher);
        pane.rect.x.to_bits().hash(&mut hasher);
        pane.rect.y.to_bits().hash(&mut hasher);
        pane.rect.width.to_bits().hash(&mut hasher);
        pane.rect.height.to_bits().hash(&mut hasher);
        pane.focused.hash(&mut hasher);
    }
    hasher.finish()
}

fn tab_signature(tabs: &[TabLabel]) -> u64 {
    let mut hasher = DefaultHasher::new();
    tabs.hash(&mut hasher);
    hasher.finish()
}

fn command_palette_signature(palette: Option<&CommandPaletteView<'_>>) -> u64 {
    let mut hasher = DefaultHasher::new();
    if let Some(palette) = palette {
        palette.query.hash(&mut hasher);
        palette.selected.hash(&mut hasher);
        palette.scroll_offset.hash(&mut hasher);
        for row in palette.rows {
            row.title.hash(&mut hasher);
            row.category.hash(&mut hasher);
            row.shortcut.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn truncate_title(title: &str, max_chars: usize) -> String {
    let mut output: String = title.chars().take(max_chars).collect();
    if title.chars().count() > max_chars {
        output.pop();
        output.push('…');
    }
    output
}

fn muted(foreground: Rgb, background: Rgb) -> Rgb {
    Rgb::new(
        ((u16::from(foreground.r) + u16::from(background.r) * 2) / 3) as u8,
        ((u16::from(foreground.g) + u16::from(background.g) * 2) / 3) as u8,
        ((u16::from(foreground.b) + u16::from(background.b) * 2) / 3) as u8,
    )
}

fn mix_color(from: Rgb, to: Rgb, amount: f32) -> Rgb {
    let mix = |from: u8, to: u8| {
        (f32::from(from) + (f32::from(to) - f32::from(from)) * amount.clamp(0.0, 1.0)).round() as u8
    };
    Rgb::new(mix(from.r, to.r), mix(from.g, to.g), mix(from.b, to.b))
}

fn border_rectangles(rect: ViewportRect, width: f32) -> [ViewportRect; 4] {
    [
        ViewportRect {
            height: width,
            ..rect
        },
        ViewportRect {
            y: rect.y + rect.height - width,
            height: width,
            ..rect
        },
        ViewportRect { width, ..rect },
        ViewportRect {
            x: rect.x + rect.width - width,
            width,
            ..rect
        },
    ]
}

fn push_rect(
    output: &mut Vec<RectInstance>,
    rect: ViewportRect,
    color: Rgb,
    opacity: f32,
    surface: PhysicalSize<u32>,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    output.push(RectInstance {
        origin: absolute_origin(rect.x, rect.y, surface.width as f32, surface.height as f32),
        size: [
            2.0 * rect.width / surface.width.max(1) as f32,
            2.0 * rect.height / surface.height.max(1) as f32,
        ],
        color: linear_color(color, opacity),
    });
}

fn absolute_origin(x: f32, y: f32, width: f32, height: f32) -> [f32; 2] {
    [-1.0 + 2.0 * x / width, 1.0 - 2.0 * y / height]
}

fn configure_font(font_system: &mut FontSystem, options: &RenderOptions) -> FontLayout {
    // macOS ships a legacy bitmap GB18030 face whose scalable metrics report
    // an infinite advance through cosmic-text. It cannot be rasterized in this
    // GPU path; removing it lets the normal CJK fallback select a scalable face.
    let incompatible_faces: Vec<_> = font_system
        .db()
        .faces()
        .filter(|face| face.post_script_name == "GB18030Bitmap")
        .map(|face| face.id)
        .collect();
    for id in incompatible_faces {
        font_system.db_mut().remove_face(id);
    }

    let mut candidates = vec![options.font_family.as_str()];
    candidates.extend(options.fallback_families.iter().map(String::as_str));
    candidates.extend([
        "SF Mono",
        "Menlo",
        "Monaco",
        "JetBrains Mono",
        "Cascadia Mono",
        "DejaVu Sans Mono",
    ]);
    for family in candidates {
        let id = font_system.db().query(&fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        });
        let Some(id) = id else { continue };
        let Some(font) = font_system.get_font(id, fontdb::Weight::NORMAL) else {
            continue;
        };
        let metrics = font.metrics();
        let units_per_em = f64::from(metrics.units_per_em.max(1));
        let font_size = options.font_size.clamp(6.0, 72.0);
        let ascent = f64::from(metrics.ascent) / units_per_em * f64::from(font_size);
        let descent = f64::from(metrics.descent.abs()) / units_per_em * f64::from(font_size);
        let leading = f64::from(metrics.leading.max(0.0)) / units_per_em * f64::from(font_size);
        let natural_height = ascent + descent + leading;
        let line_height_scale = options.line_height.unwrap_or(LINE_HEIGHT_SCALE);
        let cell_height = (natural_height * f64::from(line_height_scale))
            .ceil()
            .max(f64::from(font_size));
        let advance_ratio = font.monospace_em_width().unwrap_or_else(|| {
            metrics
                .average_width
                .map(|width| width / f32::from(metrics.units_per_em.max(1)))
                .unwrap_or(0.6)
        });
        let layout = FontLayout {
            family: family.to_owned(),
            font_size,
            cell_width: f64::from(advance_ratio * font_size),
            cell_height,
            baseline: (cell_height - natural_height) / 2.0 + ascent,
            ascent,
            descent,
        };
        font_system.db_mut().set_monospace_family(family);
        return layout;
    }

    FontLayout {
        family: "system monospace".to_owned(),
        font_size: options.font_size,
        cell_width: CELL_WIDTH,
        cell_height: CELL_HEIGHT,
        baseline: 14.0,
        ascent: 11.0,
        descent: 3.0,
    }
}

fn debug_metrics(size: PhysicalSize<u32>, scale_factor: f64, layout: &FontLayout) {
    if cfg!(debug_assertions) || std::env::var_os("GRIN_PROFILE_STARTUP").is_some() {
        eprintln!(
            "renderer: logical={:.1}x{:.1}pt physical={}x{}px scale={:.2} font='{}' requested={:.1}pt raster={:.1}px cell={:.2}x{:.2}pt ({:.2}x{:.2}px) baseline={:.2}px ascent={:.2}px descent={:.2}px",
            size.width as f64 / scale_factor,
            size.height as f64 / scale_factor,
            size.width,
            size.height,
            scale_factor,
            layout.family,
            layout.font_size,
            layout.font_size as f64 * scale_factor,
            layout.cell_width,
            layout.cell_height,
            layout.cell_width * scale_factor,
            layout.cell_height * scale_factor,
            layout.baseline * scale_factor,
            layout.ascent * scale_factor,
            layout.descent * scale_factor,
        );
    }
}

fn row_signature(screen: &Screen, row: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    let cursor = screen.display_cursor();
    if let Some(line) = screen.display_line(row) {
        for (column, cell) in line.iter().enumerate() {
            cell.text.hash(&mut hasher);
            cell.attributes.hash(&mut hasher);
            cell.width.hash(&mut hasher);
            cell.hyperlink.is_some().hash(&mut hasher);
            screen.selected_at(row, column).hash(&mut hasher);
            screen.search_match_at(row, column).hash(&mut hasher);
            cursor
                .is_some_and(|cursor| {
                    cursor.visible
                        && cursor.shape == CursorShape::Block
                        && cursor.row == row
                        && cursor.column == column
                })
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn cell_needs_glyph_span(cell: &Cell) -> bool {
    !cell.is_blank()
        || cell.attributes.underline
        || cell.attributes.strikethrough
        || cell.hyperlink.is_some()
}

fn glyph_attrs(
    cell: &Cell,
    selected: bool,
    search: bool,
    block_cursor: bool,
    theme: &RenderTheme,
) -> Attrs<'static> {
    let (mut foreground, _) = effective_colors(cell, block_cursor, theme);
    if selected || search {
        foreground = theme.selection_foreground;
    }
    let mut attrs = Attrs::new()
        .family(Family::Monospace)
        .color(glyph_color(foreground));
    if cell.attributes.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if cell.attributes.italic {
        attrs = attrs.style(Style::Italic);
    }
    if cell.attributes.underline || cell.hyperlink.is_some() {
        attrs = attrs.underline(UnderlineStyle::Single);
    }
    if cell.attributes.strikethrough {
        attrs = attrs.strikethrough();
    }
    attrs
}

fn effective_colors(cell: &Cell, block_cursor: bool, theme: &RenderTheme) -> (Rgb, Rgb) {
    let mut foreground = resolve_color(cell.attributes.foreground, true, theme);
    let mut background = resolve_color(cell.attributes.background, false, theme);
    if cell.attributes.dim {
        foreground = Rgb::new(foreground.r / 2, foreground.g / 2, foreground.b / 2);
    }
    if cell.attributes.hidden {
        foreground = background;
    }
    if cell.attributes.inverse || block_cursor {
        mem::swap(&mut foreground, &mut background);
    }
    (foreground, background)
}

fn resolve_color(color: Color, foreground: bool, theme: &RenderTheme) -> Rgb {
    match color {
        Color::Default => {
            if foreground {
                theme.foreground
            } else {
                theme.background
            }
        }
        Color::Indexed(index) => indexed_color(index, &theme.ansi),
        Color::Rgb(color) => color,
    }
}

fn default_ansi() -> [Rgb; 16] {
    [
        Rgb::new(0, 0, 0),
        Rgb::new(205, 49, 49),
        Rgb::new(13, 188, 121),
        Rgb::new(229, 229, 16),
        Rgb::new(36, 114, 200),
        Rgb::new(188, 63, 188),
        Rgb::new(17, 168, 205),
        Rgb::new(229, 229, 229),
        Rgb::new(102, 102, 102),
        Rgb::new(241, 76, 76),
        Rgb::new(35, 209, 139),
        Rgb::new(245, 245, 67),
        Rgb::new(59, 142, 234),
        Rgb::new(214, 112, 214),
        Rgb::new(41, 184, 219),
        Rgb::new(255, 255, 255),
    ]
}

fn indexed_color(index: u8, ansi: &[Rgb; 16]) -> Rgb {
    match index {
        0..=15 => ansi[index as usize],
        16..=231 => {
            let value = index - 16;
            let level = |component: u8| {
                if component == 0 {
                    0
                } else {
                    55 + component * 40
                }
            };
            Rgb::new(level(value / 36), level((value / 6) % 6), level(value % 6))
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            Rgb::new(gray, gray, gray)
        }
    }
}

fn glyph_color(color: Rgb) -> GlyphColor {
    GlyphColor::rgb(color.r, color.g, color.b)
}

fn linear_color(color: Rgb, opacity: f32) -> [f32; 4] {
    [
        srgb_channel_to_linear(color.r) as f32,
        srgb_channel_to_linear(color.g) as f32,
        srgb_channel_to_linear(color.b) as f32,
        opacity,
    ]
}

fn wgpu_color(color: Rgb, opacity: f32) -> wgpu::Color {
    wgpu::Color {
        r: srgb_channel_to_linear(color.r),
        g: srgb_channel_to_linear(color.g),
        b: srgb_channel_to_linear(color.b),
        a: f64::from(opacity),
    }
}

fn srgb_channel_to_linear(channel: u8) -> f64 {
    let encoded = f64::from(channel) / 255.0;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_grid_accounts_for_display_scale() {
        assert_eq!(grid_size(PhysicalSize::new(900, 720), 2.0), (48, 19));
    }

    #[test]
    fn grid_never_becomes_zero_sized() {
        assert_eq!(grid_size(PhysicalSize::new(0, 0), 1.0), (1, 1));
    }

    #[test]
    fn xterm_color_cube_is_resolved() {
        let ansi = default_ansi();
        assert_eq!(indexed_color(16, &ansi), Rgb::new(0, 0, 0));
        assert_eq!(indexed_color(231, &ansi), Rgb::new(255, 255, 255));
        assert_eq!(indexed_color(232, &ansi), Rgb::new(8, 8, 8));
    }

    #[test]
    fn rectangle_colors_are_linearized_for_srgb_surfaces() {
        assert_eq!(srgb_channel_to_linear(0), 0.0);
        assert_eq!(srgb_channel_to_linear(255), 1.0);
        assert!((srgb_channel_to_linear(128) - 0.21586).abs() < 0.00001);
    }

    #[test]
    fn hover_surfaces_remain_distinct_from_foreground_glyphs() {
        let theme = RenderTheme::default();
        let close_hover = mix_color(theme.active_tab, theme.foreground, 0.22);
        let new_tab_hover = mix_color(theme.tab_bar, theme.foreground, 0.14);
        assert_ne!(close_hover, theme.active_tab);
        assert_ne!(close_hover, theme.foreground);
        assert_ne!(new_tab_hover, theme.tab_bar);
        assert_ne!(new_tab_hover, theme.foreground);
    }

    #[test]
    fn tab_control_rectangles_share_logical_tab_bar_geometry() {
        let tab = LogicalRect {
            x: 120.0,
            y: 0.0,
            width: 160.0,
            height: TAB_BAR_HEIGHT as f32,
        };
        let close = tab_close_rect(tab);
        let plus = new_tab_rect(764.0);

        assert_eq!(
            close,
            LogicalRect {
                x: 252.0,
                y: 4.0,
                width: 24.0,
                height: 24.0
            }
        );
        assert_eq!(
            plus,
            LogicalRect {
                x: 764.0,
                y: 2.0,
                width: 36.0,
                height: 28.0
            }
        );
        assert!(close.contains(264.0, 16.0));
        assert!(!close.contains(276.0, 16.0));
        assert_eq!(
            close.physical(2.0),
            ViewportRect {
                x: 504.0,
                y: 8.0,
                width: 48.0,
                height: 48.0
            }
        );
    }

    #[test]
    fn tab_drag_preview_shifts_only_tabs_between_source_and_target() {
        let forward = Some(TabDragVisual {
            source: 0,
            target: 2,
            x: 0.0,
            y: 0.0,
            outside_tab_bar: false,
        });
        assert_eq!(tab_preview_index(1, forward), 0);
        assert_eq!(tab_preview_index(2, forward), 1);

        let backward = Some(TabDragVisual {
            source: 2,
            target: 0,
            x: 0.0,
            y: 0.0,
            outside_tab_bar: false,
        });
        assert_eq!(tab_preview_index(0, backward), 1);
        assert_eq!(tab_preview_index(1, backward), 2);

        let outside = backward.map(|mut drag| {
            drag.outside_tab_bar = true;
            drag
        });
        assert_eq!(tab_preview_index(0, outside), 0);
    }

    #[test]
    fn tab_drop_easing_is_bounded_and_finishes_exactly() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!(ease_out_cubic(0.5) > 0.5);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
    }

    #[test]
    fn fallback_text_stays_on_one_finite_fixed_grid_row() {
        let mut font_system = FontSystem::new();
        let layout = configure_font(&mut font_system, &RenderOptions::default());
        let scale = 2.0;
        let mut buffer = Buffer::new(&mut font_system, layout.metrics(scale));
        buffer.set_wrap(Wrap::None);
        buffer.set_text(
            "Unicode: e\u{301} | 界 | 🇦🇲 | 🙂",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        let runs: Vec<_> = buffer.layout_runs().collect();
        assert_eq!(runs.len(), 1);
        assert!(
            runs[0]
                .glyphs
                .iter()
                .all(|glyph| glyph.x.is_finite() && glyph.x < 1_000.0)
        );
        assert!(runs[0].glyphs.last().unwrap().x < 500.0);
    }
}
