use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use arboard::Clipboard;
use terminal_renderer::{PaneView, RenderOptions, RenderTheme, Renderer, TabLabel, ViewportRect};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

use crate::{
    action::{Action, KeyBindings, KeyChord},
    config::{AppConfig, ThemePalette, workspace_path},
    layout::{Direction, PaneId, Rect, SplitAxis, SplitHandle},
    session::{TabSession, TerminalPane},
    workspace::{TabState, WORKSPACE_VERSION, Workspace},
};

enum UserEvent {
    PtyReady,
    InitialReady(Result<InitialSessions, String>),
    PaneSpawned {
        target: SpawnTarget,
        result: Result<TerminalPane, String>,
    },
    TabRestored(Result<TabSession, String>),
}

enum SpawnTarget {
    NewTab { title: Option<String> },
    Split { source: PaneId, axis: SplitAxis },
}

struct InitialSessions {
    tabs: Vec<TabSession>,
    active_tab: usize,
    next_pane_id: u64,
    diagnostic: Option<String>,
}

#[derive(Clone, Copy)]
struct PendingSelection {
    pane: PaneId,
    row: usize,
    column: usize,
    position: PhysicalPosition<f64>,
    rectangular: bool,
}

#[derive(Clone, Copy)]
struct MouseClickState {
    when: Instant,
    pane: PaneId,
    position: PhysicalPosition<f64>,
    count: u8,
}

struct PaneResizeDrag {
    handle: SplitHandle,
    last_position: PhysicalPosition<f64>,
}

pub struct Application {
    started_at: Instant,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    tabs: Vec<TabSession>,
    closed_tabs: Vec<TabState>,
    active_tab: usize,
    next_pane_id: u64,
    startup_workspace: Option<Workspace>,
    workspace_path: PathBuf,
    config: AppConfig,
    keybindings: KeyBindings,
    theme_name: String,
    clipboard: Option<Clipboard>,
    modifiers: ModifiersState,
    cursor_position: PhysicalPosition<f64>,
    selecting: Option<PaneId>,
    selection_pending: Option<PendingSelection>,
    last_left_click: Option<MouseClickState>,

    dragging_tab: Option<usize>,
    tab_drag_changed: bool,

    resizing_split: Option<PaneResizeDrag>,
    split_resize_changed: bool,

    rename_input: Option<String>,
    pending_pinned_close: Option<usize>,
    first_shell_output_seen: bool,
    first_frame_seen: bool,
    profile_input: bool,
    fatal_error: Option<anyhow::Error>,
}

