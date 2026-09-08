use std::sync::Arc;

use smol_str::SmolStr;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(Rgb),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct Attributes {
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

impl Default for Attributes {
    fn default() -> Self {
        Self {
            foreground: Color::Default,
            background: Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            blink: false,
            inverse: false,
            hidden: false,
            strikethrough: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum CellWidth {
    #[default]
    Single,
    Wide,
    Continuation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: SmolStr,
    pub attributes: Attributes,
    pub width: CellWidth,
    pub hyperlink: Option<Arc<str>>,
}

impl Cell {
    pub fn blank(attributes: Attributes) -> Self {
        Self {
            text: SmolStr::new_static(" "),
            attributes,
            width: CellWidth::Single,
            hyperlink: None,
        }
    }

    pub fn continuation(attributes: Attributes, hyperlink: Option<Arc<str>>) -> Self {
        Self {
            text: SmolStr::new_static(""),
            attributes,
            width: CellWidth::Continuation,
            hyperlink,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.text == " " || self.text.is_empty()
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Attributes::default())
    }
}
