use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Instant,
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
    window::{Window, WindowId},
};

use crate::{
    action::{Action, KeyBindings, KeyChord},
    config::{AppConfig, ThemePalette, workspace_path},
    layout::{Direction, PaneId, Rect, SplitAxis},
    session::{TabSession, TerminalPane},
    workspace::{WORKSPACE_VERSION, Workspace},
};

enum UserEvent {
    PtyReady,
    InitialReady(Result<InitialSessions, String>),
    PaneSpawned {
        target: SpawnTarget,
        result: Result<TerminalPane, String>,
    },
}

enum SpawnTarget {
    NewTab,
    Split { source: PaneId, axis: SplitAxis },
}

struct InitialSessions {
    tabs: Vec<TabSession>,
    active_tab: usize,
    next_pane_id: u64,
    diagnostic: Option<String>,
}

pub struct Application {
    started_at: Instant,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    tabs: Vec<TabSession>,
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
    rename_input: Option<String>,
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
            rename_input: None,
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
        self.queue_pane_spawn(SpawnTarget::NewTab, columns, rows, working_directory);
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
            SpawnTarget::NewTab => {
                self.tabs.push(TabSession::new(
                    format!("Terminal {}", self.tabs.len() + 1),
                    pane,
                ));
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
        let closed = self.tabs.remove(index);
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
        self.active_tab = index;
        self.selecting = None;
        self.resize_active_panes();
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

    fn send_key(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.tabs.is_empty() {
            return;
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
        let application_cursor = self.active_tab().focused().terminal.application_cursor();
        let bytes = match &event.logical_key {
            Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
            Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
            Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
            Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
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
                self.queue_new_tab(default_working_directory());
            }
            Action::Close => {
                if !self.close_focused_pane() && !self.close_tab_at(self.active_tab) {
                    self.persist_workspace();
                    event_loop.exit();
                }
            }
            Action::CloseTab => {
                if !self.close_tab_at(self.active_tab) {
                    self.persist_workspace();
                    event_loop.exit();
                }
            }
            Action::NextTab => self.switch_tab((self.active_tab + 1) % self.tabs.len()),
            Action::PreviousTab => {
                self.switch_tab((self.active_tab + self.tabs.len() - 1) % self.tabs.len())
            }
            Action::RenameTab => {
                self.rename_input = Some(self.active_tab().title.clone());
                self.update_window_title();
            }
            Action::DuplicateTab => {
                let directory = self.active_tab().source_directory().to_path_buf();
                self.queue_new_tab(directory);
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

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if state == ElementState::Released {
            self.selecting = None;
            return;
        }
        if self.tabs.is_empty() {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        if renderer.new_tab_at(self.cursor_position, self.tabs.len()) {
            self.queue_new_tab(default_working_directory());
            return;
        }
        if let Some(index) = renderer.tab_at(self.cursor_position, self.tabs.len()) {
            if renderer.tab_close_at(self.cursor_position, index, self.tabs.len()) {
                self.close_tab_at(index);
            } else {
                self.switch_tab(index);
            }
            return;
        }
        let Some((pane_id, rect)) = self.pane_at(self.cursor_position) else {
            return;
        };
        self.active_tab_mut().focused_pane = pane_id;
        let (row, column) = self
            .renderer
            .as_ref()
            .expect("renderer exists")
            .cell_at_in(viewport(rect), self.cursor_position);
        let shortcut = shortcut_modifier(self.modifiers);
        let pane = self.active_tab_mut().focused_mut();
        let row = row.min(pane.terminal.screen().rows() - 1);
        let column = column.min(pane.terminal.screen().columns() - 1);
        if shortcut {
            if let Some(uri) = pane.terminal.screen().hyperlink_at(row, column)
                && let Err(error) = open::that(uri)
            {
                eprintln!("could not open hyperlink: {error}");
            }
            return;
        }
        pane.terminal.screen_mut().begin_selection(row, column);
        self.selecting = Some(pane_id);
        self.update_window_title();
        self.request_redraw();
    }

    fn cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.cursor_position = position;
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
        let Some(pane) = self.active_tab_mut().panes.get_mut(&selecting) else {
            return;
        };
        let row = row.min(pane.terminal.screen().rows() - 1);
        let column = column.min(pane.terminal.screen().columns() - 1);
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
                title: tab.title.clone(),
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
        let tree_ids: HashSet<_> = state.root.panes().into_iter().collect();
        let state_ids: HashSet<_> = state.panes.iter().map(|pane| pane.id).collect();
        if tree_ids != state_ids || !tree_ids.contains(&state.focused_pane) {
            anyhow::bail!("workspace tab has inconsistent pane identities");
        }
        let mut panes = HashMap::new();
        for pane in state.panes {
            maximum_id = maximum_id.max(pane.id.0);
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
        restored_tabs.push(TabSession {
            title: state.title,
            root: state.root,
            focused_pane: state.focused_pane,
            zoomed_pane: state.zoomed_pane,
            panes,
            custom_title: true,
        });
    }
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
