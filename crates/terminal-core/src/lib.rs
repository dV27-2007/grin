//! Window-, GPU-, and process-independent terminal state.

mod cell;
mod screen;
mod terminal;

pub use cell::{Attributes, Cell, CellWidth, Color, Rgb};
pub use screen::{Cursor, CursorShape, GridPoint, Screen, SearchMatch, Selection};
pub use terminal::Terminal;
