use std::{collections::VecDeque, sync::Arc};

use smol_str::SmolStr;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{Attributes, Cell, CellWidth};

pub const DEFAULT_SCROLLBACK_LIMIT: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub column: usize,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            column: 0,
            visible: true,
            shape: CursorShape::Block,
            blinking: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SavedCursor {
    cursor: Cursor,
    attributes: Attributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    /// Index in the logical `scrollback + live screen` line sequence.
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: GridPoint,
    pub end: GridPoint,
}

impl Selection {
    fn ordered(self) -> (GridPoint, GridPoint) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: GridPoint,
    pub columns: usize,
}

type Line = Vec<Cell>;

#[derive(Debug)]
pub struct Screen {
    columns: usize,
    rows: usize,
    lines: Vec<Line>,
    cursor: Cursor,
    attributes: Attributes,
    saved_cursor: SavedCursor,
    scroll_top: usize,
    scroll_bottom: usize,
    tab_stops: Vec<bool>,
    scrollback: VecDeque<Line>,
    scrollback_limit: usize,
    allow_scrollback: bool,
    viewport_offset: usize,
    selection: Option<Selection>,
    search_match: Option<SearchMatch>,
    active_hyperlink: Option<Arc<str>>,
    wrap_pending: bool,
    generation: u64,
}

