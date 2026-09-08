use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use terminal_core::{CursorShape, Rgb};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub cursor: CursorConfig,
    pub colors: ColorOverrides,
    pub keybindings: HashMap<String, String>,
    pub workspace: WorkspaceConfig,
    pub theme: ThemeConfig,
    pub themes: HashMap<String, ThemeDefinition>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub fallback_families: Vec<String>,
    pub size: f32,
    pub line_height: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub padding: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct CursorConfig {
    pub style: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ColorOverrides {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub selection_foreground: Option<String>,
    pub selection_background: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub restore: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub active: String,
    pub import: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ThemeDefinition {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub ansi: Vec<String>,
    pub cursor: Option<String>,
    pub selection_foreground: Option<String>,
    pub selection_background: Option<String>,
    pub tab_bar: Option<String>,
    pub inactive_tab: Option<String>,
    pub active_tab: Option<String>,
    pub pane_border: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemePalette {
    pub name: String,
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

pub struct LoadedConfig {
    pub config: AppConfig,
    pub path: PathBuf,
    pub diagnostics: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            window: WindowConfig::default(),
            cursor: CursorConfig::default(),
            colors: ColorOverrides::default(),
            keybindings: HashMap::new(),
            workspace: WorkspaceConfig::default(),
            theme: ThemeConfig::default(),
            themes: HashMap::new(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Menlo".into(),
            fallback_families: Vec::new(),
            size: 14.0,
            line_height: None,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding: 8.0,
            opacity: 1.0,
        }
    }
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: "block".into(),
        }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self { restore: true }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            active: "dark".into(),
            import: None,
        }
    }
}

impl AppConfig {
    pub fn load() -> LoadedConfig {
        let path = config_path();
        Self::load_from_path(path)
    }

    pub fn load_from_path(path: PathBuf) -> LoadedConfig {
        let mut diagnostics = Vec::new();
        let mut config = match fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|error| {
                diagnostics.push(format!(
                    "could not parse {}: {error}; using defaults",
                    path.display()
                ));
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                diagnostics.push(format!(
                    "could not read {}: {error}; using defaults",
                    path.display()
                ));
                Self::default()
            }
        };
        config.validate(&mut diagnostics);
        LoadedConfig {
            config,
            path,
            diagnostics,
        }
    }

    fn validate(&mut self, diagnostics: &mut Vec<String>) {
        if !self.font.size.is_finite() || !(6.0..=72.0).contains(&self.font.size) {
            diagnostics.push("font.size must be between 6 and 72; using 14".into());
            self.font.size = 14.0;
        }
        if self
            .font
            .line_height
            .is_some_and(|value| !value.is_finite() || !(0.8..=3.0).contains(&value))
        {
            diagnostics.push("font.line_height must be between 0.8 and 3; using automatic".into());
            self.font.line_height = None;
        }
        if !self.window.padding.is_finite() || !(0.0..=64.0).contains(&self.window.padding) {
            diagnostics.push("window.padding must be between 0 and 64; using 8".into());
            self.window.padding = 8.0;
        }
        if !self.window.opacity.is_finite() || !(0.2..=1.0).contains(&self.window.opacity) {
            diagnostics.push("window.opacity must be between 0.2 and 1; using 1".into());
            self.window.opacity = 1.0;
        }
    }

    pub fn cursor_shape(&self) -> CursorShape {
        match self.cursor.style.to_ascii_lowercase().as_str() {
            "bar" | "beam" => CursorShape::Bar,
            "underline" => CursorShape::Underline,
            _ => CursorShape::Block,
        }
    }

    pub fn active_theme(&self, override_name: Option<&str>) -> (ThemePalette, Vec<String>) {
        let mut diagnostics = Vec::new();
        let name = override_name.unwrap_or(&self.theme.active);
        let mut palette = match name {
            "light" => ThemePalette::light(),
            _ => ThemePalette::dark(),
        };
        palette.name = name.to_owned();
        if let Some(definition) = self.themes.get(name) {
            palette.apply(definition, &mut diagnostics);
        } else if name != "dark" && name != "light" {
            diagnostics.push(format!("theme '{name}' was not found; using dark colors"));
        }
        palette.apply_colors(&self.colors, &mut diagnostics);
        if let Some(import) = &self.theme.import {
            let path = if import.is_absolute() {
                import.clone()
            } else {
                config_path()
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(import)
            };
            match fs::read_to_string(&path).and_then(|contents| {
                toml::from_str::<ThemeDefinition>(&contents).map_err(std::io::Error::other)
            }) {
                Ok(definition) => palette.apply(&definition, &mut diagnostics),
                Err(error) => {
                    diagnostics.push(format!("could not load theme {}: {error}", path.display()))
                }
            }
        }
        (palette, diagnostics)
    }
}

impl ThemePalette {
    pub fn dark() -> Self {
        Self {
            name: "dark".into(),
            foreground: rgb("dadada"),
            background: rgb("121212"),
            ansi: ansi_dark(),
            cursor: rgb("f4f4f4"),
            selection_foreground: rgb("ffffff"),
            selection_background: rgb("37526e"),
            tab_bar: rgb("191919"),
            inactive_tab: rgb("222222"),
            active_tab: rgb("303030"),
            pane_border: rgb("454545"),
        }
    }

    pub fn light() -> Self {
        Self {
            name: "light".into(),
            foreground: rgb("202124"),
            background: rgb("fafafa"),
            ansi: ansi_light(),
            cursor: rgb("202124"),
            selection_foreground: rgb("101010"),
            selection_background: rgb("b8d7ff"),
            tab_bar: rgb("e8e8e8"),
            inactive_tab: rgb("dedede"),
            active_tab: rgb("ffffff"),
            pane_border: rgb("b8b8b8"),
        }
    }

