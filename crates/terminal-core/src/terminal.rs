use std::sync::Arc;

use vte::{Params, Parser, Perform};

use crate::{Attributes, Color, CursorShape, Rgb, Screen, SearchMatch};

pub struct Terminal {
    parser: Parser,
    state: TerminalState,
}

struct TerminalState {
    primary: Screen,
    alternate: Screen,
    alternate_active: bool,
    auto_wrap: bool,
    insert_mode: bool,
    origin_mode: bool,
    application_cursor: bool,
    bracketed_paste: bool,
    title: String,
    title_generation: u64,
    working_directory_uri: Option<String>,
    pending_response: Vec<u8>,
}

impl TerminalState {
    fn screen(&self) -> &Screen {
        if self.alternate_active {
            &self.alternate
        } else {
            &self.primary
        }
    }

    fn screen_mut(&mut self) -> &mut Screen {
        if self.alternate_active {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    fn switch_alternate(&mut self, enabled: bool, clear: bool) {
        if enabled == self.alternate_active {
            return;
        }
        if enabled {
            self.primary.save_cursor();
            if clear {
                self.alternate.reset();
            }
            self.alternate_active = true;
        } else {
            self.alternate_active = false;
            self.primary.restore_cursor();
        }
    }

    fn reset(&mut self) {
        let columns = self.primary.columns();
        let rows = self.primary.rows();
        self.primary = Screen::new(columns, rows, true);
        self.alternate = Screen::new(columns, rows, false);
        self.alternate_active = false;
        self.auto_wrap = true;
        self.insert_mode = false;
        self.origin_mode = false;
        self.application_cursor = false;
        self.bracketed_paste = false;
    }
}

impl Terminal {
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            parser: Parser::new(),
            state: TerminalState {
                primary: Screen::new(columns, rows, true),
                alternate: Screen::new(columns, rows, false),
                alternate_active: false,
                auto_wrap: true,
                insert_mode: false,
                origin_mode: false,
                application_cursor: false,
                bracketed_paste: false,
                title: String::new(),
                title_generation: 0,
                working_directory_uri: None,
                pending_response: Vec::new(),
            },
        }
    }

    pub fn screen(&self) -> &Screen {
        self.state.screen()
    }

    pub fn screen_mut(&mut self) -> &mut Screen {
        self.state.screen_mut()
    }

    pub fn title(&self) -> &str {
        &self.state.title
    }

    pub fn title_generation(&self) -> u64 {
        self.state.title_generation
    }

    pub fn working_directory_uri(&self) -> Option<&str> {
        self.state.working_directory_uri.as_deref()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.state.bracketed_paste
    }

    pub fn application_cursor(&self) -> bool {
        self.state.application_cursor
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.state.alternate_active
    }

    pub fn set_cursor_style(&mut self, shape: CursorShape) {
        self.state.screen_mut().set_cursor_style(shape, false);
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.state.primary.resize(columns, rows);
        self.state.alternate.resize(columns, rows);
    }

    pub fn process(&mut self, bytes: &[u8]) {
        let Self { parser, state } = self;
        parser.advance(&mut TerminalPerformer { state }, bytes);
    }

    pub fn take_response(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.state.pending_response)
    }

    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        let sanitized = text
            .replace('\0', "")
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if self.bracketed_paste() {
            let mut bytes = Vec::with_capacity(sanitized.len() + 12);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(sanitized.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            bytes
        } else {
            sanitized.into_bytes()
        }
    }

    pub fn search_scrollback(&mut self, query: &str) -> Option<SearchMatch> {
        self.state.primary.search(query)
    }
}

struct TerminalPerformer<'a> {
    state: &'a mut TerminalState,
}

