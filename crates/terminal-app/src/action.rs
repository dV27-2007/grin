use std::{collections::HashMap, fmt};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Action {
    NewTab,
    Close,
    CloseTab,
    NextTab,
    PreviousTab,
    RenameTab,
    DuplicateTab,
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
            "newtab" => Self::NewTab,
            "close" => Self::Close,
            "closetab" => Self::CloseTab,
            "nexttab" => Self::NextTab,
            "previoustab" => Self::PreviousTab,
            "renametab" => Self::RenameTab,
            "duplicatetab" => Self::DuplicateTab,
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
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self::with_overrides(&HashMap::new())
    }
}

fn default_bindings() -> &'static [(&'static str, Action)] {
    &[
        ("cmd+t", Action::NewTab),
        ("cmd+w", Action::Close),
        ("cmd+shift+[", Action::PreviousTab),
        ("cmd+shift+]", Action::NextTab),
        ("cmd+shift+r", Action::RenameTab),
        ("cmd+shift+t", Action::DuplicateTab),
        ("cmd+d", Action::SplitVertical),
        ("cmd+shift+d", Action::SplitHorizontal),
        ("cmd+f", Action::Search),
        ("cmd+c", Action::Copy),
        ("cmd+v", Action::Paste),
        ("cmd+alt+left", Action::FocusLeft),
        ("cmd+alt+right", Action::FocusRight),
        ("cmd+alt+up", Action::FocusUp),
        ("cmd+alt+down", Action::FocusDown),
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
}
