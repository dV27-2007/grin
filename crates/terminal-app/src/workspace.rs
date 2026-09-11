use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::layout::{PaneId, PaneTree};

pub const WORKSPACE_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub theme: String,
    pub windows: Vec<WindowState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub window_width: u32,
    pub window_height: u32,
    pub active_tab: usize,
    pub tabs: Vec<TabState>,
}

#[derive(Serialize, Deserialize)]
struct WorkspaceV1 {
    version: u32,
    window_width: u32,
    window_height: u32,
    active_tab: usize,
    theme: String,
    tabs: Vec<TabState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub title: String,
    pub root: PaneTree,
    pub focused_pane: PaneId,
    pub zoomed_pane: Option<PaneId>,
    pub panes: Vec<PaneState>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneState {
    pub id: PaneId,
    pub working_directory: PathBuf,
}

pub struct WorkspaceLoad {
    pub workspace: Option<Workspace>,
    pub diagnostic: Option<String>,
}

impl Workspace {
    pub fn load(path: &Path) -> WorkspaceLoad {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return WorkspaceLoad {
                    workspace: None,
                    diagnostic: None,
                };
            }
            Err(error) => {
                return WorkspaceLoad {
                    workspace: None,
                    diagnostic: Some(format!(
                        "could not read workspace {}: {error}",
                        path.display()
                    )),
                };
            }
        };
        let version = match toml::from_str::<toml::Value>(&contents)
            .ok()
            .and_then(|value| value.get("version")?.as_integer())
        {
            Some(version) if version >= 0 => version as u32,
            _ => {
                return WorkspaceLoad {
                    workspace: None,
                    diagnostic: Some(format!(
                        "workspace {} is corrupt: missing valid version; starting fresh",
                        path.display()
                    )),
                };
            }
        };
        let parsed = match version {
            1 => toml::from_str::<WorkspaceV1>(&contents).map(|old| {
                debug_assert_eq!(old.version, 1);
                Workspace {
                    version: WORKSPACE_VERSION,
                    theme: old.theme,
                    windows: vec![WindowState {
                        window_width: old.window_width,
                        window_height: old.window_height,
                        active_tab: old.active_tab,
                        tabs: old.tabs,
                    }],
                }
            }),
            WORKSPACE_VERSION => toml::from_str::<Self>(&contents),
            _ => {
                return WorkspaceLoad {
                    workspace: None,
                    diagnostic: Some(format!(
                        "workspace version {version} is unsupported; starting fresh"
                    )),
                };
            }
        };
        match parsed {
            Ok(mut workspace) => {
                workspace.sanitize();
                WorkspaceLoad {
                    workspace: Some(workspace),
                    diagnostic: (version == 1)
                        .then(|| "migrated single-window workspace version 1".into()),
                }
            }
            Err(error) => WorkspaceLoad {
                workspace: None,
                diagnostic: Some(format!(
                    "workspace {} is corrupt: {error}; starting fresh",
                    path.display()
                )),
            },
        }
    }

    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        let Some(parent) = path.parent() else {
            return Err(io::Error::other("workspace path has no parent"));
        };
        fs::create_dir_all(parent)?;
        let encoded = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, encoded)?;
        fs::rename(temporary, path)
    }

    fn sanitize(&mut self) {
        let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.windows.retain(|window| !window.tabs.is_empty());
        for window in &mut self.windows {
            window.window_width = window.window_width.clamp(320, 7680);
            window.window_height = window.window_height.clamp(180, 4320);
            window.active_tab = window.active_tab.min(window.tabs.len() - 1);
            for tab in &mut window.tabs {
                tab.panes.retain(|pane| tab.root.contains(pane.id));
                for pane in &mut tab.panes {
                    if !pane.working_directory.is_dir() {
                        pane.working_directory.clone_from(&fallback);
                    }
                }
                if !tab.root.contains(tab.focused_pane) {
                    tab.focused_pane = tab.root.panes()[0];
                }
                if tab.zoomed_pane.is_some_and(|pane| !tab.root.contains(pane)) {
                    tab.zoomed_pane = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_workspace() -> Workspace {
        Workspace {
            version: WORKSPACE_VERSION,
            theme: "dark".into(),
            windows: vec![WindowState {
                window_width: 960,
                window_height: 600,
                active_tab: 0,
                tabs: vec![TabState {
                    title: "one".into(),
                    pinned: false,
                    root: PaneTree::leaf(PaneId(1)),
                    focused_pane: PaneId(1),
                    zoomed_pane: None,
                    panes: vec![PaneState {
                        id: PaneId(1),
                        working_directory: std::env::current_dir().unwrap(),
                    }],
                }],
            }],
        }
    }

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("grin-workspace-{}-{name}.toml", std::process::id()))
    }

    #[test]
    fn workspace_round_trip_is_versioned() {
        let path = test_path("roundtrip");
        let workspace = sample_workspace();
        workspace.save_atomic(&path).unwrap();
        assert_eq!(Workspace::load(&path).workspace, Some(workspace));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_and_unknown_versions_fall_back() {
        let corrupt = test_path("corrupt");
        fs::write(&corrupt, "not toml [[[").unwrap();
        assert!(Workspace::load(&corrupt).workspace.is_none());
        let version = test_path("version");
        let mut workspace = sample_workspace();
        workspace.version = 99;
        fs::write(&version, toml::to_string(&workspace).unwrap()).unwrap();
        assert!(Workspace::load(&version).workspace.is_none());
        let _ = fs::remove_file(corrupt);
        let _ = fs::remove_file(version);
    }

    #[test]
    fn missing_directories_are_replaced() {
        let path = test_path("missing-dir");
        let mut workspace = sample_workspace();
        workspace.windows[0].tabs[0].panes[0].working_directory =
            PathBuf::from("/definitely/missing/grin-path");
        workspace.save_atomic(&path).unwrap();
        let restored = Workspace::load(&path).workspace.unwrap();
        assert!(
            restored.windows[0].tabs[0].panes[0]
                .working_directory
                .is_dir()
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn multi_window_round_trip_preserves_active_tabs_and_pane_state() {
        let path = test_path("multi-window");
        let mut workspace = sample_workspace();
        let mut second = workspace.windows[0].clone();
        second.window_width = 1280;
        second.active_tab = 1;
        let mut second_tab = second.tabs[0].clone();
        second_tab.title = "two".into();
        second_tab.pinned = true;
        second_tab.root = PaneTree::leaf(PaneId(2));
        assert!(
            second_tab
                .root
                .split(PaneId(2), PaneId(3), crate::layout::SplitAxis::Vertical)
        );
        second_tab.focused_pane = PaneId(3);
        second_tab.zoomed_pane = Some(PaneId(3));
        second_tab.panes = vec![
            PaneState {
                id: PaneId(2),
                working_directory: std::env::current_dir().unwrap(),
            },
            PaneState {
                id: PaneId(3),
                working_directory: std::env::current_dir().unwrap(),
            },
        ];
        second.tabs.push(second_tab);
        workspace.windows.push(second);
        workspace.save_atomic(&path).unwrap();
        assert_eq!(Workspace::load(&path).workspace, Some(workspace));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn version_one_workspace_migrates_to_one_window() {
        let path = test_path("v1");
        let current = sample_workspace();
        let window = &current.windows[0];
        let old = WorkspaceV1 {
            version: 1,
            window_width: window.window_width,
            window_height: window.window_height,
            active_tab: window.active_tab,
            theme: current.theme.clone(),
            tabs: window.tabs.clone(),
        };
        fs::write(&path, toml::to_string(&old).unwrap()).unwrap();
        let loaded = Workspace::load(&path);
        assert_eq!(loaded.workspace.unwrap().windows, current.windows);
        assert!(loaded.diagnostic.unwrap().contains("migrated"));
        let _ = fs::remove_file(path);
    }
}
