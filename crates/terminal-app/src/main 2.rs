mod action;
mod config;
mod layout;
mod session;
mod workspace;

use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result, anyhow};
use arboard::Clipboard;
use terminal_core::Terminal;
use terminal_pty::{PtyEvent, PtySession};
use terminal_renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

#[derive(Debug)]
enum UserEvent {
    PtyReady,
}

struct Application {
    started_at: Instant,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    terminal: Terminal,
    pty: Option<PtySession>,
    clipboard: Option<Clipboard>,
    modifiers: ModifiersState,
    cursor_position: PhysicalPosition<f64>,
    selecting: bool,
    search_query: Option<String>,
    last_title_generation: u64,
    last_rendered_generation: u64,
    first_shell_output_seen: bool,
    first_frame_seen: bool,
    profile_input: bool,
    pending_input_at: Option<Instant>,
    shell_exited: bool,
    fatal_error: Option<anyhow::Error>,
}

impl Application {
    fn new(proxy: EventLoopProxy<UserEvent>, started_at: Instant) -> Self {
        Self {
            started_at,
            proxy,
            window: None,
            renderer: None,
            terminal: Terminal::new(80, 24),
            pty: None,
            clipboard: None,
            modifiers: ModifiersState::empty(),
            cursor_position: PhysicalPosition::new(0.0, 0.0),
            selecting: false,
            search_query: None,
            last_title_generation: 0,
            last_rendered_generation: 0,
            first_shell_output_seen: false,
            first_frame_seen: false,
            profile_input: std::env::var_os("GRIN_PROFILE_INPUT").is_some(),
            pending_input_at: None,
            shell_exited: false,
            fatal_error: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Grin Terminal")
                        .with_inner_size(LogicalSize::new(960.0, 600.0))
                        .with_min_inner_size(LogicalSize::new(320.0, 180.0)),
                )
                .context("could not create native window")?,
        );
        window.set_ime_allowed(true);
        timing(self.started_at, "window created");

        let renderer = pollster::block_on(Renderer::new(Arc::clone(&window)))
            .context("GPU renderer initialization failed")?;
        timing(self.started_at, "GPU and font renderer initialized");
        let (columns, rows) = renderer.grid_size();
        self.terminal.resize(columns as usize, rows as usize);

        let proxy = self.proxy.clone();
        let pty = PtySession::spawn(columns, rows, move || {
            let _ = proxy.send_event(UserEvent::PtyReady);
        })
        .context("default shell startup failed")?;
        timing(self.started_at, "PTY and shell spawned");

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.pty = Some(pty);
        self.request_redraw();
        Ok(())
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

    fn process_pty_events(&mut self, event_loop: &ActiveEventLoop) {
        let events = match &self.pty {
            Some(pty) => pty.drain_events(),
            None => return,
        };
        for event in events {
            match event {
                PtyEvent::Output(bytes) => {
                    if !self.first_shell_output_seen {
                        self.first_shell_output_seen = true;
                        timing(self.started_at, "first shell output");
                    }
                    self.terminal.process(&bytes);
                    let response = self.terminal.take_response();
                    if !response.is_empty() {
                        self.write_pty(response);
                    }
                }
                PtyEvent::Exited { success } => {
                    self.shell_exited = true;
                    self.pty = None;
                    let status = if success { "0" } else { "non-zero" };
                    if let Some(window) = &self.window {
                        window.set_title(&format!("Grin Terminal — shell exited ({status})"));
                    }
                    self.request_redraw();
                }
                PtyEvent::Error(message) => {
                    self.fail(event_loop, anyhow!(message));
                    return;
                }
            }
        }
        if self.terminal.title_generation() != self.last_title_generation {
            self.last_title_generation = self.terminal.title_generation();
            self.update_window_title();
        }
        if self.terminal.screen().generation() != self.last_rendered_generation {
            self.request_redraw();
        }
    }

