use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use terminal_core::{CursorShape, Terminal};
use terminal_pty::{PtyEvent, PtySession};

use crate::{
    layout::{PaneId, PaneTree, Rect, SplitAxis},
    workspace::{PaneState, TabState},
};

pub struct TerminalPane {
    pub id: PaneId,
    pub terminal: Terminal,
    pub pty: Option<PtySession>,
    pub working_directory: PathBuf,
    pub shell_exited: bool,
    pub search_query: Option<String>,
    pub pending_input_at: Option<std::time::Instant>,
    last_title_generation: u64,
}

pub struct PaneUpdate {
    pub changed: bool,
    pub title_changed: bool,
    pub error: Option<String>,
}

impl TerminalPane {
    pub fn spawn(
        id: PaneId,
        columns: u16,
        rows: u16,
        working_directory: PathBuf,
        cursor_shape: CursorShape,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut terminal = Terminal::new(columns as usize, rows as usize);
        terminal.set_cursor_style(cursor_shape);
        let pty = PtySession::spawn_in(columns, rows, Some(working_directory.clone()), wake)
            .with_context(|| format!("could not start shell in {}", working_directory.display()))?;
        Ok(Self {
            id,
            terminal,
            pty: Some(pty),
            working_directory,
            shell_exited: false,
            search_query: None,
            pending_input_at: None,
            last_title_generation: 0,
        })
    }

    pub fn process_pty_events(&mut self) -> PaneUpdate {
        let events = self
            .pty
            .as_ref()
            .map(PtySession::drain_events)
            .unwrap_or_default();
        let mut changed = false;
        let mut error = None;
        for event in events {
            match event {
                PtyEvent::Output(bytes) => {
                    self.terminal.process(&bytes);
                    if let Some(directory) = self
                        .terminal
                        .working_directory_uri()
                        .and_then(directory_from_osc7)
                        .filter(|directory| directory.is_dir())
                    {
                        self.working_directory = directory;
                    }
                    let response = self.terminal.take_response();
                    if !response.is_empty() {
                        self.write(response);
                    }
                    changed = true;
                }
                PtyEvent::Exited { .. } => {
                    self.shell_exited = true;
                    self.pty = None;
                    changed = true;
                }
                PtyEvent::Error(message) => {
                    self.shell_exited = true;
                    self.pty = None;
                    error = Some(message);
                    changed = true;
                }
            }
        }
        let title_changed = self.terminal.title_generation() != self.last_title_generation;
        if title_changed {
            self.last_title_generation = self.terminal.title_generation();
        }
        PaneUpdate {
            changed,
            title_changed,
            error,
        }
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        if let Some(pty) = &self.pty
            && let Err(error) = pty.write(bytes)
        {
            eprintln!(
                "could not queue terminal input for pane {}: {error}",
                self.id.0
            );
        }
    }

    pub fn resize(&mut self, columns: u16, rows: u16) {
        if self.terminal.screen().columns() == columns as usize
            && self.terminal.screen().rows() == rows as usize
        {
            return;
        }
        self.terminal.resize(columns as usize, rows as usize);
        if let Some(pty) = &self.pty
            && let Err(error) = pty.resize(columns, rows)
        {
            eprintln!("could not resize pane {} PTY: {error}", self.id.0);
        }
    }
}

fn directory_from_osc7(uri: &str) -> Option<PathBuf> {
    let authority_and_path = uri.strip_prefix("file://")?;
    let path = authority_and_path
        .find('/')
        .map(|index| &authority_and_path[index..])?;
    let mut decoded = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

pub struct TabSession {
    pub title: String,
    pub root: PaneTree,
    pub focused_pane: PaneId,
    pub zoomed_pane: Option<PaneId>,
    pub panes: HashMap<PaneId, TerminalPane>,
    pub custom_title: bool,
}

impl TabSession {
    pub fn new(title: String, pane: TerminalPane) -> Self {
        let id = pane.id;
        Self {
            title,
            root: PaneTree::leaf(id),
            focused_pane: id,
            zoomed_pane: None,
            panes: HashMap::from([(id, pane)]),
            custom_title: false,
        }
    }

    pub fn snapshot(&self) -> TabState {
        let mut panes: Vec<_> = self
            .panes
            .values()
            .map(|pane| PaneState {
                id: pane.id,
                working_directory: pane.working_directory.clone(),
            })
            .collect();
        panes.sort_by_key(|pane| pane.id.0);
        TabState {
            title: self.title.clone(),
            root: self.root.clone(),
            focused_pane: self.focused_pane,
            zoomed_pane: self.zoomed_pane,
            panes,
        }
    }

    pub fn focused(&self) -> &TerminalPane {
        self.panes
            .get(&self.focused_pane)
            .expect("pane tree focus must reference a live pane")
    }

    pub fn focused_mut(&mut self) -> &mut TerminalPane {
        self.panes
            .get_mut(&self.focused_pane)
            .expect("pane tree focus must reference a live pane")
    }

    pub fn insert_split_at(
        &mut self,
        target: PaneId,
        pane: TerminalPane,
        axis: SplitAxis,
    ) -> Result<(), TerminalPane> {
        let id = pane.id;
        if !self.root.split(target, id, axis) {
            return Err(pane);
        }
        self.panes.insert(id, pane);
        self.focused_pane = id;
        self.zoomed_pane = None;
        Ok(())
    }

    pub fn close_focused_pane(&mut self) -> Option<PaneId> {
        if self.panes.len() <= 1 {
            return None;
        }
        let closing = self.focused_pane;
        let next = self.root.close(closing)?;
        self.panes.remove(&closing);
        self.focused_pane = next;
        if self.zoomed_pane == Some(closing) {
            self.zoomed_pane = None;
        }
        Some(closing)
    }

    pub fn layout(&self, rect: Rect) -> Vec<(PaneId, Rect)> {
        self.root.visible_layout(rect, self.zoomed_pane)
    }

    pub fn toggle_zoom(&mut self) {
        self.zoomed_pane = if self.zoomed_pane == Some(self.focused_pane) {
            None
        } else {
            Some(self.focused_pane)
        };
    }

    pub fn source_directory(&self) -> &Path {
        &self.focused().working_directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_osc7_directory_uri() {
        assert_eq!(
            directory_from_osc7("file://localhost/tmp/project%20one"),
            Some(PathBuf::from("/tmp/project one"))
        );
        assert_eq!(directory_from_osc7("https://example.com/tmp"), None);
    }
}
