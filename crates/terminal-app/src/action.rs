use std::{collections::HashMap, fmt};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Action {
    ToggleCommandPalette,
    NewTab,
    TogglePinTab,
    Close,
    CloseTab,
    NextTab,
    PreviousTab,
    MoveTabLeft,
    MoveTabRight,
    RenameTab,
    DuplicateTab,
    RestoreTab,
    SelectTab1,
    SelectTab2,
    SelectTab3,
    SelectTab4,
    SelectTab5,
    SelectTab6,
    SelectTab7,
    SelectTab8,
    SelectTab9,
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    SwapPaneLeft,
    SwapPaneRight,
    SwapPaneUp,
    SwapPaneDown,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    TogglePaneZoom,
    Copy,
    Paste,
    Search,
    ThemeDark,
    ThemeLight,
}

impl Action {
    pub fn parse(value: &str) -> Option<Self> {
        let normalized: String = value
            .chars()
            .filter(|character| {
                *character != '_' && *character != '-' && !character.is_whitespace()
            })
            .flat_map(char::to_lowercase)
            .collect();
        Some(match normalized.as_str() {
            "togglecommandpalette" => Self::ToggleCommandPalette,
            "newtab" => Self::NewTab,
            "togglepintab" => Self::TogglePinTab,
            "close" => Self::Close,
            "closetab" => Self::CloseTab,
            "nexttab" => Self::NextTab,
            "previoustab" => Self::PreviousTab,
            "movetableft" => Self::MoveTabLeft,
            "movetabright" => Self::MoveTabRight,
            "renametab" => Self::RenameTab,
            "duplicatetab" => Self::DuplicateTab,
            "restoretab" => Self::RestoreTab,
            "selecttab1" => Self::SelectTab1,
            "selecttab2" => Self::SelectTab2,
            "selecttab3" => Self::SelectTab3,
            "selecttab4" => Self::SelectTab4,
            "selecttab5" => Self::SelectTab5,
            "selecttab6" => Self::SelectTab6,
            "selecttab7" => Self::SelectTab7,
            "selecttab8" => Self::SelectTab8,
            "selecttab9" => Self::SelectTab9,
            "splithorizontal" => Self::SplitHorizontal,
            "splitvertical" => Self::SplitVertical,
            "closepane" => Self::ClosePane,
            "focusleft" => Self::FocusLeft,
            "focusright" => Self::FocusRight,
            "focusup" => Self::FocusUp,
            "focusdown" => Self::FocusDown,
            "swappaneleft" => Self::SwapPaneLeft,
            "swappaneright" => Self::SwapPaneRight,
            "swappaneup" => Self::SwapPaneUp,
            "swappanedown" => Self::SwapPaneDown,
            "resizeleft" => Self::ResizeLeft,
            "resizeright" => Self::ResizeRight,
            "resizeup" => Self::ResizeUp,
            "resizedown" => Self::ResizeDown,
            "togglepanezoom" => Self::TogglePaneZoom,
            "copy" => Self::Copy,
            "paste" => Self::Paste,
            "search" => Self::Search,
            "themedark" => Self::ThemeDark,
            "themelight" => Self::ThemeLight,
            _ => return None,
        })
    }

