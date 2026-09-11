use crate::action::{Action, ActionInfo, match_actions};

pub const VISIBLE_ROWS: usize = 9;

#[derive(Default)]
pub struct CommandPaletteState {
    pub query: String,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl CommandPaletteState {
    pub fn matches(&self) -> Vec<&'static ActionInfo> {
        match_actions(&self.query)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.matches().len();
        if count == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, count as isize - 1) as usize;
        self.scroll_offset = self.scroll_offset.min(self.selected);
        if self.selected >= self.scroll_offset + VISIBLE_ROWS {
            self.scroll_offset = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    pub fn selected_action(&self) -> Option<Action> {
        self.matches().get(self.selected).map(|info| info.action)
    }

    pub fn edit(&mut self, text: Option<&str>, backspace: bool) {
        if backspace {
            self.query.pop();
        }
        if let Some(text) = text {
            self.query.push_str(text);
        }
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub fn scroll(&mut self, delta: isize) {
        let count = self.matches().len();
        let maximum = count.saturating_sub(VISIBLE_ROWS);
        self.scroll_offset =
            (self.scroll_offset as isize + delta).clamp(0, maximum as isize) as usize;
        self.selected = self.selected.clamp(
            self.scroll_offset,
            (self.scroll_offset + VISIBLE_ROWS - 1).min(count.saturating_sub(1)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_scroll_are_clamped_to_matches() {
        let mut palette = CommandPaletteState::default();
        palette.move_selection(100);
        assert!(palette.selected < palette.matches().len());
        assert!(palette.selected < palette.scroll_offset + VISIBLE_ROWS);
        palette.scroll(100);
        assert!(palette.scroll_offset <= palette.matches().len().saturating_sub(VISIBLE_ROWS));
        palette.edit(Some("no matching command"), false);
        assert_eq!(palette.selected, 0);
        assert_eq!(palette.scroll_offset, 0);
        assert_eq!(palette.selected_action(), None);
    }
}