impl TerminalPerformer<'_> {
    fn param(params: &Params, index: usize, default: u16) -> u16 {
        params
            .iter()
            .nth(index)
            .and_then(|values| values.first().copied())
            .filter(|value| *value != 0)
            .unwrap_or(default)
    }

    fn private(intermediates: &[u8]) -> bool {
        intermediates.contains(&b'?')
    }

    fn apply_sgr(&mut self, params: &Params) {
        let values: Vec<u16> = params
            .iter()
            .flat_map(|item| item.iter().copied())
            .collect();
        let values = if values.is_empty() { vec![0] } else { values };
        let mut index = 0;
        while index < values.len() {
            let value = values[index];
            let attributes = self.state.screen_mut().attributes_mut();
            match value {
                0 => *attributes = Attributes::default(),
                1 => attributes.bold = true,
                2 => attributes.dim = true,
                3 => attributes.italic = true,
                4 | 21 => attributes.underline = true,
                5 | 6 => attributes.blink = true,
                7 => attributes.inverse = true,
                8 => attributes.hidden = true,
                9 => attributes.strikethrough = true,
                22 => {
                    attributes.bold = false;
                    attributes.dim = false;
                }
                23 => attributes.italic = false,
                24 => attributes.underline = false,
                25 => attributes.blink = false,
                27 => attributes.inverse = false,
                28 => attributes.hidden = false,
                29 => attributes.strikethrough = false,
                30..=37 => attributes.foreground = Color::Indexed((value - 30) as u8),
                39 => attributes.foreground = Color::Default,
                40..=47 => attributes.background = Color::Indexed((value - 40) as u8),
                49 => attributes.background = Color::Default,
                90..=97 => attributes.foreground = Color::Indexed((value - 90 + 8) as u8),
                100..=107 => attributes.background = Color::Indexed((value - 100 + 8) as u8),
                38 | 48 => {
                    let color = if values.get(index + 1) == Some(&5) {
                        values
                            .get(index + 2)
                            .map(|value| (Color::Indexed((*value).min(255) as u8), 2))
                    } else if values.get(index + 1) == Some(&2) && index + 4 < values.len() {
                        Some((
                            Color::Rgb(Rgb::new(
                                values[index + 2].min(255) as u8,
                                values[index + 3].min(255) as u8,
                                values[index + 4].min(255) as u8,
                            )),
                            4,
                        ))
                    } else {
                        None
                    };
                    if let Some((color, consumed)) = color {
                        if value == 38 {
                            attributes.foreground = color;
                        } else {
                            attributes.background = color;
                        }
                        index += consumed;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn set_mode(&mut self, params: &Params, private: bool, enabled: bool) {
        let values: Vec<u16> = params
            .iter()
            .filter_map(|values| values.first().copied())
            .collect();
        for value in values {
            if private {
                match value {
                    1 => self.state.application_cursor = enabled,
                    6 => {
                        self.state.origin_mode = enabled;
                        self.state.screen_mut().set_cursor(0, 0, enabled);
                    }
                    7 => self.state.auto_wrap = enabled,
                    25 => self.state.screen_mut().set_cursor_visible(enabled),
                    47 | 1047 => self.state.switch_alternate(enabled, enabled),
                    1049 => self.state.switch_alternate(enabled, true),
                    2004 => self.state.bracketed_paste = enabled,
                    _ => {}
                }
            } else if value == 4 {
                self.state.insert_mode = enabled;
            }
        }
    }
}

impl Perform for TerminalPerformer<'_> {
    fn print(&mut self, character: char) {
        let auto_wrap = self.state.auto_wrap;
        let insert_mode = self.state.insert_mode;
        self.state
            .screen_mut()
            .put(character, auto_wrap, insert_mode);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.state.screen_mut().line_feed(),
            b'\r' => self.state.screen_mut().carriage_return(),
            0x08 => self.state.screen_mut().backspace(),
            b'\t' => self.state.screen_mut().tab(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let amount = Self::param(params, 0, 1) as usize;
        let private = Self::private(intermediates);
        match action {
            'A' => self.state.screen_mut().move_cursor(-(amount as isize), 0),
            'B' => self.state.screen_mut().move_cursor(amount as isize, 0),
            'C' | 'a' => self.state.screen_mut().move_cursor(0, amount as isize),
            'D' => self.state.screen_mut().move_cursor(0, -(amount as isize)),
            'E' => {
                self.state.screen_mut().move_cursor(amount as isize, 0);
                self.state.screen_mut().carriage_return();
            }
            'F' => {
                self.state.screen_mut().move_cursor(-(amount as isize), 0);
                self.state.screen_mut().carriage_return();
            }
            'G' | '`' => self
                .state
                .screen_mut()
                .set_cursor_column(amount.saturating_sub(1)),
            'd' => {
                let origin = self.state.origin_mode;
                self.state
                    .screen_mut()
                    .set_cursor_row(amount.saturating_sub(1), origin);
            }
            'H' | 'f' => {
                let origin = self.state.origin_mode;
                let column = Self::param(params, 1, 1) as usize;
                self.state.screen_mut().set_cursor(
                    amount.saturating_sub(1),
                    column.saturating_sub(1),
                    origin,
                );
            }
            'J' => self
                .state
                .screen_mut()
                .erase_display(Self::param(params, 0, 0)),
            'K' => self
                .state
                .screen_mut()
                .erase_line(Self::param(params, 0, 0)),
            '@' => self.state.screen_mut().insert_cells(amount),
            'P' => self.state.screen_mut().delete_cells(amount),
            'X' => self.state.screen_mut().erase_cells(amount),
            'L' => self.state.screen_mut().insert_lines(amount),
            'M' => self.state.screen_mut().delete_lines(amount),
            'S' => self.state.screen_mut().scroll_up(amount),
            'T' => self.state.screen_mut().scroll_down(amount),
            'r' if !private => {
                let rows = self.state.screen().rows();
                let top = Self::param(params, 0, 1) as usize;
                let bottom = Self::param(params, 1, rows as u16) as usize;
                self.state
                    .screen_mut()
                    .set_scroll_region(top.saturating_sub(1), bottom.min(rows));
            }
            'g' => self
                .state
                .screen_mut()
                .clear_tab_stop(Self::param(params, 0, 0) == 3),
            'm' => self.apply_sgr(params),
            's' => self.state.screen_mut().save_cursor(),
            'u' => self.state.screen_mut().restore_cursor(),
            'h' => self.set_mode(params, private, true),
            'l' => self.set_mode(params, private, false),
            'n' if !private && Self::param(params, 0, 0) == 5 => {
                self.state.pending_response.extend_from_slice(b"\x1b[0n")
            }
            'n' if !private && Self::param(params, 0, 0) == 6 => {
                let cursor = self.state.screen().cursor();
                self.state.pending_response.extend_from_slice(
                    format!("\x1b[{};{}R", cursor.row + 1, cursor.column + 1).as_bytes(),
                );
            }
            'c' => self.state.pending_response.extend_from_slice(b"\x1b[?1;2c"),
            'q' if intermediates.contains(&b' ') => {
                let (shape, blinking) = match Self::param(params, 0, 0) {
                    0 | 1 => (CursorShape::Block, true),
                    2 => (CursorShape::Block, false),
                    3 => (CursorShape::Underline, true),
                    4 => (CursorShape::Underline, false),
                    5 => (CursorShape::Bar, true),
                    6 => (CursorShape::Bar, false),
                    _ => return,
                };
                self.state.screen_mut().set_cursor_style(shape, blinking);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        match (intermediates, byte) {
            (_, b'7') => self.state.screen_mut().save_cursor(),
            (_, b'8') => self.state.screen_mut().restore_cursor(),
            (_, b'D') => self.state.screen_mut().line_feed(),
            (_, b'E') => {
                self.state.screen_mut().line_feed();
                self.state.screen_mut().carriage_return();
            }
            (_, b'M') => self.state.screen_mut().reverse_index(),
            (_, b'H') => self.state.screen_mut().set_tab_stop(),
            (_, b'c') => self.state.reset(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(command) = params
            .first()
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            return;
        };
        match command {
            "0" | "2" => {
                let title = params
                    .get(1)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .unwrap_or_default();
                self.state.title.clear();
                self.state.title.push_str(title);
                self.state.title_generation = self.state.title_generation.wrapping_add(1);
            }
            "8" => {
                let uri = params
                    .get(2)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .unwrap_or_default();
                let link = (!uri.is_empty()).then(|| Arc::<str>::from(uri));
                self.state.screen_mut().set_active_hyperlink(link);
            }
            "7" => {
                self.state.working_directory_uri = params
                    .get(1)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .filter(|value| value.starts_with("file://"))
                    .map(str::to_owned);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellWidth, GridPoint};

    fn row_text(screen: &Screen, row: usize) -> String {
        screen
            .display_line(row)
            .unwrap()
            .iter()
            .filter(|cell| cell.width != CellWidth::Continuation)
            .map(|cell| cell.text.as_str())
            .collect()
    }

    #[test]
    fn parses_text_newlines_and_cursor_motion() {
        let mut terminal = Terminal::new(8, 3);
        terminal.process(b"one\r\ntwo\x1b[1A\x1b[4GX");
        assert_eq!(row_text(terminal.screen(), 0), "oneX    ");
        assert_eq!(row_text(terminal.screen(), 1), "two     ");
    }

    #[test]
    fn clusters_combining_and_wide_graphemes() {
        let mut terminal = Terminal::new(8, 2);
        terminal.process("e\u{301}界🇦🇲".as_bytes());
        assert_eq!(terminal.screen().cell(0, 0).unwrap().text, "e\u{301}");
        assert_eq!(terminal.screen().cell(0, 1).unwrap().width, CellWidth::Wide);
        assert_eq!(
            terminal.screen().cell(0, 2).unwrap().width,
            CellWidth::Continuation
        );
        assert_eq!(terminal.screen().cell(0, 3).unwrap().text, "🇦🇲");
        assert_eq!(terminal.screen().cell(0, 3).unwrap().width, CellWidth::Wide);
    }

    #[test]
    fn scrolling_retains_searchable_history() {
        let mut terminal = Terminal::new(6, 2);
        terminal.process(b"alpha\r\nbeta\r\ngamma");
        assert_eq!(terminal.screen().scrollback_len(), 1);
        assert_eq!(
            terminal.search_scrollback("alpha").unwrap().start,
            GridPoint { line: 0, column: 0 }
        );
        assert!(terminal.screen().viewport_offset() > 0);
    }

    #[test]
    fn scrolling_region_does_not_enter_scrollback() {
        let mut terminal = Terminal::new(4, 4);
        terminal.process(b"1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1HX\nY");
        assert_eq!(terminal.screen().scrollback_len(), 0);
        assert_eq!(row_text(terminal.screen(), 0), "1   ");
        assert_eq!(row_text(terminal.screen(), 3), "4   ");
    }

    #[test]
    fn alternate_screen_restores_primary_content_and_cursor() {
        let mut terminal = Terminal::new(8, 2);
        terminal.process(b"primary\x1b[?1049halternate\x1b[?1049l");
        assert!(!terminal.alternate_screen_active());
        assert_eq!(row_text(terminal.screen(), 0), "primary ");
        assert_eq!(terminal.screen().cursor().column, 7);
    }

    #[test]
    fn insert_delete_and_erase_cells() {
        let mut terminal = Terminal::new(8, 1);
        terminal.process(b"abcdef\x1b[1;3H\x1b[2@XY\x1b[1P\x1b[2X");
        assert_eq!(row_text(terminal.screen(), 0), "abXY  f ");
    }

    #[test]
    fn parses_indexed_truecolor_and_styles() {
        let mut terminal = Terminal::new(4, 1);
        terminal.process(b"\x1b[1;3;4;38;5;123;48;2;1;2;3mX");
        let attributes = terminal.screen().cell(0, 0).unwrap().attributes;
        assert!(attributes.bold && attributes.italic && attributes.underline);
        assert_eq!(attributes.foreground, Color::Indexed(123));
        assert_eq!(attributes.background, Color::Rgb(Rgb::new(1, 2, 3)));
    }

    #[test]
    fn supports_title_hyperlink_and_bracketed_paste() {
        let mut terminal = Terminal::new(8, 1);
        terminal.process(b"\x1b]2;work\x07\x1b]8;;https://example.com\x07x\x1b]8;;\x07\x1b[?2004h");
        assert_eq!(terminal.title(), "work");
        assert_eq!(
            terminal.screen().cell(0, 0).unwrap().hyperlink.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(terminal.paste_bytes("a\r\nb"), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn tracks_osc7_working_directory_without_affecting_screen() {
        let mut terminal = Terminal::new(20, 2);
        terminal.process(b"\x1b]7;file://localhost/tmp/project%20one\x07");
        assert_eq!(
            terminal.working_directory_uri(),
            Some("file://localhost/tmp/project%20one")
        );
        assert!(terminal.screen().cell(0, 0).unwrap().is_blank());
    }

    #[test]
    fn copy_selection_spans_scrollback_and_screen() {
        let mut terminal = Terminal::new(5, 2);
        terminal.process(b"one\r\ntwo\r\ntri");
        terminal.screen_mut().scroll_viewport(1);
        terminal.screen_mut().begin_selection(0, 0);
        terminal.screen_mut().update_selection(1, 2);
        assert_eq!(
            terminal.screen().selected_text().as_deref(),
            Some("one\ntwo")
        );
    }

    #[test]
    fn responds_to_status_and_cursor_reports() {
        let mut terminal = Terminal::new(8, 2);
        terminal.process(b"abc\x1b[5n\x1b[6n\x1b[c");
        assert_eq!(terminal.take_response(), b"\x1b[0n\x1b[1;4R\x1b[?1;2c");
    }
}