    pub fn tab_index(self) -> Option<usize> {
        match self {
            Self::SelectTab1 => Some(0),
            Self::SelectTab2 => Some(1),
            Self::SelectTab3 => Some(2),
            Self::SelectTab4 => Some(3),
            Self::SelectTab5 => Some(4),
            Self::SelectTab6 => Some(5),
            Self::SelectTab7 => Some(6),
            Self::SelectTab8 => Some(7),
            Self::SelectTab9 => Some(8),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub keywords: &'static str,
    pub action: Action,
}

pub fn action_registry() -> &'static [ActionInfo] {
    &[
        ActionInfo {
            id: "terminal.new_tab",
            title: "New Tab",
            category: "Terminal",
            keywords: "new terminal create",
            action: Action::NewTab,
        },
        ActionInfo {
            id: "terminal.close",
            title: "Close",
            category: "Terminal",
            keywords: "close terminal",
            action: Action::Close,
        },
        ActionInfo {
            id: "terminal.duplicate_tab",
            title: "Duplicate Tab",
            category: "Terminal",
            keywords: "copy clone terminal",
            action: Action::DuplicateTab,
        },
        ActionInfo {
            id: "terminal.restore_closed_tab",
            title: "Restore Closed Tab",
            category: "Terminal",
            keywords: "restore reopen undo",
            action: Action::RestoreTab,
        },
        ActionInfo {
            id: "terminal.rename_tab",
            title: "Rename Tab",
            category: "Terminal",
            keywords: "rename title",
            action: Action::RenameTab,
        },
        ActionInfo {
            id: "pane.split_right",
            title: "Split Right",
            category: "Pane",
            keywords: "split vertical right",
            action: Action::SplitVertical,
        },
        ActionInfo {
            id: "pane.split_down",
            title: "Split Down",
            category: "Pane",
            keywords: "split horizontal down",
            action: Action::SplitHorizontal,
        },
        ActionInfo {
            id: "pane.zoom",
            title: "Zoom Pane",
            category: "Pane",
            keywords: "zoom maximize",
            action: Action::TogglePaneZoom,
        },
        ActionInfo {
            id: "pane.focus_left",
            title: "Focus Left",
            category: "Pane",
            keywords: "focus pane left",
            action: Action::FocusLeft,
        },
        ActionInfo {
            id: "pane.focus_right",
            title: "Focus Right",
            category: "Pane",
            keywords: "focus pane right",
            action: Action::FocusRight,
        },
        ActionInfo {
            id: "pane.focus_up",
            title: "Focus Up",
            category: "Pane",
            keywords: "focus pane up",
            action: Action::FocusUp,
        },
        ActionInfo {
            id: "pane.focus_down",
            title: "Focus Down",
            category: "Pane",
            keywords: "focus pane down",
            action: Action::FocusDown,
        },
        ActionInfo {
            id: "pane.swap_left",
            title: "Swap Left",
            category: "Pane",
            keywords: "swap pane left",
            action: Action::SwapPaneLeft,
        },
        ActionInfo {
            id: "pane.swap_right",
            title: "Swap Right",
            category: "Pane",
            keywords: "swap pane right",
            action: Action::SwapPaneRight,
        },
        ActionInfo {
            id: "pane.swap_up",
            title: "Swap Up",
            category: "Pane",
            keywords: "swap pane up",
            action: Action::SwapPaneUp,
        },
        ActionInfo {
            id: "pane.swap_down",
            title: "Swap Down",
            category: "Pane",
            keywords: "swap pane down",
            action: Action::SwapPaneDown,
        },
        ActionInfo {
            id: "view.search",
            title: "Search",
            category: "View",
            keywords: "find scrollback",
            action: Action::Search,
        },
    ]
}

pub fn match_actions(query: &str) -> Vec<&'static ActionInfo> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return action_registry().iter().collect();
    }
    let mut matches: Vec<_> = action_registry()
        .iter()
        .filter_map(|info| {
            let title = info.title.to_ascii_lowercase();
            let score = if title == query {
                Some(0)
            } else if title.starts_with(&query) {
                Some(1)
            } else if title.contains(&query) {
                Some(2)
            } else if ordered_subsequence(&title, &query) {
                Some(3)
            } else if info.keywords.to_ascii_lowercase().contains(&query) {
                Some(4)
            } else if info.category.to_ascii_lowercase().contains(&query) {
                Some(5)
            } else {
                None
            }?;
            Some((score, info))
        })
        .collect();
    matches.sort_by_key(|(score, info)| (*score, info.title));
    matches.into_iter().map(|(_, info)| info).collect()
}

