use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::layout::{PaneId, PaneTree};

pub const WORKSPACE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub window_width: u32,
    pub window_height: u32,
    pub active_tab: usize,
    pub theme: String,
    pub tabs: Vec<TabState>,
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
        match toml::from_str::<Self>(&contents) {
            Ok(mut workspace) if workspace.version == WORKSPACE_VERSION => {
                workspace.sanitize();
                WorkspaceLoad {
                    workspace: Some(workspace),
                    diagnostic: None,
                }
            }
            Ok(workspace) => WorkspaceLoad {
                workspace: None,
                diagnostic: Some(format!(
                    "workspace version {} is unsupported; starting fresh",
                    workspace.version
                )),
            },
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
        self.window_width = self.window_width.clamp(320, 7680);
        self.window_height = self.window_height.clamp(180, 4320);
        if self.tabs.is_empty() {
            self.active_tab = 0;
            return;
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for tab in &mut self.tabs {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_workspace() -> Workspace {
        Workspace {
            version: WORKSPACE_VERSION,
            window_width: 960,
            window_height: 600,
            active_tab: 0,
            theme: "dark".into(),
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
        workspace.tabs[0].panes[0].working_directory =
            PathBuf::from("/definitely/missing/grin-path");
        workspace.save_atomic(&path).unwrap();
        let restored = Workspace::load(&path).workspace.unwrap();
        assert!(restored.tabs[0].panes[0].working_directory.is_dir());
        let _ = fs::remove_file(path);
    }
}