impl Application {
    fn new(proxy: EventLoopProxy<UserEvent>, started_at: Instant) -> Self {
        let loaded = AppConfig::load();
        for diagnostic in &loaded.diagnostics {
            eprintln!("config: {diagnostic}");
        }
        let workspace_path = workspace_path();
        let workspace_load = if loaded.config.workspace.restore {
            Workspace::load(&workspace_path)
        } else {
            crate::workspace::WorkspaceLoad {
                workspace: None,
                diagnostic: None,
            }
        };
        if let Some(diagnostic) = &workspace_load.diagnostic {
            eprintln!("workspace: {diagnostic}");
        }
        let theme_name = workspace_load
            .workspace
            .as_ref()
            .map(|workspace| workspace.theme.clone())
            .unwrap_or_else(|| loaded.config.theme.active.clone());
        let keybindings = KeyBindings::with_overrides(&loaded.config.keybindings);
        for diagnostic in keybindings.diagnostics() {
            eprintln!("keybinding: {diagnostic}");
        }
        if cfg!(debug_assertions) {
            eprintln!("config: {}", loaded.path.display());
        }
        Self {
            started_at,
            proxy,
            window: None,
            renderer: None,
            tabs: Vec::new(),
            closed_tabs: Vec::new(),
            active_tab: 0,
            next_pane_id: 1,
            startup_workspace: workspace_load.workspace,
            workspace_path,
            config: loaded.config,
            keybindings,
            theme_name,
            clipboard: None,
            modifiers: ModifiersState::empty(),
            cursor_position: PhysicalPosition::new(0.0, 0.0),
            selecting: None,
            selection_pending: None,
            last_left_click: None,

            dragging_tab: None,
            tab_drag_changed: false,

            resizing_split: None,
            split_resize_changed: false,

            rename_input: None,
            pending_pinned_close: None,
            first_shell_output_seen: false,
            first_frame_seen: false,
            profile_input: std::env::var_os("GRIN_PROFILE_INPUT").is_some(),
            fatal_error: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let initial_size = self
            .startup_workspace
            .as_ref()
            .map(|workspace| {
                LogicalSize::new(
                    workspace.window_width as f64,
                    workspace.window_height as f64,
                )
            })
            .unwrap_or_else(|| LogicalSize::new(960.0, 600.0));
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Grin Terminal")
                        .with_inner_size(initial_size)
                        .with_min_inner_size(LogicalSize::new(480.0, 260.0))
                        .with_transparent(self.config.window.opacity < 1.0),
                )
                .context("could not create native window")?,
        );
        window.set_ime_allowed(true);
        timing(self.started_at, "window created");
        let renderer = pollster::block_on(Renderer::new_with_options(
            Arc::clone(&window),
            self.render_options(),
        ))
        .context("GPU renderer initialization failed")?;
        timing(self.started_at, "GPU and font renderer initialized");
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.queue_initial_sessions()?;
        self.update_window_title();
        self.request_redraw();
        Ok(())
    }

    fn render_options(&self) -> RenderOptions {
        let (theme, diagnostics) = self.config.active_theme(Some(&self.theme_name));
        for diagnostic in diagnostics {
            eprintln!("theme: {diagnostic}");
        }
        RenderOptions {
            font_family: self.config.font.family.clone(),
            fallback_families: self.config.font.fallback_families.clone(),
            font_size: self.config.font.size,
            line_height: self.config.font.line_height,
            padding: f64::from(self.config.window.padding),
            opacity: self.config.window.opacity,
            theme: render_theme(&theme),
        }
    }

    fn queue_initial_sessions(&mut self) -> Result<()> {
        let workspace = self.startup_workspace.take();
        let cursor_shape = self.config.cursor_shape();
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name("workspace-restorer".into())
            .spawn(move || {
                let result = build_initial_sessions(workspace, cursor_shape, proxy.clone())
                    .map_err(|error| error.to_string());
                let _ = proxy.send_event(UserEvent::InitialReady(result));
            })
            .context("could not start workspace restoration worker")?;
        Ok(())
    }

    fn complete_initial_sessions(&mut self, result: Result<InitialSessions, String>) {
        match result {
            Ok(initial) => {
                if let Some(diagnostic) = initial.diagnostic {
                    eprintln!("workspace: {diagnostic}");
                }
                self.tabs = initial.tabs;
                self.active_tab = initial.active_tab;
                self.next_pane_id = initial.next_pane_id;
                self.resize_active_panes();
                self.process_pty_events();
                timing(self.started_at, "workspace PTYs spawned");
                self.update_window_title();
                self.request_redraw();
            }
            Err(error) => {
                self.fatal_error =
                    Some(anyhow::anyhow!("could not start initial terminal: {error}"));
                if let Some(window) = &self.window {
                    window.set_title("Grin — shell startup failed");
                }
            }
        }
    }

    fn queue_pane_spawn(
        &mut self,
        target: SpawnTarget,
        columns: u16,
        rows: u16,
        working_directory: PathBuf,
    ) {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        let cursor_shape = self.config.cursor_shape();
        let event_proxy = self.proxy.clone();
        let wake_proxy = self.proxy.clone();
        let spawn = std::thread::Builder::new()
            .name("pty-spawner".into())
            .spawn(move || {
                let result = TerminalPane::spawn(
                    id,
                    columns,
                    rows,
                    working_directory,
                    cursor_shape,
                    move || {
                        let _ = wake_proxy.send_event(UserEvent::PtyReady);
                    },
                )
                .map_err(|error| error.to_string());
                let _ = event_proxy.send_event(UserEvent::PaneSpawned { target, result });
            });
        if let Err(error) = spawn {
            eprintln!("could not start PTY spawner thread: {error}");
        }
    }

    fn queue_new_tab(&mut self, working_directory: PathBuf) {
        let (columns, rows) = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.grid_size_for(renderer.content_rect()))
            .unwrap_or((80, 24));

        self.queue_pane_spawn(
            SpawnTarget::NewTab { title: None },
            columns,
            rows,
            working_directory,
        );
    }

    fn queue_duplicate_tab(&mut self) {
        let (working_directory, title) = {
            let tab = self.active_tab();
            (
                tab.source_directory().to_path_buf(),
                format!("{} copy", tab.title),
            )
        };

        let (columns, rows) = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.grid_size_for(renderer.content_rect()))
            .unwrap_or((80, 24));

        self.queue_pane_spawn(
            SpawnTarget::NewTab { title: Some(title) },
            columns,
            rows,
            working_directory,
        );
    }

    fn queue_restore_closed_tab(&mut self) {
        let Some(state) = self.closed_tabs.pop() else {
            return;
        };

        let cursor_shape = self.config.cursor_shape();
        let restore_proxy = self.proxy.clone();
        let event_proxy = self.proxy.clone();

        let spawn = std::thread::Builder::new()
            .name("tab-restorer".into())
            .spawn(move || {
                let result = restore_tab_session(state, cursor_shape, restore_proxy)
                    .map_err(|error| error.to_string());

                let _ = event_proxy.send_event(UserEvent::TabRestored(result));
            });

        if let Err(error) = spawn {
            eprintln!("could not start tab restorer thread: {error}");
        }
    }

    fn complete_tab_restore(&mut self, result: Result<TabSession, String>) {
        let tab = match result {
            Ok(tab) => tab,
            Err(error) => {
                eprintln!("could not restore closed tab: {error}");
                return;
            }
        };

        let target = if tab.pinned {
            self.tabs.iter().take_while(|tab| tab.pinned).count()
        } else {
            self.tabs.len()
        };

        self.tabs.insert(target, tab);
        self.active_tab = target;

        self.resize_active_panes();
        self.process_pty_events();
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
    }

    fn complete_pane_spawn(&mut self, target: SpawnTarget, result: Result<TerminalPane, String>) {
        let pane = match result {
            Ok(pane) => pane,
            Err(error) => {
                eprintln!("could not start terminal session: {error}");
                return;
            }
        };
        match target {
            SpawnTarget::NewTab { title } => {
                let custom_title = title.is_some();
                let title = title.unwrap_or_else(|| format!("Terminal {}", self.tabs.len() + 1));

                let mut tab = TabSession::new(title, pane);
                tab.custom_title = custom_title;

                self.tabs.push(tab);
                self.active_tab = self.tabs.len() - 1;
            }
            SpawnTarget::Split { source, axis } => {
                let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.panes.contains_key(&source))
                else {
                    return;
                };
                if tab.insert_split_at(source, pane, axis).is_err() {
                    eprintln!("split target disappeared before its shell started");
                    return;
                }
            }
        }
        self.ensure_active_tab_visible();
        self.resize_active_panes();
        self.process_pty_events();
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
    }

    fn split_focused(&mut self, axis: SplitAxis) {
        let (directory, columns, rows) = {
            let pane = self.active_tab().focused();
            (
                pane.working_directory.clone(),
                pane.terminal.screen().columns() as u16,
                pane.terminal.screen().rows() as u16,
            )
        };
        let source = self.active_tab().focused_pane;
        self.queue_pane_spawn(
            SpawnTarget::Split { source, axis },
            columns,
            rows,
            directory,
        );
    }

    fn close_focused_pane(&mut self) -> bool {
        let Some(closed) = self.active_tab_mut().close_focused_pane() else {
            return false;
        };
        if let Some(renderer) = &mut self.renderer {
            renderer.remove_pane(closed.0);
        }
        self.resize_active_panes();
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
        true
    }

    fn close_tab_at(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return false;
        }

        self.pending_pinned_close = None;

        let snapshot = self.tabs[index].snapshot();
        let closed = self.tabs.remove(index);

        self.closed_tabs.push(snapshot);

        if self.closed_tabs.len() > 20 {
            self.closed_tabs.remove(0);
        }

        if let Some(renderer) = &mut self.renderer {
            for pane in closed.panes.keys() {
                renderer.remove_pane(pane.0);
            }
        }
        self.active_tab = if self.active_tab > index {
            self.active_tab - 1
        } else {
            self.active_tab.min(self.tabs.len() - 1)
        };
        self.resize_active_panes();
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
        true
    }

    fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.pending_pinned_close = None;
        self.active_tab = index;
        self.selecting = None;
        self.ensure_active_tab_visible();
        self.resize_active_panes();
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
    }

    fn toggle_active_tab_pin(&mut self) {
        if self.tabs.is_empty() {
            return;
        }

        let index = self.active_tab;

        self.tabs[index].pinned = !self.tabs[index].pinned;

        let tab = self.tabs.remove(index);

        let target = if tab.pinned {
            // Новый pinned tab всегда становится самым первым.
            0
        } else {
            // Unpin: ставим tab сразу после последнего pinned.
            self.tabs.iter().take_while(|tab| tab.pinned).count()
        };

        self.tabs.insert(target, tab);
        self.active_tab = target;

        self.ensure_active_tab_visible();
        self.resize_active_panes();

        self.pending_pinned_close = None;
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
    }

    fn confirm_pinned_tab_close(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || !self.tabs[index].pinned {
            self.pending_pinned_close = None;
            return true;
        }

        if self.pending_pinned_close == Some(index) {
            self.pending_pinned_close = None;
            return true;
        }

        self.pending_pinned_close = Some(index);
        self.update_window_title();
        self.request_redraw();

        false
    }

    fn move_active_tab(&mut self, direction: isize) {
        if self.tabs.len() < 2 {
            return;
        }

        let current = self.active_tab as isize;
        let target = current + direction;

        if target < 0 || target >= self.tabs.len() as isize {
            return;
        }

        let current = current as usize;
        let target = target as usize;

        // Pinned можно менять местами только с pinned.
        // Обычные tabs можно менять местами только с обычными.
        if self.tabs[current].pinned != self.tabs[target].pinned {
            return;
        }

        self.tabs.swap(current, target);
        self.active_tab = target;

        self.ensure_active_tab_visible();

        self.pending_pinned_close = None;
        self.persist_workspace();
        self.update_window_title();
        self.request_redraw();
    }

    fn active_tab(&self) -> &TabSession {
        &self.tabs[self.active_tab]
    }
    fn active_tab_mut(&mut self) -> &mut TabSession {
        &mut self.tabs[self.active_tab]
    }
    fn ensure_active_tab_visible(&mut self) {
        if self.tabs.is_empty() {
            return;
        }

        let index = self.active_tab;
        let count = self.tabs.len();

        if let Some(renderer) = &mut self.renderer {
            renderer.ensure_tab_visible(index, count);
        }
    }
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl Into<anyhow::Error>) {
        self.fatal_error = Some(error.into());
        event_loop.exit();
    }

    fn process_pty_events(&mut self) {
        let active_tab = self.active_tab;
        let mut redraw = false;
        let mut title_changed = false;
        for (tab_index, tab) in self.tabs.iter_mut().enumerate() {
            let ids: Vec<_> = tab.panes.keys().copied().collect();
            for id in ids {
                let update = tab
                    .panes
                    .get_mut(&id)
                    .expect("pane id came from the same map")
                    .process_pty_events();
                if update.changed && !self.first_shell_output_seen {
                    self.first_shell_output_seen = true;
                    timing(self.started_at, "first shell output");
                }
                if let Some(error) = update.error {
                    eprintln!("pane {} stopped: {error}", id.0);
                }
                redraw |= update.changed && tab_index == active_tab;
                if update.title_changed && id == tab.focused_pane && !tab.custom_title {
                    let terminal_title = tab.panes[&id].terminal.title();
                    if !terminal_title.is_empty() {
                        tab.title = terminal_title.to_owned();
                        title_changed = true;
                    }
                }
            }
        }
        if title_changed {
            self.update_window_title();
            redraw = true;
        }
        if redraw {
            self.request_redraw();
        }
    }

    fn resize(&mut self) {
        let Some(window) = &self.window else { return };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        renderer.resize(window.inner_size(), window.scale_factor());

        self.ensure_active_tab_visible();
        self.resize_active_panes();
        self.request_redraw();
    }

    fn active_layout(&self) -> Vec<(PaneId, Rect)> {
        if self.tabs.is_empty() {
            return Vec::new();
        }
        let Some(renderer) = &self.renderer else {
            return Vec::new();
        };
        let rect = renderer.content_rect();
        self.active_tab().layout(Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        })
    }

    fn resize_active_panes(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let sizes: Vec<_> = self
            .active_layout()
            .into_iter()
            .map(|(pane, rect)| {
                let size = self
                    .renderer
                    .as_ref()
                    .map(|renderer| renderer.grid_size_for(viewport(rect)))
                    .unwrap_or((80, 24));
                (pane, size)
            })
            .collect();
        let tab = self.active_tab_mut();
        for (id, (columns, rows)) in sizes {
            if let Some(pane) = tab.panes.get_mut(&id) {
                pane.resize(columns, rows);
            }
        }
    }

    fn collapse_keyboard_selection_to_edge(&mut self, toward_right: bool) -> bool {
        let application_cursor = self.active_tab().focused().terminal.application_cursor();

        let delta = {
            let screen = self.active_tab_mut().focused_mut().terminal.screen_mut();

            screen.collapse_keyboard_selection(toward_right)
        };

        let Some(delta) = delta else {
            return false;
        };

        self.selecting = None;
        self.selection_pending = None;
        self.last_left_click = None;

        if delta != 0 {
            let final_byte = if delta < 0 { b'D' } else { b'C' };

            let sequence = cursor_sequence(final_byte, application_cursor);

            let steps = delta.unsigned_abs();

            let mut bytes = Vec::with_capacity(sequence.len() * steps);

            for _ in 0..steps {
                bytes.extend_from_slice(&sequence);
            }

            self.active_tab().focused().write(bytes);
        }

        self.request_redraw();
        true
    }

    fn handle_selection_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        if event.state != ElementState::Pressed || !self.modifiers.shift_key() {
            return false;
        }

        // Cmd + Shift оставляем обычным shortcut'ам приложения.
        if self.modifiers.super_key() {
            return false;
        }

        // Во время rename/search keyboard selection не вмешивается.
        if self.rename_input.is_some() || self.active_tab().focused().search_query.is_some() {
            return false;
        }

        let key = match event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => NamedKey::ArrowLeft,
            Key::Named(NamedKey::ArrowRight) => NamedKey::ArrowRight,
            Key::Named(NamedKey::ArrowUp) => NamedKey::ArrowUp,
            Key::Named(NamedKey::ArrowDown) => NamedKey::ArrowDown,
            _ => return false,
        };

        let word_jump = self.modifiers.alt_key() || self.modifiers.control_key();

        {
            let screen = self.active_tab_mut().focused_mut().terminal.screen_mut();

            let already_selected = screen.selection().is_some();

            if !already_selected {
                match key {
                    // Shift + Left:
                    // сразу выделяем символ слева от cursor.
                    NamedKey::ArrowLeft => {
                        if !screen.begin_selection_from_cursor(true) {
                            return true;
                        }

                        // Option/Control + Shift + Left:
                        // сразу расширяем до начала слова.
                        if word_jump {
                            screen.move_selection_end_by_word(false);
                        }
                    }

                    // Shift + Right:
                    // начинаем selection с позиции cursor.
                    NamedKey::ArrowRight => {
                        if !screen.begin_selection_from_cursor(false) {
                            return true;
                        }

                        // Option/Control + Shift + Right:
                        // сразу расширяем до конца слова.
                        if word_jump {
                            screen.move_selection_end_by_word(true);
                        }
                    }

                    // Shift + Up:
                    // начинаем прямо от cursor и идём строкой вверх.
                    NamedKey::ArrowUp => {
                        if !screen.begin_selection_at_cursor() {
                            return true;
                        }

                        screen.move_selection_end_by_row(false);
                    }

                    // Shift + Down:
                    // начинаем прямо от cursor и идём строкой вниз.
                    NamedKey::ArrowDown => {
                        if !screen.begin_selection_at_cursor() {
                            return true;
                        }

                        screen.move_selection_end_by_row(true);
                    }

                    _ => unreachable!(),
                }
            } else {
                match key {
                    NamedKey::ArrowLeft if word_jump => {
                        screen.move_selection_end_by_word(false);
                    }

                    NamedKey::ArrowRight if word_jump => {
                        screen.move_selection_end_by_word(true);
                    }

                    NamedKey::ArrowLeft => {
                        screen.move_selection_end_by_cell(false);
                    }

                    NamedKey::ArrowRight => {
                        screen.move_selection_end_by_cell(true);
                    }

                    NamedKey::ArrowUp => {
                        screen.move_selection_end_by_row(false);
                    }

                    NamedKey::ArrowDown => {
                        screen.move_selection_end_by_row(true);
                    }

                    _ => unreachable!(),
                }
            }
        }

        self.request_redraw();
        true
    }

    fn send_key(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.tabs.is_empty() {
            return;
        }

        if self.handle_selection_key(event) {
            return;
        }
        let plain_arrow = !self.modifiers.shift_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.control_key()
            && !shortcut_modifier(self.modifiers);

        if plain_arrow {
            if matches!(&event.logical_key, Key::Named(NamedKey::ArrowLeft))
                && self.collapse_keyboard_selection_to_edge(false)
            {
                return;
            }

            if matches!(&event.logical_key, Key::Named(NamedKey::ArrowRight))
                && self.collapse_keyboard_selection_to_edge(true)
            {
                return;
            }
        }
        if let Some(chord) = key_chord(event, self.modifiers)
            && let Some(action) = self.keybindings.action(&chord)
        {
            self.dispatch_action(event_loop, action);
            return;
        }
        if self.handle_rename_key(event) || self.handle_search_key(event) {
            return;
        }
        if shortcut_modifier(self.modifiers) || self.active_tab().focused().shell_exited {
            return;
        }
        // Selection снимается только при реальном вводе текста
        // или при нажатии Enter.
        //
        // Стрелки, Option, Control, Command и navigation shortcuts
        // сами по себе selection не снимают.
        let is_text_input = matches!(&event.logical_key, Key::Character(_))
            && !self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !shortcut_modifier(self.modifiers);

        let is_enter = matches!(event.logical_key, Key::Named(NamedKey::Enter));

        if is_text_input {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .mark_input_start_at_cursor();
        }

        if is_enter {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .clear_input_start();
        }

        if is_text_input || is_enter {
            let had_selection = self
                .active_tab()
                .focused()
                .terminal
                .screen()
                .selection()
                .is_some();

            if had_selection {
                self.active_tab_mut()
                    .focused_mut()
                    .terminal
                    .screen_mut()
                    .clear_selection();

                self.selecting = None;
                self.selection_pending = None;
                self.last_left_click = None;

                self.request_redraw();
            }
        }
        let application_cursor = self.active_tab().focused().terminal.application_cursor();
        let bytes = match &event.logical_key {
            Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
            Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
            Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
            Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
            Key::Named(NamedKey::ArrowLeft)
                if self.modifiers.alt_key() || self.modifiers.control_key() =>
            {
                Some(b"\x1bb".to_vec())
            }

            Key::Named(NamedKey::ArrowRight)
                if self.modifiers.alt_key() || self.modifiers.control_key() =>
            {
                Some(b"\x1bf".to_vec())
            }
            Key::Named(NamedKey::ArrowUp) => Some(cursor_sequence(b'A', application_cursor)),
            Key::Named(NamedKey::ArrowDown) => Some(cursor_sequence(b'B', application_cursor)),
            Key::Named(NamedKey::ArrowRight) => Some(cursor_sequence(b'C', application_cursor)),
            Key::Named(NamedKey::ArrowLeft) => Some(cursor_sequence(b'D', application_cursor)),
            Key::Named(NamedKey::Home) => Some(b"\x1b[H".to_vec()),
            Key::Named(NamedKey::End) => Some(b"\x1b[F".to_vec()),
            Key::Named(NamedKey::Delete) => Some(b"\x1b[3~".to_vec()),
            Key::Named(NamedKey::PageUp) if self.modifiers.shift_key() => {
                let rows = self.active_tab().focused().terminal.screen().rows() as isize;
                self.active_tab_mut()
                    .focused_mut()
                    .terminal
                    .screen_mut()
                    .scroll_viewport(rows);
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::PageDown) if self.modifiers.shift_key() => {
                let rows = self.active_tab().focused().terminal.screen().rows() as isize;
                self.active_tab_mut()
                    .focused_mut()
                    .terminal
                    .screen_mut()
                    .scroll_viewport(-rows);
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::PageUp) => Some(b"\x1b[5~".to_vec()),
            Key::Named(NamedKey::PageDown) => Some(b"\x1b[6~".to_vec()),
            Key::Character(text) if self.modifiers.control_key() => text
                .chars()
                .next()
                .and_then(control_byte)
                .map(|byte| vec![byte]),
            _ => event.text.as_ref().map(|text| text.as_bytes().to_vec()),
        };
        if let Some(bytes) = bytes {
            let profile = self.profile_input;
            let pane = self.active_tab_mut().focused_mut();
            pane.terminal.screen_mut().scroll_to_bottom();
            if profile && pane.pending_input_at.is_none() {
                pane.pending_input_at = Some(Instant::now());
            }
            pane.write(bytes);
        }
    }

    fn dispatch_action(&mut self, event_loop: &ActiveEventLoop, action: Action) {
        if let Some(index) = action.tab_index() {
            self.switch_tab(index);
            return;
        }
        match action {
            Action::NewTab => {
                let directory = self.active_tab().source_directory().to_path_buf();
                self.queue_new_tab(directory);
            }
            Action::Close => {
                if self.close_focused_pane() {
                    self.pending_pinned_close = None;
                    self.update_window_title();
                    return;
                }

                let index = self.active_tab;

                if !self.confirm_pinned_tab_close(index) {
                    return;
                }

                if !self.close_tab_at(index) {
                    self.persist_workspace();
                    event_loop.exit();
                }
            }
            Action::CloseTab => {
                let index = self.active_tab;

                if !self.confirm_pinned_tab_close(index) {
                    return;
                }

                if !self.close_tab_at(index) {
                    self.persist_workspace();
                    event_loop.exit();
                }
            }
            Action::NextTab => self.switch_tab((self.active_tab + 1) % self.tabs.len()),
            Action::PreviousTab => {
                self.switch_tab((self.active_tab + self.tabs.len() - 1) % self.tabs.len())
            }
            Action::MoveTabLeft => {
                self.move_active_tab(-1);
            }
            Action::MoveTabRight => {
                self.move_active_tab(1);
            }
            Action::RenameTab => {
                self.rename_input = Some(self.active_tab().title.clone());
                self.update_window_title();
            }
            Action::DuplicateTab => {
                self.queue_duplicate_tab();
            }
            Action::TogglePinTab => {
                self.toggle_active_tab_pin();
            }
            Action::RestoreTab => {
                self.queue_restore_closed_tab();
            }
            Action::SplitHorizontal => self.split_focused(SplitAxis::Horizontal),
            Action::SplitVertical => self.split_focused(SplitAxis::Vertical),
            Action::ClosePane => {
                self.close_focused_pane();
            }
            Action::FocusLeft => self.focus_direction(Direction::Left),
            Action::FocusRight => self.focus_direction(Direction::Right),
            Action::FocusUp => self.focus_direction(Direction::Up),
            Action::FocusDown => self.focus_direction(Direction::Down),
            Action::SwapPaneLeft => self.swap_focused_pane(Direction::Left),
            Action::SwapPaneRight => self.swap_focused_pane(Direction::Right),
            Action::SwapPaneUp => self.swap_focused_pane(Direction::Up),
            Action::SwapPaneDown => self.swap_focused_pane(Direction::Down),
            Action::ResizeLeft => self.resize_direction(Direction::Left),
            Action::ResizeRight => self.resize_direction(Direction::Right),
            Action::ResizeUp => self.resize_direction(Direction::Up),
            Action::ResizeDown => self.resize_direction(Direction::Down),
            Action::TogglePaneZoom => {
                self.active_tab_mut().toggle_zoom();
                self.resize_active_panes();
                self.persist_workspace();
                self.request_redraw();
            }
            Action::Copy => self.copy_selection(),
            Action::Paste => self.paste_clipboard(),
            Action::Search => {
                let pane = self.active_tab_mut().focused_mut();
                pane.search_query = Some(String::new());
                pane.terminal.search_scrollback("");
                self.update_window_title();
                self.request_redraw();
            }
            Action::ThemeDark => self.apply_theme("dark"),
            Action::ThemeLight => self.apply_theme("light"),
            Action::SelectTab1
            | Action::SelectTab2
            | Action::SelectTab3
            | Action::SelectTab4
            | Action::SelectTab5
            | Action::SelectTab6
            | Action::SelectTab7
            | Action::SelectTab8
            | Action::SelectTab9 => unreachable!(),
        }
    }

    fn focus_direction(&mut self, direction: Direction) {
        let Some(rect) = self.renderer.as_ref().map(Renderer::content_rect) else {
            return;
        };
        let content = Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        };
        let next = {
            let tab = self.active_tab();
            tab.root
                .focus_in_direction(tab.focused_pane, direction, content)
        };
        if let Some(next) = next {
            self.active_tab_mut().focused_pane = next;
            self.update_window_title();
            self.persist_workspace();
            self.request_redraw();
        }
    }

    fn swap_focused_pane(&mut self, direction: Direction) {
        let Some(rect) = self.renderer.as_ref().map(Renderer::content_rect) else {
            return;
        };

        let content = Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        };

        let (focused, target) = {
            let tab = self.active_tab();
            let focused = tab.focused_pane;

            let Some(target) = tab.root.focus_in_direction(focused, direction, content) else {
                return;
            };

            (focused, target)
        };

        if self.active_tab_mut().root.swap_panes(focused, target) {
            self.resize_active_panes();
            self.persist_workspace();
            self.update_window_title();
            self.request_redraw();
        }
    }

    fn resize_direction(&mut self, direction: Direction) {
        let focused = self.active_tab().focused_pane;
        if self
            .active_tab_mut()
            .root
            .resize_toward(focused, direction, 0.05)
        {
            self.resize_active_panes();
            self.persist_workspace();
            self.request_redraw();
        }
    }

    fn apply_theme(&mut self, name: &str) {
        let (theme, diagnostics) = self.config.active_theme(Some(name));
        for diagnostic in diagnostics {
            eprintln!("theme: {diagnostic}");
        }
        self.theme_name = theme.name.clone();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_theme(render_theme(&theme));
        }
        self.persist_workspace();
        self.request_redraw();
    }

    fn handle_rename_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let Some(mut name) = self.rename_input.take() else {
            return false;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {}
            Key::Named(NamedKey::Enter) => {
                let name = name.trim();
                if !name.is_empty() {
                    let tab = self.active_tab_mut();
                    tab.title = name.to_owned();
                    tab.custom_title = true;
                    self.persist_workspace();
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::Backspace) => {
                name.pop();
                self.rename_input = Some(name);
            }
            _ => {
                if let Some(text) = &event.text {
                    name.push_str(text);
                }
                self.rename_input = Some(name);
            }
        }
        self.update_window_title();
        true
    }

    fn handle_search_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let Some(mut query) = self.active_tab_mut().focused_mut().search_query.take() else {
            return false;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.active_tab_mut()
                    .focused_mut()
                    .terminal
                    .search_scrollback("");
                self.update_window_title();
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::Enter) => {}
            Key::Named(NamedKey::Backspace) => {
                query.pop();
            }
            _ => {
                if let Some(text) = &event.text {
                    query.push_str(text);
                }
            }
        }
        let pane = self.active_tab_mut().focused_mut();
        pane.terminal.search_scrollback(&query);
        pane.search_query = Some(query);
        self.update_window_title();
        self.request_redraw();
        true
    }

    fn copy_selection(&mut self) {
        let Some(text) = self
            .active_tab()
            .focused()
            .terminal
            .screen()
            .selected_text()
        else {
            return;
        };
        if self.clipboard.is_none() {
            self.clipboard = Clipboard::new().ok();
        }
        if let Some(clipboard) = &mut self.clipboard
            && let Err(error) = clipboard.set_text(text)
        {
            eprintln!("clipboard copy failed: {error}");
        }
    }

    fn paste_clipboard(&mut self) {
        if self.clipboard.is_none() {
            self.clipboard = Clipboard::new().ok();
        }
        let text = self
            .clipboard
            .as_mut()
            .and_then(|clipboard| clipboard.get_text().ok());
        if let Some(text) = text {
            let pane = self.active_tab().focused();
            pane.write(pane.terminal.paste_bytes(&text));
        }
    }

    fn register_left_click(&mut self, pane: PaneId, position: PhysicalPosition<f64>) -> u8 {
        let now = Instant::now();

        let count = self
            .last_left_click
            .filter(|previous| {
                if previous.pane != pane
                    || now.duration_since(previous.when) > Duration::from_millis(400)
                {
                    return false;
                }

                let dx = position.x - previous.position.x;
                let dy = position.y - previous.position.y;

                dx * dx + dy * dy <= 36.0
            })
            .map_or(1, |previous| {
                if previous.count >= 3 {
                    1
                } else {
                    previous.count + 1
                }
            });

        self.last_left_click = Some(MouseClickState {
            when: now,
            pane,
            position,
            count,
        });

        count
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }

        if state == ElementState::Released {
            self.selecting = None;
            self.selection_pending = None;

            self.dragging_tab = None;
            self.resizing_split = None;

            let layout_changed = self.tab_drag_changed || self.split_resize_changed;

            self.tab_drag_changed = false;
            self.split_resize_changed = false;

            if layout_changed {
                self.persist_workspace();
            }

            return;
        }

        if self.tabs.is_empty() {
            return;
        }

        let Some(renderer) = &self.renderer else {
            return;
        };

        if renderer.new_tab_at(self.cursor_position, self.tabs.len()) {
            let directory = self.active_tab().source_directory().to_path_buf();
            self.queue_new_tab(directory);
            return;
        }

        if let Some(index) = renderer.tab_at(self.cursor_position, self.tabs.len()) {
            if renderer.tab_close_at(self.cursor_position, index, self.tabs.len()) {
                self.dragging_tab = None;

                if !self.confirm_pinned_tab_close(index) {
                    return;
                }

                self.close_tab_at(index);
            } else {
                self.switch_tab(index);

                self.dragging_tab = Some(index);
                self.tab_drag_changed = false;
            }

            return;
        }

        // Нажатие рядом с divider начинает resize panes.
        if self.active_tab().zoomed_pane.is_none() {
            let content = renderer.content_rect();

            let content_rect = Rect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: content.height,
            };

            let position = self.cursor_position;

            let handle = self.active_tab().root.split_handle_at(
                position.x as f32,
                position.y as f32,
                content_rect,
                6.0,
            );

            if let Some(handle) = handle {
                self.resizing_split = Some(PaneResizeDrag {
                    handle,
                    last_position: position,
                });

                self.split_resize_changed = false;
                self.dragging_tab = None;
                self.selecting = None;
                self.selection_pending = None;

                return;
            }
        }

        let Some((pane_id, rect)) = self.pane_at(self.cursor_position) else {
            return;
        };

        // Нажатие рядом с divider начинает resize panes.
        if self.active_tab().zoomed_pane.is_none() {
            let content = renderer.content_rect();

            let content_rect = Rect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: content.height,
            };

            let position = self.cursor_position;

            let handle = self.active_tab().root.split_handle_at(
                position.x as f32,
                position.y as f32,
                content_rect,
                6.0,
            );

            if let Some(handle) = handle {
                self.resizing_split = Some(PaneResizeDrag {
                    handle,
                    last_position: position,
                });

                self.split_resize_changed = false;
                self.dragging_tab = None;
                self.selecting = None;
                self.selection_pending = None;

                return;
            }
        }

        let position = self.cursor_position;

        let (row, column) = self
            .renderer
            .as_ref()
            .expect("renderer exists")
            .cell_at_in(viewport(rect), position);

        let (row, column, last_row, has_selection) = {
            let Some(pane) = self.active_tab().panes.get(&pane_id) else {
                return;
            };

            let screen = pane.terminal.screen();

            (
                row.min(screen.rows() - 1),
                column.min(screen.columns() - 1),
                last_interactive_row(screen),
                screen.selection().is_some(),
            )
        };

        // Pane всегда можно активировать кликом.
        self.active_tab_mut().focused_pane = pane_id;

        let Some(last_row) = last_row else {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .clear_selection();

            self.selecting = None;
            self.selection_pending = None;
            self.last_left_click = None;
            self.request_redraw();
            return;
        };

        // Ниже последнего prompt/output selection не существует.
        if row > last_row {
            if !self.modifiers.shift_key() {
                self.active_tab_mut()
                    .focused_mut()
                    .terminal
                    .screen_mut()
                    .clear_selection();
            }

            self.selecting = None;
            self.selection_pending = None;
            self.last_left_click = None;

            self.update_window_title();
            self.request_redraw();
            return;
        }

        if shortcut_modifier(self.modifiers) {
            self.selection_pending = None;
            self.last_left_click = None;

            let pane = self.active_tab_mut().focused_mut();

            if let Some(uri) = pane.terminal.screen().hyperlink_at(row, column)
                && let Err(error) = open::that(uri)
            {
                eprintln!("could not open hyperlink: {error}");
            }

            return;
        }

        // Shift + click изменяет конец существующего selection.
        if self.modifiers.shift_key() && has_selection {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .extend_selection_to(row.min(last_row), column);

            self.selecting = None;
            self.selection_pending = None;
            self.last_left_click = None;

            self.request_redraw();
            return;
        }

        let click_count = self.register_left_click(pane_id, position);

        // Triple click = строка.
        if click_count == 3 {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .select_line_at(row);

            self.selecting = None;
            self.selection_pending = None;
            self.last_left_click = None;

            self.request_redraw();
            return;
        }

        // Double click = слово.
        if click_count == 2 {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .select_word_at(row, column);

            self.selecting = None;
            self.selection_pending = None;

            self.request_redraw();
            return;
        }

        // Один click сам по себе ничего не выделяет.
        self.active_tab_mut()
            .focused_mut()
            .terminal
            .screen_mut()
            .clear_selection();

        self.selecting = None;

        self.selection_pending = Some(PendingSelection {
            pane: pane_id,
            row,
            column,
            position,
            rectangular: self.modifiers.alt_key(),
        });

        self.update_window_title();
        self.request_redraw();
    }

    fn update_mouse_cursor(&self, position: PhysicalPosition<f64>) {
        let Some(window) = &self.window else {
            return;
        };

        if self.tabs.is_empty() {
            window.set_cursor(CursorIcon::Default);
            return;
        }

        let Some(renderer) = &self.renderer else {
            return;
        };

        // Drag tab.
        if self.dragging_tab.is_some() {
            window.set_cursor(CursorIcon::Grabbing);
            return;
        }

        // Active pane resize.
        if let Some(drag) = &self.resizing_split {
            let icon = match drag.handle.axis() {
                SplitAxis::Vertical => CursorIcon::ColResize,
                SplitAxis::Horizontal => CursorIcon::RowResize,
            };

            window.set_cursor(icon);
            return;
        }

        // Tabs / close / plus.
        if renderer.new_tab_at(position, self.tabs.len())
            || renderer.tab_at(position, self.tabs.len()).is_some()
        {
            window.set_cursor(CursorIcon::Pointer);
            return;
        }

        // Hover over pane divider.
        if self.active_tab().zoomed_pane.is_none() {
            let content = renderer.content_rect();

            let rect = Rect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: content.height,
            };

            if let Some(handle) = self.active_tab().root.split_handle_at(
                position.x as f32,
                position.y as f32,
                rect,
                6.0,
            ) {
                let icon = match handle.axis() {
                    SplitAxis::Vertical => CursorIcon::ColResize,
                    SplitAxis::Horizontal => CursorIcon::RowResize,
                };

                window.set_cursor(icon);
                return;
            }
        }

        // Terminal body.
        if self.pane_at(position).is_some() {
            window.set_cursor(CursorIcon::Text);
            return;
        }

        window.set_cursor(CursorIcon::Default);
    }

    fn cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.cursor_position = position;
        self.update_mouse_cursor(position);

        let resize_drag = self
            .resizing_split
            .as_ref()
            .map(|drag| (drag.handle.clone(), drag.last_position));

        if let Some((handle, previous_position)) = resize_drag {
            let Some(content) = self
                .renderer
                .as_ref()
                .map(|renderer| renderer.content_rect())
            else {
                return;
            };

            let content_rect = Rect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: content.height,
            };

            let delta_x = (position.x - previous_position.x) as f32;
            let delta_y = (position.y - previous_position.y) as f32;

            let changed = self.active_tab_mut().root.resize_split_by_pixels(
                &handle,
                content_rect,
                delta_x,
                delta_y,
            );

            if let Some(drag) = &mut self.resizing_split {
                drag.last_position = position;
            }

            if changed {
                self.split_resize_changed = true;
                self.resize_active_panes();
                self.request_redraw();
            }

            return;
        }

        if let Some(source) = self.dragging_tab {
            let Some(target) = self
                .renderer
                .as_ref()
                .and_then(|renderer| renderer.tab_at(position, self.tabs.len()))
            else {
                return;
            };

            if source == target {
                return;
            }

            if source >= self.tabs.len() || target >= self.tabs.len() {
                return;
            }

            // Pinned tabs остаются только внутри pinned-группы.
            if self.tabs[source].pinned != self.tabs[target].pinned {
                return;
            }

            let tab = self.tabs.remove(source);
            self.tabs.insert(target, tab);

            self.active_tab = target;
            self.dragging_tab = Some(target);
            self.tab_drag_changed = true;
            self.pending_pinned_close = None;

            self.request_redraw();
            return;
        }

        // MouseDown уже был, но drag ещё не начался.
        if self.selecting.is_none()
            && let Some(pending) = self.selection_pending
        {
            let dx = position.x - pending.position.x;
            let dy = position.y - pending.position.y;

            // Маленькие движения мыши не являются selection.
            if dx * dx + dy * dy < 16.0 {
                return;
            }

            let Some((_, rect)) = self
                .active_layout()
                .into_iter()
                .find(|(pane, _)| *pane == pending.pane)
            else {
                self.selection_pending = None;
                return;
            };

            let (row, column) = self
                .renderer
                .as_ref()
                .expect("renderer exists")
                .cell_at_in(viewport(rect), position);

            let (row, column, last_row) = {
                let Some(pane) = self.active_tab().panes.get(&pending.pane) else {
                    self.selection_pending = None;
                    return;
                };

                let screen = pane.terminal.screen();

                (
                    row.min(screen.rows() - 1),
                    column.min(screen.columns() - 1),
                    last_interactive_row(screen),
                )
            };

            let Some(last_row) = last_row else {
                self.selection_pending = None;
                return;
            };

            let row = row.min(last_row);

            let Some(pane) = self.active_tab_mut().panes.get_mut(&pending.pane) else {
                self.selection_pending = None;
                return;
            };

            if pending.rectangular {
                pane.terminal
                    .screen_mut()
                    .begin_rectangular_selection(pending.row, pending.column);
            } else {
                pane.terminal
                    .screen_mut()
                    .begin_selection(pending.row, pending.column);
            }

            pane.terminal.screen_mut().update_selection(row, column);

            self.selecting = Some(pending.pane);
            self.selection_pending = None;

            // Drag не должен потом превратиться в double-click.
            self.last_left_click = None;

            self.request_redraw();
            return;
        }

        let Some(selecting) = self.selecting else {
            return;
        };

        let Some((_, rect)) = self
            .active_layout()
            .into_iter()
            .find(|(pane, _)| *pane == selecting)
        else {
            return;
        };

        let (row, column) = self
            .renderer
            .as_ref()
            .expect("renderer exists")
            .cell_at_in(viewport(rect), position);

        let (row, column, last_row) = {
            let Some(pane) = self.active_tab().panes.get(&selecting) else {
                return;
            };

            let screen = pane.terminal.screen();

            (
                row.min(screen.rows() - 1),
                column.min(screen.columns() - 1),
                last_interactive_row(screen),
            )
        };

        let Some(last_row) = last_row else {
            return;
        };

        // Главное правило:
        // ниже последнего prompt/output selection никогда не идёт.
        let row = row.min(last_row);

        let Some(pane) = self.active_tab_mut().panes.get_mut(&selecting) else {
            return;
        };

        pane.terminal.screen_mut().update_selection(row, column);

        self.request_redraw();
    }

    fn pane_at(&self, position: PhysicalPosition<f64>) -> Option<(PaneId, Rect)> {
        self.active_layout()
            .into_iter()
            .find(|(_, rect)| rect.contains(position.x as f32, position.y as f32))
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        if self.tabs.is_empty() {
            return;
        }
        // Если mouse находится над tab bar,
        // wheel/trackpad scroll'ит tabs, а не terminal.
        if self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.tab_bar_at(self.cursor_position))
        {
            let amount = match delta {
                MouseScrollDelta::LineDelta(x, y) => {
                    let axis = if x.abs() > y.abs() {
                        x as f64
                    } else {
                        y as f64
                    };

                    -axis * 48.0
                }

                MouseScrollDelta::PixelDelta(position) => {
                    let axis = if position.x.abs() > position.y.abs() {
                        position.x
                    } else {
                        position.y
                    };

                    -axis
                }
            };

            let count = self.tabs.len();

            let changed = if let Some(renderer) = &mut self.renderer {
                renderer.scroll_tabs(amount, count)
            } else {
                false
            };

            if changed {
                self.request_redraw();
            }

            return;
        }
        if let Some((pane, _)) = self.pane_at(self.cursor_position) {
            self.active_tab_mut().focused_pane = pane;
        }
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y.round() as isize * 3,
            MouseScrollDelta::PixelDelta(position) => {
                (position.y / terminal_renderer::CELL_HEIGHT).round() as isize
            }
        };
        if lines != 0 {
            self.active_tab_mut()
                .focused_mut()
                .terminal
                .screen_mut()
                .scroll_viewport(lines);
            self.request_redraw();
        }
    }

    fn render(&mut self, event_loop: &ActiveEventLoop) {
        if self.tabs.is_empty() {
            let result = self
                .renderer
                .as_mut()
                .expect("render requested after initialization")
                .render_panes(&[], &[]);
            if let Err(error) = result {
                self.fail(event_loop, error);
            }
            return;
        }
        let labels: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| TabLabel {
                title: if tab.pinned {
                    format!("📌 {}", tab.title)
                } else {
                    tab.title.clone()
                },
                active: index == self.active_tab,
            })
            .collect();
        let layout = self.active_layout();
        let active = self.active_tab;
        let focused = self.tabs[active].focused_pane;
        let result = {
            let tab = &self.tabs[active];
            let panes: Vec<_> = layout
                .iter()
                .filter_map(|(id, rect)| {
                    tab.panes.get(id).map(|pane| PaneView {
                        id: id.0,
                        screen: pane.terminal.screen(),
                        rect: viewport(*rect),
                        focused: *id == focused,
                    })
                })
                .collect();
            self.renderer
                .as_mut()
                .expect("render requested after initialization")
                .render_panes(&panes, &labels)
        };
        if let Err(error) = result {
            self.fail(event_loop, error);
            return;
        }
        if !self.first_frame_seen {
            self.first_frame_seen = true;
            timing(self.started_at, "first frame rendered");
        }
        if let Some(started_at) = self.active_tab_mut().focused_mut().pending_input_at.take() {
            eprintln!(
                "input-to-frame: {:.2} ms",
                started_at.elapsed().as_secs_f64() * 1_000.0
            );
        }
    }

    fn update_window_title(&self) {
        let Some(window) = &self.window else { return };
        if self.tabs.is_empty() {
            window.set_title("Grin — starting shell…");
            return;
        }
        if let Some(index) = self.pending_pinned_close
            && let Some(tab) = self.tabs.get(index)
        {
            window.set_title(&format!(
                "⚠ Close pinned tab \"{}\"? Close it again to confirm",
                tab.title
            ));
            return;
        }
        if let Some(name) = &self.rename_input {
            window.set_title(&format!("Rename tab: {name}"));
        } else if let Some(query) = &self.active_tab().focused().search_query {
            window.set_title(&format!("Find: {query}"));
        } else {
            let suffix = if self.active_tab().focused().shell_exited {
                " — shell exited"
            } else {
                ""
            };
            window.set_title(&format!("Grin — {}{suffix}", self.active_tab().title));
        }
    }

    fn persist_workspace(&self) {
        if !self.config.workspace.restore || self.tabs.is_empty() {
            return;
        }
        let (size, scale_factor) = self
            .window
            .as_ref()
            .map(|window| (window.inner_size(), window.scale_factor()))
            .unwrap_or_default();
        let workspace = Workspace {
            version: WORKSPACE_VERSION,
            window_width: (f64::from(size.width) / scale_factor.max(1.0))
                .round()
                .max(320.0) as u32,
            window_height: (f64::from(size.height) / scale_factor.max(1.0))
                .round()
                .max(180.0) as u32,
            active_tab: self.active_tab,
            theme: self.theme_name.clone(),
            tabs: self.tabs.iter().map(TabSession::snapshot).collect(),
        };
        if let Err(error) = workspace.save_atomic(&self.workspace_path) {
            eprintln!(
                "could not save workspace {}: {error}",
                self.workspace_path.display()
            );
        }
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.initialize(event_loop)
        {
            self.fail(event_loop, error);
        }
    }
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyReady => self.process_pty_events(),
            UserEvent::InitialReady(result) => self.complete_initial_sessions(result),
            UserEvent::PaneSpawned { target, result } => self.complete_pane_spawn(target, result),
            UserEvent::TabRestored(result) => self.complete_tab_restore(result),
        }
    }
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.persist_workspace();
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.resize(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => self.send_key(event_loop, &event),
            WindowEvent::CursorMoved { position, .. } => self.cursor_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.mouse_wheel(delta),
            WindowEvent::RedrawRequested => self.render(event_loop),
            _ => {}
        }
    }
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.persist_workspace();
    }
}