fn ordered_subsequence(value: &str, query: &str) -> bool {
    let mut chars = value.chars();
    query
        .chars()
        .all(|needle| chars.by_ref().any(|value| value == needle))
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeyChord {
    pub command: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

impl KeyChord {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut chord = Self {
            command: false,
            control: false,
            alt: false,
            shift: false,
            key: String::new(),
        };
        for part in value
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "super" => chord.command = true,
                "ctrl" | "control" => chord.control = true,
                "alt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                key if chord.key.is_empty() => chord.key = normalize_key(key),
                _ => return Err(format!("key chord has more than one key: {value}")),
            }
        }
        if chord.key.is_empty() {
            return Err(format!("key chord has no key: {value}"));
        }
        Ok(chord)
    }
}

fn normalize_key(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "leftbracket" => "[".into(),
        "rightbracket" => "]".into(),
        "return" => "enter".into(),
        other => other.into(),
    }
}

#[derive(Clone, Debug)]
pub struct KeyBindings {
    bindings: HashMap<KeyChord, Action>,
    diagnostics: Vec<String>,
}

impl KeyBindings {
    pub fn with_overrides(overrides: &HashMap<String, String>) -> Self {
        let mut bindings = HashMap::new();
        for (shortcut, action) in default_bindings() {
            let chord = KeyChord::parse(shortcut).expect("built-in key chord must be valid");
            bindings.insert(chord, *action);
        }
        let mut diagnostics = Vec::new();
        let mut ordered: Vec<_> = overrides.iter().collect();
        ordered.sort_by(|left, right| left.0.cmp(right.0));
        for (shortcut, action_name) in ordered {
            match (KeyChord::parse(shortcut), Action::parse(action_name)) {
                (Ok(chord), Some(action)) => {
                    bindings.insert(chord, action);
                }
                (Err(error), _) => diagnostics.push(error),
                (_, None) => diagnostics.push(format!("unknown action '{action_name}'")),
            }
        }
        Self {
            bindings,
            diagnostics,
        }
    }

    pub fn action(&self, chord: &KeyChord) -> Option<Action> {
        self.bindings.get(chord).copied()
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    pub fn shortcut_label(&self, action: Action) -> Option<String> {
        self.shortcut(action).map(|chord| format_shortcut(&chord))
    }

    pub fn shortcut(&self, action: Action) -> Option<KeyChord> {
        let mut chords: Vec<_> = self
            .bindings
            .iter()
            .filter_map(|(chord, bound)| (*bound == action).then_some(chord.clone()))
            .collect();
        chords.sort_by_key(|chord| {
            format!(
                "{}{}{}{}{}",
                chord.command, chord.control, chord.alt, chord.shift, chord.key
            )
        });
        chords.into_iter().next()
    }
}

pub fn format_shortcut(chord: &KeyChord) -> String {
    if cfg!(target_os = "macos") {
        let mut label = String::new();
        if chord.control {
            label.push('⌃');
        }
        if chord.alt {
            label.push('⌥');
        }
        if chord.shift {
            label.push('⇧');
        }
        if chord.command {
            label.push('⌘');
        }
        label.push_str(&chord.key.to_ascii_uppercase());
        label
    } else {
        let mut parts = Vec::new();
        if chord.control || chord.command {
            parts.push("Ctrl");
        }
        if chord.alt {
            parts.push("Alt");
        }
        if chord.shift {
            parts.push("Shift");
        }
        parts.push(&chord.key);
        parts.join("+")
    }
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self::with_overrides(&HashMap::new())
    }
}