    fn apply(&mut self, definition: &ThemeDefinition, diagnostics: &mut Vec<String>) {
        apply_color(
            &mut self.foreground,
            &definition.foreground,
            "foreground",
            diagnostics,
        );
        apply_color(
            &mut self.background,
            &definition.background,
            "background",
            diagnostics,
        );
        apply_color(&mut self.cursor, &definition.cursor, "cursor", diagnostics);
        apply_color(
            &mut self.selection_foreground,
            &definition.selection_foreground,
            "selection_foreground",
            diagnostics,
        );
        apply_color(
            &mut self.selection_background,
            &definition.selection_background,
            "selection_background",
            diagnostics,
        );
        apply_color(
            &mut self.tab_bar,
            &definition.tab_bar,
            "tab_bar",
            diagnostics,
        );
        apply_color(
            &mut self.inactive_tab,
            &definition.inactive_tab,
            "inactive_tab",
            diagnostics,
        );
        apply_color(
            &mut self.active_tab,
            &definition.active_tab,
            "active_tab",
            diagnostics,
        );
        apply_color(
            &mut self.pane_border,
            &definition.pane_border,
            "pane_border",
            diagnostics,
        );
        for (index, color) in definition.ansi.iter().take(16).enumerate() {
            match parse_hex(color) {
                Ok(color) => self.ansi[index] = color,
                Err(error) => diagnostics.push(format!("ansi[{index}]: {error}")),
            }
        }
        if !definition.ansi.is_empty() && definition.ansi.len() != 16 {
            diagnostics.push("custom theme ansi should contain exactly 16 colors".into());
        }
    }

    fn apply_colors(&mut self, colors: &ColorOverrides, diagnostics: &mut Vec<String>) {
        apply_color(
            &mut self.foreground,
            &colors.foreground,
            "colors.foreground",
            diagnostics,
        );
        apply_color(
            &mut self.background,
            &colors.background,
            "colors.background",
            diagnostics,
        );
        apply_color(
            &mut self.selection_foreground,
            &colors.selection_foreground,
            "colors.selection_foreground",
            diagnostics,
        );
        apply_color(
            &mut self.selection_background,
            &colors.selection_background,
            "colors.selection_background",
            diagnostics,
        );
    }
}

fn apply_color(
    target: &mut Rgb,
    value: &Option<String>,
    name: &str,
    diagnostics: &mut Vec<String>,
) {
    if let Some(value) = value {
        match parse_hex(value) {
            Ok(color) => *target = color,
            Err(error) => diagnostics.push(format!("{name}: {error}")),
        }
    }
}

pub fn parse_hex(value: &str) -> Result<Rgb, String> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return Err(format!("'{value}' is not a six-digit RGB color"));
    }
    let number =
        u32::from_str_radix(value, 16).map_err(|_| format!("'{value}' is not hexadecimal"))?;
    Ok(Rgb::new(
        (number >> 16) as u8,
        (number >> 8) as u8,
        number as u8,
    ))
}

fn rgb(value: &str) -> Rgb {
    parse_hex(value).expect("built-in color must be valid")
}

fn ansi_dark() -> [Rgb; 16] {
    [
        "000000", "cd3131", "0dbc79", "e5e510", "2472c8", "bc3fbc", "11a8cd", "e5e5e5", "666666",
        "f14c4c", "23d18b", "f5f543", "3b8eea", "d670d6", "29b8db", "ffffff",
    ]
    .map(rgb)
}

fn ansi_light() -> [Rgb; 16] {
    [
        "000000", "c91b00", "00c200", "c7c400", "0225c7", "ca30c7", "00c5c7", "c7c7c7", "686868",
        "ff6e67", "5ffa68", "fffc67", "6871ff", "ff77ff", "60fdff", "ffffff",
    ]
    .map(rgb)
}

pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("GRIN_CONFIG") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("grin").join("config.toml")
}

pub fn workspace_path() -> PathBuf {
    if let Some(path) = std::env::var_os("GRIN_WORKSPACE") {
        return PathBuf::from(path);
    }
    config_path().with_file_name("workspace.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_production_sane() {
        let config = AppConfig::default();
        assert_eq!(config.font.family, "Menlo");
        assert_eq!(config.theme.active, "dark");
        assert!(config.workspace.restore);
    }

    #[test]
    fn valid_custom_config_and_theme_parse() {
        let config: AppConfig = toml::from_str(
            r##"
            [font]
            family = "SF Mono"
            size = 16
            [theme]
            active = "ocean"
            [themes.ocean]
            background = "#001122"
            foreground = "ddeeff"
            [keybindings]
            "cmd+k" = "NewTab"
        "##,
        )
        .unwrap();
        let (theme, diagnostics) = config.active_theme(None);
        assert!(diagnostics.is_empty());
        assert_eq!(theme.background, Rgb::new(0, 17, 34));
        assert_eq!(config.keybindings["cmd+k"], "NewTab");
    }

    #[test]
    fn malformed_file_falls_back_without_crashing() {
        let path =
            std::env::temp_dir().join(format!("grin-config-{}-{}.toml", std::process::id(), 1));
        fs::write(&path, "[font\nsize = nope").unwrap();
        let loaded = AppConfig::load_from_path(path.clone());
        assert_eq!(loaded.config.font.size, 14.0);
        assert_eq!(loaded.diagnostics.len(), 1);
        let _ = fs::remove_file(path);
    }
}