impl Screen {
    pub fn new(columns: usize, rows: usize, allow_scrollback: bool) -> Self {
        assert!(
            columns > 0 && rows > 0,
            "terminal dimensions must be non-zero"
        );
        Self {
            columns,
            rows,
            lines: (0..rows)
                .map(|_| blank_line(columns, Attributes::default()))
                .collect(),
            cursor: Cursor::default(),
            attributes: Attributes::default(),
            saved_cursor: SavedCursor::default(),
            scroll_top: 0,
            scroll_bottom: rows,
            tab_stops: tab_stops(columns),
            scrollback: VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK_LIMIT,
            allow_scrollback,
            viewport_offset: 0,
            selection: None,
            search_match: None,
            active_hyperlink: None,
            wrap_pending: false,
            generation: 1,
        }
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn viewport_offset(&self) -> usize {
        self.viewport_offset
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn search_match(&self) -> Option<SearchMatch> {
        self.search_match
    }

    pub fn cell(&self, row: usize, column: usize) -> Option<&Cell> {
        self.lines.get(row).and_then(|line| line.get(column))
    }

    pub fn display_cell(&self, row: usize, column: usize) -> Option<&Cell> {
        self.display_line(row)?.get(column)
    }

    pub fn display_line(&self, row: usize) -> Option<&[Cell]> {
        if row >= self.rows {
            return None;
        }
        self.logical_line(self.display_top() + row)
            .map(Vec::as_slice)
    }

    pub fn display_cursor(&self) -> Option<Cursor> {
        (self.viewport_offset == 0).then_some(self.cursor)
    }

    pub fn selected_at(&self, row: usize, column: usize) -> bool {
        let Some(selection) = self.selection else {
            return false;
        };
        let point = GridPoint {
            line: self.display_top() + row,
            column,
        };
        let (start, end) = selection.ordered();
        point >= start && point <= end
    }

    pub fn search_match_at(&self, row: usize, column: usize) -> bool {
        let Some(found) = self.search_match else {
            return false;
        };
        let point = GridPoint {
            line: self.display_top() + row,
            column,
        };
        point.line == found.start.line
            && point.column >= found.start.column
            && point.column < found.start.column + found.columns
    }

    pub fn hyperlink_at(&self, row: usize, column: usize) -> Option<&str> {
        self.display_cell(row, column)?.hyperlink.as_deref()
    }

    pub fn set_scrollback_limit(&mut self, limit: usize) {
        self.scrollback_limit = limit;
        self.trim_scrollback();
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        if columns == 0 || rows == 0 || (columns == self.columns && rows == self.rows) {
            return;
        }
        if columns != self.columns {
            for line in self.scrollback.iter_mut().chain(self.lines.iter_mut()) {
                resize_line(line, columns);
            }
            self.columns = columns;
            self.tab_stops = tab_stops(columns);
        }
        if rows < self.rows {
            let remove = self.rows - rows;
            for _ in 0..remove {
                let line = self.lines.remove(0);
                if self.allow_scrollback {
                    self.push_scrollback(line);
                }
            }
            self.cursor.row = self.cursor.row.saturating_sub(remove);
        } else if rows > self.rows {
            let line = self.blank_line();
            self.lines
                .extend((0..rows - self.rows).map(|_| line.clone()));
        }
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.column = self.cursor.column.min(columns - 1);
        self.wrap_pending = false;
        self.viewport_offset = self.viewport_offset.min(self.scrollback.len());
        self.mark_changed();
    }

    /// Positive deltas move toward older history; negative deltas return to live output.
    pub fn scroll_viewport(&mut self, delta: isize) {
        let offset = if delta >= 0 {
            self.viewport_offset.saturating_add(delta as usize)
        } else {
            self.viewport_offset.saturating_sub(delta.unsigned_abs())
        }
        .min(self.scrollback.len());
        if offset != self.viewport_offset {
            self.viewport_offset = offset;
            self.mark_changed();
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.viewport_offset != 0 {
            self.viewport_offset = 0;
            self.mark_changed();
        }
    }

    pub fn begin_selection(&mut self, row: usize, column: usize) {
        let point = self.display_point(row, column);
        self.selection = Some(Selection {
            start: point,
            end: point,
        });
        self.mark_changed();
    }

    pub fn update_selection(&mut self, row: usize, column: usize) {
        let point = self.display_point(row, column);
        if let Some(selection) = &mut self.selection {
            selection.end = point;
            self.mark_changed();
        }
    }

    pub fn clear_selection(&mut self) {
        if self.selection.take().is_some() {
            self.mark_changed();
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection?.ordered();
        let mut output = String::new();
        for line_index in start.line..=end.line {
            let Some(line) = self.logical_line(line_index) else {
                continue;
            };
            let from = if line_index == start.line {
                start.column
            } else {
                0
            };
            let to = if line_index == end.line {
                end.column
            } else {
                self.columns - 1
            };
            let mut text = String::new();
            for cell in &line[from.min(line.len() - 1)..=to.min(line.len() - 1)] {
                if cell.width != CellWidth::Continuation {
                    text.push_str(&cell.text);
                }
            }
            output.push_str(text.trim_end_matches(' '));
            if line_index != end.line {
                output.push('\n');
            }
        }
        Some(output)
    }

    pub fn search(&mut self, query: &str) -> Option<SearchMatch> {
        self.search_match = None;
        if query.is_empty() {
            self.mark_changed();
            return None;
        }
        for line_index in (0..self.logical_line_count()).rev() {
            let Some(line) = self.logical_line(line_index) else {
                continue;
            };
            let text = line_text(line);
            if let Some(byte_index) = text.rfind(query) {
                let result = SearchMatch {
                    start: GridPoint {
                        line: line_index,
                        column: UnicodeWidthStr::width(&text[..byte_index]).min(self.columns - 1),
                    },
                    columns: UnicodeWidthStr::width(query).max(1),
                };
                self.search_match = Some(result);
                self.reveal_line(line_index);
                self.mark_changed();
                return Some(result);
            }
        }
        self.mark_changed();
        None
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.columns, self.rows, self.allow_scrollback);
    }

    pub(crate) fn attributes_mut(&mut self) -> &mut Attributes {
        &mut self.attributes
    }

    pub(crate) fn set_active_hyperlink(&mut self, link: Option<Arc<str>>) {
        self.active_hyperlink = link;
    }

    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
        self.mark_changed();
    }

    pub(crate) fn set_cursor_style(&mut self, shape: CursorShape, blinking: bool) {
        self.cursor.shape = shape;
        self.cursor.blinking = blinking;
        self.mark_changed();
    }

    pub(crate) fn put(&mut self, character: char, auto_wrap: bool, insert_mode: bool) {
        if self.try_extend_grapheme(character) {
            return;
        }
        if self.wrap_pending {
            if auto_wrap {
                self.cursor.column = 0;
                self.line_feed();
            }
            self.wrap_pending = false;
        }
        let mut width = UnicodeWidthChar::width(character).unwrap_or(1).clamp(1, 2);
        if width == 2 && self.cursor.column + 1 >= self.columns {
            if auto_wrap && self.columns > 1 {
                self.cursor.column = 0;
                self.line_feed();
            } else {
                width = 1;
            }
        }
        if insert_mode {
            self.insert_cells(width);
        }
        self.clear_wide_at(self.cursor.row, self.cursor.column);
        if width == 2 {
            self.clear_wide_at(self.cursor.row, self.cursor.column + 1);
        }
        self.lines[self.cursor.row][self.cursor.column] = Cell {
            text: SmolStr::new(character.to_string()),
            attributes: self.attributes,
            width: if width == 2 {
                CellWidth::Wide
            } else {
                CellWidth::Single
            },
            hyperlink: self.active_hyperlink.clone(),
        };
        if width == 2 {
            self.lines[self.cursor.row][self.cursor.column + 1] =
                Cell::continuation(self.attributes, self.active_hyperlink.clone());
        }
        if self.cursor.column + width >= self.columns {
            self.cursor.column = self.columns - 1;
            self.wrap_pending = true;
        } else {
            self.cursor.column += width;
        }
        self.viewport_offset = 0;
        self.mark_changed();
    }

    pub(crate) fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.cursor.row == self.scroll_bottom - 1 {
            self.scroll_up(1);
        } else {
            self.cursor.row = (self.cursor.row + 1).min(self.rows - 1);
        }
        self.mark_changed();
    }

    pub(crate) fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
        self.mark_changed();
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor.column = 0;
        self.wrap_pending = false;
        self.mark_changed();
    }