fn default_bindings() -> &'static [(&'static str, Action)] {
    &[
        ("cmd+t", Action::NewTab),
        ("cmd+shift+p", Action::ToggleCommandPalette),
        ("cmd+alt+p", Action::TogglePinTab),
        ("cmd+w", Action::Close),
        ("cmd+shift+w", Action::CloseTab),
        ("cmd+shift+[", Action::PreviousTab),
        ("cmd+shift+]", Action::NextTab),
        ("cmd+alt+[", Action::MoveTabLeft),
        ("cmd+alt+]", Action::MoveTabRight),
        ("cmd+shift+r", Action::RenameTab),
        ("cmd+shift+t", Action::DuplicateTab),
        ("cmd+alt+t", Action::RestoreTab),
        ("cmd+d", Action::SplitVertical),
        ("cmd+shift+d", Action::SplitHorizontal),
        ("cmd+f", Action::Search),
        ("cmd+c", Action::Copy),
        ("cmd+v", Action::Paste),
        ("cmd+alt+left", Action::FocusLeft),
        ("cmd+alt+right", Action::FocusRight),
        ("cmd+alt+up", Action::FocusUp),
        ("cmd+alt+down", Action::FocusDown),
        ("cmd+alt+shift+left", Action::SwapPaneLeft),
        ("cmd+alt+shift+right", Action::SwapPaneRight),
        ("cmd+alt+shift+up", Action::SwapPaneUp),
        ("cmd+alt+shift+down", Action::SwapPaneDown),
        ("cmd+ctrl+left", Action::ResizeLeft),
        ("cmd+ctrl+right", Action::ResizeRight),
        ("cmd+ctrl+up", Action::ResizeUp),
        ("cmd+ctrl+down", Action::ResizeDown),
        ("cmd+shift+enter", Action::TogglePaneZoom),
        ("cmd+1", Action::SelectTab1),
        ("cmd+2", Action::SelectTab2),
        ("cmd+3", Action::SelectTab3),
        ("cmd+4", Action::SelectTab4),
        ("cmd+5", Action::SelectTab5),
        ("cmd+6", Action::SelectTab6),
        ("cmd+7", Action::SelectTab7),
        ("cmd+8", Action::SelectTab8),
        ("cmd+9", Action::SelectTab9),
    ]
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_binding_predictably_overrides_default() {
        let overrides = HashMap::from([("cmd+t".into(), "SplitHorizontal".into())]);
        let bindings = KeyBindings::with_overrides(&overrides);
        let chord = KeyChord::parse("Command+T").unwrap();
        assert_eq!(bindings.action(&chord), Some(Action::SplitHorizontal));
    }

    #[test]
    fn invalid_bindings_are_diagnostics_not_failures() {
        let overrides = HashMap::from([
            ("cmd".into(), "NewTab".into()),
            ("cmd+x".into(), "Imaginary".into()),
        ]);
        let bindings = KeyBindings::with_overrides(&overrides);
        assert_eq!(bindings.diagnostics().len(), 2);
        assert_eq!(
            bindings.action(&KeyChord::parse("cmd+t").unwrap()),
            Some(Action::NewTab)
        );
    }

    #[test]
    fn action_matcher_ranks_exact_prefix_substring_fuzzy_and_metadata() {
        assert_eq!(match_actions("")[0].action, Action::NewTab);
        assert_eq!(match_actions("NEW TAB")[0].action, Action::NewTab);
        assert_eq!(match_actions("split r")[0].action, Action::SplitVertical);
        assert_eq!(match_actions("nt")[0].action, Action::NewTab);
        assert_eq!(match_actions("restore")[0].action, Action::RestoreTab);
        assert_eq!(match_actions("terminal")[0].category, "Terminal");
        assert!(match_actions("not-an-action").is_empty());
    }

    #[test]
    fn shortcut_formatter_uses_platform_conventions() {
        let chord = KeyChord::parse("cmd+alt+shift+t").unwrap();
        let expected = if cfg!(target_os = "macos") {
            "⌥⇧⌘T"
        } else {
            "Ctrl+Alt+Shift+T"
        };
        assert_eq!(format_shortcut(&chord), expected);
    }

    #[test]
    fn contextual_close_is_the_only_cmd_w_binding() {
        let bindings = KeyBindings::default();
        assert_eq!(
            bindings.shortcut(Action::Close),
            Some(KeyChord::parse("cmd+w").unwrap())
        );
        assert_eq!(bindings.shortcut(Action::ClosePane), None);
    }
}
