//! Text-input editing helpers (cursor movement, insertion, deletion).

use super::App;

impl App {
    /// Insert an ASCII character at the current cursor position.
    pub(crate) fn insert_char(&mut self, ch: char) {
        if !ch.is_ascii() {
            return;
        }
        self.input.insert(self.cursor, ch);
        self.cursor = (self.cursor + 1).min(self.input.len());
    }

    /// Delete the character before the cursor.
    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        self.input.remove(self.cursor);
    }

    /// Delete the character at the cursor.
    pub(crate) fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        self.input.remove(self.cursor);
    }

    /// Move the cursor one position to the left.
    pub(crate) fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move the cursor one position to the right.
    pub(crate) fn move_cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    /// Move the cursor to the beginning of the input.
    pub(crate) fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the end of the input.
    pub(crate) fn move_cursor_end(&mut self) {
        self.cursor = self.input.len();
    }
}