    pub(crate) fn backspace(&mut self) {
        self.cursor.column = self.cursor.column.saturating_sub(1);
        self.wrap_pending = false;
        self.mark_changed();
    }

    pub(crate) fn tab(&mut self) {
        self.wrap_pending = false;
        self.cursor.column = ((self.cursor.column + 1)..self.columns)
            .find(|column| self.tab_stops[*column])
            .unwrap_or(self.columns - 1);
        self.mark_changed();
    }

    pub(crate) fn set_tab_stop(&mut self) {
        self.tab_stops[self.cursor.column] = true;
    }

    pub(crate) fn clear_tab_stop(&mut self, all: bool) {
        if all {
            self.tab_stops.fill(false);
        } else {
            self.tab_stops[self.cursor.column] = false;
        }
    }

    pub(crate) fn move_cursor(&mut self, row_delta: isize, column_delta: isize) {
        self.cursor.row = self
            .cursor
            .row
            .saturating_add_signed(row_delta)
            .min(self.rows - 1);
        self.cursor.column = self
            .cursor
            .column
            .saturating_add_signed(column_delta)
            .min(self.columns - 1);
        self.wrap_pending = false;
        self.mark_changed();
    }

    pub(crate) fn set_cursor(&mut self, row: usize, column: usize, origin_mode: bool) {
        let (top, bottom) = if origin_mode {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows)
        };
        self.cursor.row = (top + row).min(bottom - 1);
        self.cursor.column = column.min(self.columns - 1);
        self.wrap_pending = false;
        self.mark_changed();
    }

    pub(crate) fn set_cursor_row(&mut self, row: usize, origin_mode: bool) {
        let column = self.cursor.column;
        self.set_cursor(row, column, origin_mode);
    }

    pub(crate) fn set_cursor_column(&mut self, column: usize) {
        self.cursor.column = column.min(self.columns - 1);
        self.wrap_pending = false;
        self.mark_changed();
    }

    pub(crate) fn erase_display(&mut self, mode: u16) {
        let row = self.cursor.row;
        let column = self.cursor.column;
        let blank = self.blank_cell();
        match mode {
            0 => {
                self.lines[row][column..].fill(blank.clone());
                for line in &mut self.lines[row + 1..] {
                    line.fill(blank.clone());
                }
            }
            1 => {
                for line in &mut self.lines[..row] {
                    line.fill(blank.clone());
                }
                self.lines[row][..=column].fill(blank);
            }
            2 => {
                for line in &mut self.lines {
                    line.fill(blank.clone());
                }
            }
            3 => {
                self.scrollback.clear();
                self.viewport_offset = 0;
            }
            _ => return,
        }
        self.mark_changed();
    }

    pub(crate) fn erase_line(&mut self, mode: u16) {
        let blank = self.blank_cell();
        match mode {
            0 => self.lines[self.cursor.row][self.cursor.column..].fill(blank),
            1 => self.lines[self.cursor.row][..=self.cursor.column].fill(blank),
            2 => self.lines[self.cursor.row].fill(blank),
            _ => return,
        }
        self.mark_changed();
    }

    pub(crate) fn erase_cells(&mut self, count: usize) {
        let end = (self.cursor.column + count).min(self.columns);
        let blank = self.blank_cell();
        self.lines[self.cursor.row][self.cursor.column..end].fill(blank);
        self.mark_changed();
    }

    pub(crate) fn insert_cells(&mut self, count: usize) {
        let count = count.min(self.columns - self.cursor.column);
        let blank = self.blank_cell();
        let line = &mut self.lines[self.cursor.row];
        line[self.cursor.column..].rotate_right(count);
        line[self.cursor.column..self.cursor.column + count].fill(blank);
        normalize_wide(line);
        self.mark_changed();
    }

    pub(crate) fn delete_cells(&mut self, count: usize) {
        let count = count.min(self.columns - self.cursor.column);
        let blank = self.blank_cell();
        let line = &mut self.lines[self.cursor.row];
        line[self.cursor.column..].rotate_left(count);
        line[self.columns - count..].fill(blank);
        normalize_wide(line);
        self.mark_changed();
    }

    pub(crate) fn insert_lines(&mut self, count: usize) {
        if !(self.scroll_top..self.scroll_bottom).contains(&self.cursor.row) {
            return;
        }
        for _ in 0..count.min(self.scroll_bottom - self.cursor.row) {
            self.lines.insert(self.cursor.row, self.blank_line());
            self.lines.remove(self.scroll_bottom);
        }
        self.mark_changed();
    }

    pub(crate) fn delete_lines(&mut self, count: usize) {
        if !(self.scroll_top..self.scroll_bottom).contains(&self.cursor.row) {
            return;
        }
        for _ in 0..count.min(self.scroll_bottom - self.cursor.row) {
            self.lines.remove(self.cursor.row);
            self.lines.insert(self.scroll_bottom - 1, self.blank_line());
        }
        self.mark_changed();
    }

    pub(crate) fn scroll_up(&mut self, count: usize) {
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top) {
            let line = self.lines.remove(self.scroll_top);
            if self.scroll_top == 0 && self.scroll_bottom == self.rows && self.allow_scrollback {
                self.push_scrollback(line);
            }
            self.lines.insert(self.scroll_bottom - 1, self.blank_line());
        }
        self.mark_changed();
    }

    pub(crate) fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.min(self.scroll_bottom - self.scroll_top) {
            self.lines.remove(self.scroll_bottom - 1);
            self.lines.insert(self.scroll_top, self.blank_line());
        }
        self.mark_changed();
    }

    pub(crate) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        if top < bottom && bottom <= self.rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.set_cursor(0, 0, false);
        }
    }

    pub(crate) fn save_cursor(&mut self) {
        self.saved_cursor = SavedCursor {
            cursor: self.cursor,
            attributes: self.attributes,
        };
    }

    pub(crate) fn restore_cursor(&mut self) {
        self.cursor = self.saved_cursor.cursor;
        self.attributes = self.saved_cursor.attributes;
        self.wrap_pending = false;
        self.mark_changed();
    }

    fn try_extend_grapheme(&mut self, character: char) -> bool {
        let mut column = if self.wrap_pending {
            self.cursor.column
        } else if self.cursor.column > 0 {
            self.cursor.column - 1
        } else {
            return false;
        };
        while column > 0 && self.lines[self.cursor.row][column].width == CellWidth::Continuation {
            column -= 1;
        }
        let cell = &self.lines[self.cursor.row][column];
        if cell.width == CellWidth::Continuation || cell.text == " " {
            return false;
        }
        let mut candidate = cell.text.to_string();
        candidate.push(character);
        if UnicodeSegmentation::graphemes(candidate.as_str(), true).count() != 1 {
            return false;
        }
        let new_width = UnicodeWidthStr::width(candidate.as_str()).clamp(1, 2);
        let old_width = usize::from(cell.width == CellWidth::Wide) + 1;
        self.lines[self.cursor.row][column].text = SmolStr::new(candidate);
        if new_width == 2 && old_width == 1 && column + 1 < self.columns {
            self.lines[self.cursor.row][column].width = CellWidth::Wide;
            self.lines[self.cursor.row][column + 1] =
                Cell::continuation(self.attributes, self.active_hyperlink.clone());
            if !self.wrap_pending {
                if self.cursor.column + 1 >= self.columns {
                    self.cursor.column = self.columns - 1;
                    self.wrap_pending = true;
                } else {
                    self.cursor.column += 1;
                }
            }
        }
        self.mark_changed();
        true
    }

    fn clear_wide_at(&mut self, row: usize, column: usize) {
        if column >= self.columns {
            return;
        }
        let blank = self.blank_cell();
        match self.lines[row][column].width {
            CellWidth::Wide => {
                self.lines[row][column] = blank.clone();
                if column + 1 < self.columns {
                    self.lines[row][column + 1] = blank;
                }
            }
            CellWidth::Continuation => {
                self.lines[row][column] = blank.clone();
                if column > 0 {
                    self.lines[row][column - 1] = blank;
                }
            }
            CellWidth::Single => {}
        }
    }

    fn blank_cell(&self) -> Cell {
        Cell::blank(self.attributes)
    }

    fn blank_line(&self) -> Line {
        blank_line(self.columns, self.attributes)
    }

    fn mark_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn logical_line_count(&self) -> usize {
        self.scrollback.len() + self.lines.len()
    }

    fn display_top(&self) -> usize {
        self.scrollback.len().saturating_sub(self.viewport_offset)
    }

    fn logical_line(&self, index: usize) -> Option<&Line> {
        if index < self.scrollback.len() {
            self.scrollback.get(index)
        } else {
            self.lines.get(index - self.scrollback.len())
        }
    }

    fn display_point(&self, row: usize, column: usize) -> GridPoint {
        GridPoint {
            line: (self.display_top() + row.min(self.rows - 1)).min(self.logical_line_count() - 1),
            column: column.min(self.columns - 1),
        }
    }

    fn reveal_line(&mut self, line: usize) {
        let history = self.scrollback.len();
        self.viewport_offset = if line < history { history - line } else { 0 };
    }

    fn push_scrollback(&mut self, line: Line) {
        if self.viewport_offset > 0 {
            self.viewport_offset += 1;
        }
        self.scrollback.push_back(line);
        self.trim_scrollback();
    }

    fn trim_scrollback(&mut self) {
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
            self.viewport_offset = self.viewport_offset.saturating_sub(1);
            if let Some(selection) = &mut self.selection {
                selection.start.line = selection.start.line.saturating_sub(1);
                selection.end.line = selection.end.line.saturating_sub(1);
            }
        }
    }
}

fn blank_line(columns: usize, attributes: Attributes) -> Line {
    vec![Cell::blank(attributes); columns]
}

fn tab_stops(columns: usize) -> Vec<bool> {
    (0..columns)
        .map(|column| column != 0 && column % 8 == 0)
        .collect()
}

fn resize_line(line: &mut Line, columns: usize) {
    line.resize(columns, Cell::default());
    line.truncate(columns);
    normalize_wide(line);
}

fn normalize_wide(line: &mut Line) {
    for column in 0..line.len() {
        match line[column].width {
            CellWidth::Wide
                if column + 1 >= line.len()
                    || line[column + 1].width != CellWidth::Continuation =>
            {
                line[column] = Cell::default();
            }
            CellWidth::Continuation if column == 0 || line[column - 1].width != CellWidth::Wide => {
                line[column] = Cell::default();
            }
            _ => {}
        }
    }
}

fn line_text(line: &[Cell]) -> String {
    let mut text = String::new();
    for cell in line {
        if cell.width != CellWidth::Continuation {
            text.push_str(&cell.text);
        }
    }
    text
}