pub fn run() -> Result<()> {
    let started_at = Instant::now();
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("could not initialize native event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut application = Application::new(event_loop.create_proxy(), started_at);
    event_loop
        .run_app(&mut application)
        .context("native event loop failed")?;
    if let Some(error) = application.fatal_error {
        return Err(error);
    }
    Ok(())
}

fn build_initial_sessions(
    workspace: Option<Workspace>,
    cursor_shape: terminal_core::CursorShape,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<InitialSessions> {
    if let Some(workspace) = workspace {
        match restore_initial_sessions(workspace, cursor_shape, proxy.clone()) {
            Ok(initial) => return Ok(initial),
            Err(error) => {
                let mut initial = fresh_initial_session(cursor_shape, proxy)?;
                initial.diagnostic = Some(format!("restore failed: {error}; started fresh"));
                return Ok(initial);
            }
        }
    }
    fresh_initial_session(cursor_shape, proxy)
}

fn fresh_initial_session(
    cursor_shape: terminal_core::CursorShape,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<InitialSessions> {
    let pane = spawn_runtime(
        PaneId(1),
        80,
        24,
        default_working_directory(),
        cursor_shape,
        proxy,
    )?;
    Ok(InitialSessions {
        tabs: vec![TabSession::new("Terminal 1".into(), pane)],
        active_tab: 0,
        next_pane_id: 2,
        diagnostic: None,
    })
}

fn restore_tab_session(
    state: TabState,
    cursor_shape: terminal_core::CursorShape,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<TabSession> {
    let tree_ids: HashSet<_> = state.root.panes().into_iter().collect();
    let state_ids: HashSet<_> = state.panes.iter().map(|pane| pane.id).collect();

    if tree_ids != state_ids || !tree_ids.contains(&state.focused_pane) {
        anyhow::bail!("tab has inconsistent pane identities");
    }

    let mut panes = HashMap::new();

    for pane in state.panes {
        let runtime = spawn_runtime(
            pane.id,
            80,
            24,
            pane.working_directory,
            cursor_shape,
            proxy.clone(),
        )?;

        panes.insert(pane.id, runtime);
    }

    Ok(TabSession {
        title: state.title,
        pinned: state.pinned,
        root: state.root,
        focused_pane: state.focused_pane,
        zoomed_pane: state.zoomed_pane,
        panes,
        custom_title: true,
    })
}

fn restore_initial_sessions(
    workspace: Workspace,
    cursor_shape: terminal_core::CursorShape,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<InitialSessions> {
    if workspace.tabs.is_empty() {
        anyhow::bail!("workspace has no tabs");
    }
    let active_tab = workspace.active_tab.min(workspace.tabs.len() - 1);
    let mut restored_tabs = Vec::new();
    let mut maximum_id = 0;
    for state in workspace.tabs {
        for pane in &state.panes {
            maximum_id = maximum_id.max(pane.id.0);
        }

        restored_tabs.push(restore_tab_session(state, cursor_shape, proxy.clone())?);
    }

    let active_pane = restored_tabs.get(active_tab).map(|tab| tab.focused_pane);

    restored_tabs.sort_by_key(|tab| !tab.pinned);

    let active_tab = active_pane
        .and_then(|pane| {
            restored_tabs
                .iter()
                .position(|tab| tab.focused_pane == pane)
        })
        .unwrap_or(0);

    Ok(InitialSessions {
        tabs: restored_tabs,
        active_tab,
        next_pane_id: maximum_id.saturating_add(1).max(1),
        diagnostic: None,
    })
}

fn spawn_runtime(
    id: PaneId,
    columns: u16,
    rows: u16,
    working_directory: PathBuf,
    cursor_shape: terminal_core::CursorShape,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<TerminalPane> {
    let wake_proxy = proxy;
    TerminalPane::spawn(
        id,
        columns,
        rows,
        working_directory,
        cursor_shape,
        move || {
            let _ = wake_proxy.send_event(UserEvent::PtyReady);
        },
    )
}

fn last_interactive_row(screen: &terminal_core::Screen) -> Option<usize> {
    let last_content_row = (0..screen.rows()).rev().find(|&row| {
        screen
            .display_line(row)
            .is_some_and(|line| line.iter().any(|cell| !cell.is_blank()))
    });

    let cursor_row = screen
        .display_cursor()
        .filter(|cursor| cursor.visible)
        .map(|cursor| cursor.row);

    match (last_content_row, cursor_row) {
        (Some(content), Some(cursor)) => Some(content.max(cursor)),
        (Some(content), None) => Some(content),
        (None, Some(cursor)) => Some(cursor),
        (None, None) => None,
    }
}

fn viewport(rect: Rect) -> ViewportRect {
    ViewportRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}
fn render_theme(theme: &ThemePalette) -> RenderTheme {
    RenderTheme {
        foreground: theme.foreground,
        background: theme.background,
        ansi: theme.ansi,
        cursor: theme.cursor,
        selection_foreground: theme.selection_foreground,
        selection_background: theme.selection_background,
        tab_bar: theme.tab_bar,
        inactive_tab: theme.inactive_tab,
        active_tab: theme.active_tab,
        pane_border: theme.pane_border,
    }
}
fn default_working_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn key_chord(event: &winit::event::KeyEvent, modifiers: ModifiersState) -> Option<KeyChord> {
    let mut key = match &event.logical_key {
        Key::Character(text) => text.to_ascii_lowercase(),
        Key::Named(NamedKey::ArrowLeft) => "left".into(),
        Key::Named(NamedKey::ArrowRight) => "right".into(),
        Key::Named(NamedKey::ArrowUp) => "up".into(),
        Key::Named(NamedKey::ArrowDown) => "down".into(),
        Key::Named(NamedKey::Enter) => "enter".into(),
        _ => return None,
    };
    if key == "{" {
        key = "[".into();
    }
    if key == "}" {
        key = "]".into();
    }
    Some(KeyChord {
        command: shortcut_modifier(modifiers),
        control: cfg!(target_os = "macos") && modifiers.control_key(),
        alt: modifiers.alt_key(),
        shift: modifiers.shift_key(),
        key,
    })
}
fn cursor_sequence(final_byte: u8, application_mode: bool) -> Vec<u8> {
    vec![0x1b, if application_mode { b'O' } else { b'[' }, final_byte]
}
fn shortcut_modifier(modifiers: ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.super_key()
    } else {
        modifiers.control_key()
    }
}
fn control_byte(character: char) -> Option<u8> {
    let upper = character.to_ascii_uppercase();
    if upper.is_ascii_alphabetic() {
        Some((upper as u8) - b'@')
    } else {
        match character {
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' => Some(0x1f),
            _ => None,
        }
    }
}
fn timing(start: Instant, milestone: &str) {
    if cfg!(debug_assertions) || std::env::var_os("GRIN_PROFILE_STARTUP").is_some() {
        eprintln!(
            "startup +{:>5} ms: {milestone}",
            start.elapsed().as_millis()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_control_and_application_cursor_keys() {
        assert_eq!(control_byte('c'), Some(3));
        assert_eq!(control_byte('Z'), Some(26));
        assert_eq!(control_byte('['), Some(0x1b));
        assert_eq!(control_byte('1'), None);
        assert_eq!(cursor_sequence(b'A', false), b"\x1b[A");
        assert_eq!(cursor_sequence(b'A', true), b"\x1bOA");
    }
}
