// ── EditorBuffer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct EditorBuffer {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

impl EditorBuffer {
    pub const TAB_WIDTH: usize = 4;

    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    pub fn from_text(text: String) -> Self {
        let mut lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
        if text.ends_with('\n') {
            lines.push(String::new());
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    pub fn to_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn set_cursor_to_end(&mut self) {
        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
    }

    pub fn insert_char(&mut self, c: char) {
        let idx = self.byte_idx_at_cursor();
        self.lines[self.cursor_row].insert(idx, c);
        self.cursor_col += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.insert_newline(),
                '\r' => {} // skip CR from CRLF clipboard content
                c => self.insert_char(c),
            }
        }
    }

    pub fn insert_pasted_str(&mut self, s: &str) {
        let normalized = normalize_pasted_text(s, self.cursor_col, Self::TAB_WIDTH);
        self.insert_str(&normalized);
    }

    pub fn insert_newline(&mut self) {
        let idx = self.byte_idx_at_cursor();
        let tail = self.lines[self.cursor_row].split_off(idx);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_row, tail);
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let start = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col - 1);
            let end = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col);
            self.lines[self.cursor_row].replace_range(start..end, "");
            self.cursor_col -= 1;
            return;
        }

        if self.cursor_row > 0 {
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_len();
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    pub fn delete(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            let start = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col);
            let end = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col + 1);
            self.lines[self.cursor_row].replace_range(start..end, "");
            return;
        }

        if self.cursor_row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_len();
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_col < self.current_line_len() {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    pub fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor_col = self.current_line_len();
    }

    pub fn current_line_len(&self) -> usize {
        self.lines[self.cursor_row].chars().count()
    }

    pub fn byte_idx_at_cursor(&self) -> usize {
        byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col)
    }

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    pub fn move_word_left(&mut self) {
        // Skip trailing whitespace/punctuation
        while self.cursor_col > 0 {
            let idx = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col - 1);
            let c = if idx < self.lines[self.cursor_row].len() {
                self.lines[self.cursor_row][idx..].chars().next().unwrap_or(' ')
            } else {
                ' '
            };
            if Self::is_word_char(c) {
                break;
            }
            self.cursor_col -= 1;
        }
        // Move to beginning of word
        while self.cursor_col > 0 {
            let idx = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col - 1);
            let c = if idx < self.lines[self.cursor_row].len() {
                self.lines[self.cursor_row][idx..].chars().next().unwrap_or(' ')
            } else {
                ' '
            };
            if !Self::is_word_char(c) {
                break;
            }
            self.cursor_col -= 1;
        }
        if self.cursor_col == 0 && self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_len();
        }
    }

    pub fn move_word_right(&mut self) {
        let line_len = self.current_line_len();
        // Exit word
        while self.cursor_col < line_len {
            let idx = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col);
            let c = if idx < self.lines[self.cursor_row].len() {
                self.lines[self.cursor_row][idx..].chars().next().unwrap_or(' ')
            } else {
                ' '
            };
            if !Self::is_word_char(c) {
                break;
            }
            self.cursor_col += 1;
        }
        // Skip separators
        while self.cursor_col < line_len {
            let idx = byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col);
            let c = if idx < self.lines[self.cursor_row].len() {
                self.lines[self.cursor_row][idx..].chars().next().unwrap_or(' ')
            } else {
                ' '
            };
            if Self::is_word_char(c) {
                break;
            }
            self.cursor_col += 1;
        }
        if self.cursor_col >= line_len && self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }
}

// ── byte index helpers ─────────────────────────────────────────────────────────

/// Convert a 0-based character index in `s` to a byte index.
fn byte_idx_from_char_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

// ── paste normalization ────────────────────────────────────────────────────────

/// Normalize pasted text: expand tabs relative to `start_col`, strip bare CR.
pub fn normalize_pasted_text(text: &str, start_col: usize, tab_width: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut col = start_col;

    for ch in text.chars() {
        match ch {
            '\n' | '\r' => {
                if ch == '\n' {
                    out.push('\n');
                }
                col = 0;
            }
            '\t' => {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    out.push(' ');
                }
                col += spaces;
            }
            _ => {
                out.push(ch);
                col += 1;
            }
        }
    }

    out
}