    fn resize(&mut self) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let previous_grid = renderer.grid_size();
        renderer.resize(window.inner_size(), window.scale_factor());
        let (columns, rows) = renderer.grid_size();
        if (columns, rows) == previous_grid {
            self.request_redraw();
            return;
        }
        self.terminal.resize(columns as usize, rows as usize);
        if let Some(pty) = &self.pty {
            if let Err(error) = pty.resize(columns, rows) {
                eprintln!("could not queue PTY resize: {error}");
            }
        }
        self.request_redraw();
    }

    fn write_pty(&self, bytes: impl Into<Vec<u8>>) {
        if let Some(pty) = &self.pty {
            if let Err(error) = pty.write(bytes) {
                eprintln!("could not queue terminal input: {error}");
            }
        }
    }

    fn send_key(&mut self, event: &winit::event::KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.handle_shortcut(event) || self.handle_search_key(event) {
            return;
        }
        if self.shell_exited {
            return;
        }
        let application_cursor = self.terminal.application_cursor();
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
                let rows = self.terminal.screen().rows() as isize;
                self.terminal.screen_mut().scroll_viewport(rows);
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::PageDown) if self.modifiers.shift_key() => {
                let rows = self.terminal.screen().rows() as isize;
                self.terminal.screen_mut().scroll_viewport(-rows);
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
            self.terminal.screen_mut().scroll_to_bottom();
            if self.profile_input && self.pending_input_at.is_none() {
                self.pending_input_at = Some(Instant::now());
            }
            self.write_pty(bytes);
        }
    }

    fn handle_shortcut(&mut self, event: &winit::event::KeyEvent) -> bool {
        if !shortcut_modifier(self.modifiers) {
            return false;
        }
        let Key::Character(text) = &event.logical_key else {
            return false;
        };
        match text.to_ascii_lowercase().as_str() {
            "c" if self.terminal.screen().selection().is_some() => {
                if let Some(text) = self.terminal.screen().selected_text() {
                    self.set_clipboard(text);
                }
                true
            }
            "v" => {
                self.paste_clipboard();
                true
            }
            "f" => {
                self.search_query = Some(String::new());
                self.terminal.search_scrollback("");
                self.update_window_title();
                self.request_redraw();
                true
            }
            _ => false,
        }
    }

    fn handle_search_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let Some(mut query) = self.search_query.take() else {
            return false;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.terminal.search_scrollback("");
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
        self.terminal.search_scrollback(&query);
        self.search_query = Some(query);
        self.update_window_title();
        self.request_redraw();
        true
    }

    fn set_clipboard(&mut self, text: String) {
        if self.clipboard.is_none() {
            self.clipboard = Clipboard::new()
                .map_err(|error| {
                    eprintln!("clipboard is unavailable: {error}");
                    error
                })
                .ok();
        }
        if let Some(clipboard) = &mut self.clipboard {
            if let Err(error) = clipboard.set_text(text) {
                eprintln!("clipboard copy failed: {error}");
            }
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
            self.write_pty(self.terminal.paste_bytes(&text));
        }
    }

    fn mouse_cell(&self) -> Option<(usize, usize)> {
        let renderer = self.renderer.as_ref()?;
        let (row, column) = renderer.cell_at(self.cursor_position);
        Some((
            row.min(self.terminal.screen().rows() - 1),
            column.min(self.terminal.screen().columns() - 1),
        ))
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if state == ElementState::Released {
            self.selecting = false;
            return;
        }
        let Some((row, column)) = self.mouse_cell() else {
            return;
        };
        if shortcut_modifier(self.modifiers) {
            if let Some(uri) = self.terminal.screen().hyperlink_at(row, column) {
                if let Err(error) = open::that(uri) {
                    eprintln!("could not open hyperlink: {error}");
                }
                return;
            }
        }
        self.selecting = true;
        self.terminal.screen_mut().begin_selection(row, column);
        self.request_redraw();
    }

    fn cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.cursor_position = position;
        if self.selecting {
            if let Some((row, column)) = self.mouse_cell() {
                self.terminal.screen_mut().update_selection(row, column);
                self.request_redraw();
            }
        }
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y.round() as isize * 3,
            MouseScrollDelta::PixelDelta(position) => {
                (position.y / terminal_renderer::CELL_HEIGHT).round() as isize
            }
        };
        if lines != 0 {
            self.terminal.screen_mut().scroll_viewport(lines);
            self.request_redraw();
        }
    }

    fn update_window_title(&self) {
        let Some(window) = &self.window else { return };
        if let Some(query) = &self.search_query {
            window.set_title(&format!("Find: {query}"));
        } else if self.shell_exited {
            window.set_title("Grin Terminal — shell exited");
        } else if self.terminal.title().is_empty() {
            window.set_title("Grin Terminal");
        } else {
            window.set_title(self.terminal.title());
        }
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(error) = self.initialize(event_loop) {
                self.fail(event_loop, error);
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyReady => self.process_pty_events(event_loop),
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.resize(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => self.send_key(&event),
            WindowEvent::CursorMoved { position, .. } => self.cursor_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.mouse_wheel(delta),
            WindowEvent::RedrawRequested => {
                let Some(renderer) = &mut self.renderer else {
                    return;
                };
                if let Err(error) = renderer.render(self.terminal.screen()) {
                    self.fail(event_loop, error);
                    return;
                }
                self.last_rendered_generation = self.terminal.screen().generation();
                if !self.first_frame_seen {
                    self.first_frame_seen = true;
                    timing(self.started_at, "first frame rendered");
                }
                if let Some(started_at) = self.pending_input_at.take() {
                    eprintln!(
                        "input-to-frame: {:.2} ms",
                        started_at.elapsed().as_secs_f64() * 1_000.0
                    );
                }
            }
            _ => {}
        }
    }
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

fn main() -> Result<()> {
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
