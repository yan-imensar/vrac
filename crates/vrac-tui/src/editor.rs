//! Inline editor state and terminal-width-aware caret movement.

use unicode_width::UnicodeWidthChar;
use vrac::{NodeId, Placement, ReferenceInput};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EditTarget {
    Existing(NodeId),
    New {
        parent_id: Option<NodeId>,
        placement: Placement,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Editor {
    pub(super) target: EditTarget,
    pub(super) text: String,
    pub(super) cursor: usize,
    pub(super) references: Vec<ReferenceInput>,
    pub(super) tags: Vec<String>,
}

impl Editor {
    pub(super) fn new(
        target: EditTarget,
        text: String,
        references: Vec<ReferenceInput>,
        tags: Vec<String>,
    ) -> Self {
        let cursor = text.chars().count();
        Self {
            target,
            text,
            cursor,
            references,
            tags,
        }
    }

    pub(super) fn empty(target: EditTarget) -> Self {
        Self::new(target, String::new(), Vec::new(), Vec::new())
    }

    pub(super) fn insert(&mut self, character: char) {
        let byte = char_to_byte(&self.text, self.cursor);
        let added = character.len_utf8();
        self.references.retain_mut(|reference| {
            let token_start = reference.label_start.saturating_sub(2);
            let token_end = reference.label_end.saturating_add(2);
            if byte > token_start && byte < token_end {
                return false;
            }
            if byte <= token_start {
                reference.label_start += added;
                reference.label_end += added;
            }
            true
        });
        self.text.insert(byte, character);
        self.cursor += 1;
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor - 1);
        let end = char_to_byte(&self.text, self.cursor);
        self.remove_range(start, end);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor == self.text.chars().count() {
            return;
        }
        let start = char_to_byte(&self.text, self.cursor);
        let end = char_to_byte(&self.text, self.cursor + 1);
        self.remove_range(start, end);
        self.text.replace_range(start..end, "");
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        for character in text.chars() {
            self.insert(character);
        }
    }

    pub(super) fn move_word(&mut self, direction: isize) {
        let characters = self.text.chars().collect::<Vec<_>>();
        if direction < 0 {
            while self.cursor > 0 && characters[self.cursor - 1].is_whitespace() {
                self.cursor -= 1;
            }
            if self.cursor == 0 {
                return;
            }
            let word = word_character(characters[self.cursor - 1]);
            while self.cursor > 0
                && !characters[self.cursor - 1].is_whitespace()
                && word_character(characters[self.cursor - 1]) == word
            {
                self.cursor -= 1;
            }
        } else {
            while self.cursor < characters.len() && characters[self.cursor].is_whitespace() {
                self.cursor += 1;
            }
            if self.cursor == characters.len() {
                return;
            }
            let word = word_character(characters[self.cursor]);
            while self.cursor < characters.len()
                && !characters[self.cursor].is_whitespace()
                && word_character(characters[self.cursor]) == word
            {
                self.cursor += 1;
            }
        }
    }

    pub(super) fn backspace_word(&mut self) {
        let end = self.cursor;
        self.move_word(-1);
        let start = self.cursor;
        if start == end {
            return;
        }
        let start_byte = char_to_byte(&self.text, start);
        let end_byte = char_to_byte(&self.text, end);
        self.remove_range(start_byte, end_byte);
        self.text.replace_range(start_byte..end_byte, "");
    }

    pub(super) fn move_vertical(&mut self, direction: isize, width: usize) -> bool {
        let positions = caret_positions(&self.text, width);
        let (row, column) = positions[self.cursor];
        let target_row = row.saturating_add_signed(direction);
        if target_row == row {
            return false;
        }
        if let Some((index, _)) = positions
            .iter()
            .enumerate()
            .filter(|(_, (candidate_row, _))| *candidate_row == target_row)
            .min_by_key(|(index, (_, candidate_column))| {
                (
                    candidate_column.abs_diff(column),
                    index.abs_diff(self.cursor),
                )
            })
        {
            self.cursor = index;
            return true;
        }
        false
    }

    pub(super) fn move_to_visual_edge(&mut self, end: bool, width: usize) {
        let positions = caret_positions(&self.text, width);
        let row = positions[self.cursor].0;
        let mut candidates = positions
            .iter()
            .enumerate()
            .filter(|(_, (candidate_row, _))| *candidate_row == row);
        if let Some((index, _)) = if end {
            candidates.next_back()
        } else {
            candidates.next()
        } {
            self.cursor = index;
        }
    }

    fn remove_range(&mut self, start: usize, end: usize) {
        let removed = end - start;
        self.references.retain_mut(|reference| {
            let token_start = reference.label_start.saturating_sub(2);
            let token_end = reference.label_end.saturating_add(2);
            if start < token_end && end > token_start {
                return false;
            }
            if end <= token_start {
                reference.label_start -= removed;
                reference.label_end -= removed;
            }
            true
        });
    }
}

fn word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn shown_width(character: char) -> usize {
    UnicodeWidthChar::width(if character.is_control() {
        '↵'
    } else {
        character
    })
    .unwrap_or(0)
}

fn caret_positions(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let characters = text.chars().collect::<Vec<_>>();
    let mut positions = Vec::with_capacity(characters.len() + 1);
    let mut row = 0;
    let mut column = 0;
    for index in 0..=characters.len() {
        let next_width = characters.get(index).copied().map(shown_width);
        if column == width && column > 0
            || next_width.is_some_and(|next| column > 0 && column + next > width)
        {
            row += 1;
            column = 0;
        }
        positions.push((row, column));
        if let Some(next_width) = next_width {
            column += next_width;
        }
    }
    positions
}

pub(super) fn char_to_byte(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(byte, _)| byte)
}
