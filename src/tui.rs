use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span as TSpan, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use harper_core::linting::{LintGroup, Linter};
use harper_core::parsers::PlainEnglish;
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

use crate::config::AppConfig;
use crate::storage::{NoteSummary, Store};

// arboard is optional at runtime (no display server): treat errors as no-op.
use arboard::Clipboard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Edit,
    Search,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VimMode {
    Normal,
    Insert,
    Visual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeName {
    NeoNoir,
    Paper,
    Matrix,
}

impl ThemeName {
    fn next(self) -> Self {
        match self {
            Self::NeoNoir => Self::Paper,
            Self::Paper => Self::Matrix,
            Self::Matrix => Self::NeoNoir,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NeoNoir => "neo-noir",
            Self::Paper => "paper",
            Self::Matrix => "matrix",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "neo-noir" | "neonoir" | "neo" => Some(Self::NeoNoir),
            "paper" => Some(Self::Paper),
            "matrix" => Some(Self::Matrix),
            _ => None,
        }
    }

    fn palette(self) -> Palette {
        match self {
            Self::NeoNoir => Palette {
                bg: Color::Rgb(12, 14, 18),
                panel: Color::Rgb(18, 23, 31),
                text: Color::Rgb(226, 232, 240),
                muted: Color::Rgb(128, 142, 160),
                accent: Color::Rgb(56, 189, 248),
                danger: Color::Rgb(248, 113, 113),
                ok: Color::Rgb(74, 222, 128),
            },
            Self::Paper => Palette {
                bg: Color::Rgb(246, 242, 230),
                panel: Color::Rgb(255, 252, 245),
                text: Color::Rgb(31, 41, 55),
                muted: Color::Rgb(107, 114, 128),
                accent: Color::Rgb(185, 28, 28),
                danger: Color::Rgb(153, 27, 27),
                ok: Color::Rgb(21, 128, 61),
            },
            Self::Matrix => Palette {
                bg: Color::Rgb(4, 16, 10),
                panel: Color::Rgb(8, 28, 16),
                text: Color::Rgb(166, 255, 181),
                muted: Color::Rgb(69, 140, 83),
                accent: Color::Rgb(52, 211, 153),
                danger: Color::Rgb(248, 113, 113),
                ok: Color::Rgb(134, 239, 172),
            },
        }
    }

    // (bg, fg) pairs for tag pills — one per theme, hashed by tag name
    fn tag_pill_colors(self) -> &'static [(Color, Color)] {
        const BG: Color = Color::Rgb(12, 14, 18);   // NeoNoir bg
        const PG: Color = Color::Rgb(246, 242, 230); // Paper bg
        const MG: Color = Color::Rgb(4, 16, 10);    // Matrix bg
        match self {
            Self::NeoNoir => &[
                (Color::Rgb(56, 189, 248),  BG), // sky
                (Color::Rgb(167, 139, 250), BG), // violet
                (Color::Rgb(74, 222, 128),  BG), // green
                (Color::Rgb(251, 146, 60),  BG), // orange
                (Color::Rgb(244, 114, 182), BG), // pink
                (Color::Rgb(250, 204, 21),  BG), // yellow
            ],
            Self::Paper => &[
                (Color::Rgb(185, 28, 28),   PG), // red
                (Color::Rgb(29, 78, 216),   PG), // blue
                (Color::Rgb(21, 128, 61),   PG), // green
                (Color::Rgb(194, 65, 12),   PG), // orange
                (Color::Rgb(126, 34, 206),  PG), // purple
                (Color::Rgb(15, 118, 110),  PG), // teal
            ],
            Self::Matrix => &[
                (Color::Rgb(52, 211, 153),  MG), // teal
                (Color::Rgb(34, 211, 238),  MG), // cyan
                (Color::Rgb(163, 230, 53),  MG), // lime
                (Color::Rgb(96, 165, 250),  MG), // blue
                (Color::Rgb(244, 114, 182), MG), // pink
                (Color::Rgb(167, 139, 250), MG), // purple
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeymapPreset {
    Default,
    Vim,
}

impl KeymapPreset {
    fn next(self) -> Self {
        match self {
            Self::Default => Self::Vim,
            Self::Vim => Self::Default,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Vim => "vim",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "vim" => Some(Self::Vim),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Density {
    Cozy,
    Compact,
}

impl Density {
    fn toggle(self) -> Self {
        match self {
            Self::Cozy => Self::Compact,
            Self::Compact => Self::Cozy,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cozy => "cozy",
            Self::Compact => "compact",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "cozy" => Some(Self::Cozy),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Palette {
    bg: Color,
    panel: Color,
    text: Color,
    muted: Color,
    accent: Color,
    danger: Color,
    ok: Color,
}

#[derive(Debug, Clone)]
struct EditorBuffer {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

impl EditorBuffer {
    const TAB_WIDTH: usize = 4;

    fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    fn from_text(text: String) -> Self {
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

    fn to_text(&self) -> String {
        self.lines.join("\n")
    }

    fn set_cursor_to_end(&mut self) {
        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
    }

    fn insert_char(&mut self, c: char) {
        let idx = self.byte_idx_at_cursor();
        self.lines[self.cursor_row].insert(idx, c);
        self.cursor_col += 1;
    }

    fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.insert_newline(),
                '\r' => {} // skip CR from CRLF clipboard content
                c => self.insert_char(c),
            }
        }
    }

    fn insert_pasted_str(&mut self, s: &str) {
        let normalized = normalize_pasted_text(s, self.cursor_col, Self::TAB_WIDTH);
        self.insert_str(&normalized);
    }

    fn insert_newline(&mut self) {
        let idx = self.byte_idx_at_cursor();
        let tail = self.lines[self.cursor_row].split_off(idx);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_row, tail);
    }

    fn open_line_below(&mut self) {
        self.move_end();
        self.insert_newline();
    }

    fn open_line_above(&mut self) {
        self.lines.insert(self.cursor_row, String::new());
        self.cursor_col = 0;
    }

    fn backspace(&mut self) {
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

    fn delete(&mut self) {
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

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_len();
        }
    }

    fn move_right(&mut self) {
        if self.cursor_col < self.current_line_len() {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    fn move_end(&mut self) {
        self.cursor_col = self.current_line_len();
    }

    fn current_line_len(&self) -> usize {
        self.lines[self.cursor_row].chars().count()
    }

    fn byte_idx_at_cursor(&self) -> usize {
        byte_idx_from_char_idx(&self.lines[self.cursor_row], self.cursor_col)
    }
}

fn byte_idx_from_char_idx(s: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }

    let len = s.chars().count();
    if char_idx >= len {
        return s.len();
    }

    s.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

fn normalize_pasted_text(text: &str, start_col: usize, tab_width: usize) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut col = start_col;

    for ch in text.chars() {
        match ch {
            '\r' => {}
            '\n' => {
                normalized.push('\n');
                col = 0;
            }
            '\t' => {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    normalized.push(' ');
                }
                col += spaces;
            }
            _ => {
                normalized.push(ch);
                col += 1;
            }
        }
    }

    normalized
}

fn is_ctrl_char(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(c)
}

pub struct App {
    store: Store,
    notes: Vec<NoteSummary>,
    selected: usize,
    active_note_id: Option<i64>,
    editor_buffer: EditorBuffer,
    query: String,
    search_input: String,
    command_input: String,
    mode: Mode,
    vim_mode: VimMode,
    status: String,
    dirty: bool,
    theme: ThemeName,
    keymap: KeymapPreset,
    density: Density,
    config_path: PathBuf,
    linter: LintGroup,
    lints: Vec<harper_core::linting::Lint>,
    lints_active: bool,
    last_edit: Option<Instant>,
    delete_pending: bool,
    editor_col_width: usize,
    selection_anchor: Option<usize>,
    yank_buffer: String,
    clipboard: Option<Clipboard>,
}

impl App {
    pub fn new(store: Store) -> Result<Self> {
        let (config, config_path) = AppConfig::load_default()?;

        let mut app = Self {
            store,
            notes: Vec::new(),
            selected: 0,
            active_note_id: None,
            editor_buffer: EditorBuffer::new(),
            query: String::new(),
            search_input: String::new(),
            command_input: String::new(),
            mode: Mode::Normal,
            vim_mode: VimMode::Insert,
            status: "Ready".to_string(),
            dirty: false,
            theme: ThemeName::from_label(&config.theme).unwrap_or(ThemeName::NeoNoir),
            keymap: KeymapPreset::from_label(&config.keymap).unwrap_or(KeymapPreset::Default),
            density: Density::from_label(&config.density).unwrap_or(Density::Cozy),
            config_path,
            linter: {
                let mut lg = LintGroup::new_curated(FstDictionary::curated(), Dialect::American);
                lg.config.set_rule_enabled("AvoidCurses", false);
                lg
            },
            lints: Vec::new(),
            lints_active: false,
            last_edit: None,
            delete_pending: false,
            editor_col_width: 80,
            selection_anchor: None,
            yank_buffer: String::new(),
            clipboard: Clipboard::new().ok(),
        };
        app.refresh_notes()?;
        app.load_selected()?;
        Ok(app)
    }

    pub fn run(mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let loop_result = self.event_loop(&mut terminal);

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        loop_result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        const AUTO_SAVE_SECS: u64 = 2;
        loop {
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_key(key)? {
                        return Ok(());
                    }
                }
            }

            if self.dirty {
                if let Some(t) = self.last_edit {
                    if t.elapsed() >= Duration::from_secs(AUTO_SAVE_SECS) {
                        let saved_row = self.editor_buffer.cursor_row;
                        let saved_col = self.editor_buffer.cursor_col;
                        let _ = self.save_active_note();
                        self.editor_buffer.cursor_row =
                            saved_row.min(self.editor_buffer.lines.len().saturating_sub(1));
                        self.editor_buffer.cursor_col =
                            saved_col.min(self.editor_buffer.current_line_len());
                        self.last_edit = None;
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::F(6) => {
                self.theme = self.theme.next();
                self.persist_preferences();
                self.status = format!("Theme -> {}", self.theme.label());
                return Ok(false);
            }
            KeyCode::F(7) => {
                self.keymap = self.keymap.next();
                self.persist_preferences();
                self.status = format!("Keymap -> {}", self.keymap.label());
                return Ok(false);
            }
            KeyCode::F(8) => {
                self.density = self.density.toggle();
                self.persist_preferences();
                self.status = format!("Density -> {}", self.density.label());
                return Ok(false);
            }
            _ => {}
        }

        match self.mode {
            Mode::Normal => self.handle_normal_key(key),
            Mode::Edit => self.handle_edit_key(key),
            Mode::Search => self.handle_search_key(key),
            Mode::Command => self.handle_command_key(key),
        }
    }

    fn persist_preferences(&mut self) {
        let config = AppConfig {
            theme: self.theme.label().to_string(),
            keymap: self.keymap.label().to_string(),
            density: self.density.label().to_string(),
        };
        if let Err(err) = config.save(&self.config_path) {
            self.status = format!("Config save failed: {}", err);
        }
    }

    fn enter_edit_mode(&mut self) {
        self.mode = Mode::Edit;
        self.vim_mode = if self.keymap == KeymapPreset::Vim {
            VimMode::Normal
        } else {
            VimMode::Insert
        };
        self.editor_buffer.set_cursor_to_end();
        if self.keymap == KeymapPreset::Vim {
            self.clamp_cursor_for_vim_normal();
        }
        self.status = if self.keymap == KeymapPreset::Vim {
            "Edit mode (vim normal)".to_string()
        } else {
            "Edit mode".to_string()
        };
    }

    fn focus_notes_pane(&mut self) {
        self.mode = Mode::Normal;
        self.selection_anchor = None;
        self.status = "Notes pane".to_string();
    }

    fn active_summary(&self) -> Option<&NoteSummary> {
        self.notes.get(self.selected)
    }

    fn enter_vim_normal_mode(&mut self, status: &str) {
        self.vim_mode = VimMode::Normal;
        self.selection_anchor = None;
        self.clamp_cursor_for_vim_normal();
        self.status = status.to_string();
    }

    fn vim_normal_max_col(&self, row: usize) -> usize {
        let len = self.editor_buffer.lines[row].chars().count();
        len.saturating_sub(1)
    }

    fn clamp_cursor_for_vim_normal(&mut self) {
        let row = self
            .editor_buffer
            .cursor_row
            .min(self.editor_buffer.lines.len().saturating_sub(1));
        self.editor_buffer.cursor_row = row;

        let len = self.editor_buffer.lines[row].chars().count();
        self.editor_buffer.cursor_col = if len == 0 {
            0
        } else {
            self.editor_buffer.cursor_col.min(len.saturating_sub(1))
        };
    }

    fn has_char_under_vim_cursor(&self) -> bool {
        let len = self.editor_buffer.lines[self.editor_buffer.cursor_row]
            .chars()
            .count();
        self.editor_buffer.cursor_col < len
    }

    fn move_left_for_vim_normal(&mut self) {
        self.clamp_cursor_for_vim_normal();
        self.editor_buffer.cursor_col = self.editor_buffer.cursor_col.saturating_sub(1);
    }

    fn move_right_for_vim_normal(&mut self) {
        self.clamp_cursor_for_vim_normal();
        let max_col = self.vim_normal_max_col(self.editor_buffer.cursor_row);
        self.editor_buffer.cursor_col = self.editor_buffer.cursor_col.min(max_col);
        if self.editor_buffer.cursor_col < max_col {
            self.editor_buffer.cursor_col += 1;
        }
    }

    fn move_right_for_vim_append(&mut self) {
        let len = self.editor_buffer.lines[self.editor_buffer.cursor_row]
            .chars()
            .count();
        if len == 0 {
            self.editor_buffer.cursor_col = 0;
            return;
        }

        self.clamp_cursor_for_vim_normal();
        self.editor_buffer.cursor_col = (self.editor_buffer.cursor_col + 1).min(len);
    }

    fn move_up_for_vim_normal(&mut self) {
        self.move_visual_up();
        self.clamp_cursor_for_vim_normal();
    }

    fn move_down_for_vim_normal(&mut self) {
        self.move_visual_down();
        self.clamp_cursor_for_vim_normal();
    }

    fn normal_is_down(&self, key: &KeyEvent) -> bool {
        match self.keymap {
            KeymapPreset::Default | KeymapPreset::Vim => {
                matches!(key.code, KeyCode::Char('j') | KeyCode::Down)
            }
        }
    }

    fn normal_is_up(&self, key: &KeyEvent) -> bool {
        match self.keymap {
            KeymapPreset::Default | KeymapPreset::Vim => {
                matches!(key.code, KeyCode::Char('k') | KeyCode::Up)
            }
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Char('q') {
            return Ok(true);
        }

        if self.keymap == KeymapPreset::Vim && key.code == KeyCode::Char('l') {
            if self.active_note_id.is_some() {
                self.enter_edit_mode();
                self.status = "Preview pane".to_string();
            }
            return Ok(false);
        }

        if self.normal_is_down(&key) {
            if self.selected + 1 < self.notes.len() {
                self.selected += 1;
                self.load_selected()?;
            }
            return Ok(false);
        }

        if self.normal_is_up(&key) {
            if self.selected > 0 {
                self.selected -= 1;
                self.load_selected()?;
            }
            return Ok(false);
        }

        match key.code {
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.command_input.clear();
                self.status = "Command mode".to_string();
            }
            KeyCode::Char('n') => {
                let id = self.store.create_note("Untitled", "")?;
                self.refresh_notes()?;
                self.select_by_id(id);
                self.load_selected()?;
                self.enter_edit_mode();
                self.status = "Created note".to_string();
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                if self.active_note_id.is_some() {
                    self.enter_edit_mode();
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.search_input = self.query.clone();
                self.status = "Search mode".to_string();
            }
            KeyCode::Char('r') => {
                self.refresh_notes()?;
                self.load_selected()?;
                self.status = "Reloaded".to_string();
            }
            KeyCode::Char('d') if !self.delete_pending => {
                if self.active_note_id.is_some() {
                    self.delete_pending = true;
                    self.status =
                        "Delete? Press d again to confirm, any other key cancels".to_string();
                }
            }
            KeyCode::Char('d') if self.delete_pending => {
                self.delete_pending = false;
                if let Some(id) = self.active_note_id {
                    self.store.delete_note(id)?;
                    self.refresh_notes()?;
                    self.load_selected()?;
                    self.status = "Note deleted".to_string();
                }
            }
            _ => {
                self.delete_pending = false;
            }
        }
        Ok(false)
    }

    fn handle_edit_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.save_active_note()?;
            self.mode = Mode::Normal;
            self.status = "Saved".to_string();
            return Ok(false);
        }

        if is_ctrl_char(&key, 'l') {
            self.run_lints();
            self.lints_active = true;
            return Ok(false);
        }

        // Selection / clipboard shortcuts (all keymaps)
        if is_ctrl_char(&key, 'c') {
            self.copy_selection();
            self.selection_anchor = None;
            return Ok(false);
        }
        if is_ctrl_char(&key, 'x') {
            self.copy_selection();
            self.delete_selection();
            return Ok(false);
        }
        if is_ctrl_char(&key, 'v') {
            self.delete_selection();
            let sys = self.clipboard_get().filter(|s| !s.is_empty());
            let text = sys.unwrap_or_else(|| self.yank_buffer.clone());
            if !text.is_empty() {
                self.editor_buffer.insert_pasted_str(&text);
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            return Ok(false);
        }
        if is_ctrl_char(&key, 'a') {
            self.selection_anchor = Some(0);
            let text = self.editor_buffer.to_text();
            let total = text.chars().count();
            let (row, col) = Self::char_offset_to_pos(&text, total);
            self.editor_buffer.cursor_row = row.min(self.editor_buffer.lines.len().saturating_sub(1));
            self.editor_buffer.cursor_col =
                col.min(self.editor_buffer.lines[self.editor_buffer.cursor_row].chars().count());
            return Ok(false);
        }

        if self.keymap == KeymapPreset::Vim {
            return self.handle_edit_key_vim(key);
        }

        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            self.selection_anchor = None;
            self.status = "Normal mode".to_string();
            return Ok(false);
        }

        if key.code == KeyCode::Char(']') && self.lints_active {
            self.selection_anchor = None;
            if let Some(off) = self.next_lint_offset() {
                self.jump_to_flat_offset(off);
            }
            return Ok(false);
        }
        if key.code == KeyCode::Char('[') && self.lints_active {
            self.selection_anchor = None;
            if let Some(off) = self.prev_lint_offset() {
                self.jump_to_flat_offset(off);
            }
            return Ok(false);
        }

        self.apply_insert_key(key);
        Ok(false)
    }

    fn handle_edit_key_vim(&mut self, key: KeyEvent) -> Result<bool> {
        match self.vim_mode {
            VimMode::Insert => {
                if key.code == KeyCode::Esc {
                    self.enter_vim_normal_mode("VIM NORMAL");
                    return Ok(false);
                }
                self.apply_insert_key(key);
                Ok(false)
            }
            VimMode::Normal => {
                if key.code == KeyCode::Esc {
                    self.status = "VIM NORMAL  (:w save  :wq quit  Ctrl+S save & exit)".to_string();
                    return Ok(false);
                }

                match key.code {
                    KeyCode::Char(':') => {
                        self.mode = Mode::Command;
                        self.command_input.clear();
                    }
                    KeyCode::Char('/') => {
                        self.mode = Mode::Search;
                        self.search_input = self.query.clone();
                    }
                    KeyCode::Char('i') => {
                        self.clamp_cursor_for_vim_normal();
                        self.vim_mode = VimMode::Insert;
                        self.status = "VIM INSERT".to_string();
                    }
                    KeyCode::Char('a') => {
                        self.clamp_cursor_for_vim_normal();
                        self.move_right_for_vim_append();
                        self.vim_mode = VimMode::Insert;
                        self.status = "VIM INSERT".to_string();
                    }
                    KeyCode::Char('I') => {
                        self.editor_buffer.move_home();
                        self.vim_mode = VimMode::Insert;
                        self.status = "VIM INSERT".to_string();
                    }
                    KeyCode::Char('A') => {
                        self.editor_buffer.move_end();
                        self.vim_mode = VimMode::Insert;
                        self.status = "VIM INSERT".to_string();
                    }
                    KeyCode::Char('o') => {
                        self.editor_buffer.open_line_below();
                        self.vim_mode = VimMode::Insert;
                        self.status = "VIM INSERT".to_string();
                        self.dirty = true;
                        self.last_edit = Some(Instant::now());
                    }
                    KeyCode::Char('O') => {
                        self.editor_buffer.open_line_above();
                        self.vim_mode = VimMode::Insert;
                        self.status = "VIM INSERT".to_string();
                        self.dirty = true;
                        self.last_edit = Some(Instant::now());
                    }
                    // Shift+arrows: extend selection
                    KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        if self.selection_anchor.is_none() {
                            self.selection_anchor = Some(self.cursor_flat_offset());
                        }
                        self.editor_buffer.move_left();
                    }
                    KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        if self.selection_anchor.is_none() {
                            self.selection_anchor = Some(self.cursor_flat_offset());
                        }
                        self.editor_buffer.move_right();
                    }
                    KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        if self.selection_anchor.is_none() {
                            self.selection_anchor = Some(self.cursor_flat_offset());
                        }
                        self.move_visual_up();
                    }
                    KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        if self.selection_anchor.is_none() {
                            self.selection_anchor = Some(self.cursor_flat_offset());
                        }
                        self.move_visual_down();
                    }
                    // Bare movements: clear selection
                    KeyCode::Char('h') | KeyCode::Left => {
                        self.selection_anchor = None;
                        if self.editor_buffer.cursor_col == 0 {
                            self.focus_notes_pane();
                        } else {
                            self.move_left_for_vim_normal();
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.selection_anchor = None;
                        self.move_down_for_vim_normal();
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.selection_anchor = None;
                        self.move_up_for_vim_normal();
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        self.selection_anchor = None;
                        self.move_right_for_vim_normal();
                    }
                    KeyCode::Char('0') | KeyCode::Home => {
                        self.selection_anchor = None;
                        self.editor_buffer.move_home();
                    }
                    KeyCode::Char('$') | KeyCode::End => {
                        self.selection_anchor = None;
                        self.editor_buffer.move_end();
                    }
                    KeyCode::Char('x') | KeyCode::Delete => {
                        if !self.delete_selection() && self.has_char_under_vim_cursor() {
                            self.editor_buffer.delete();
                            self.dirty = true;
                            self.last_edit = Some(Instant::now());
                        }
                    }
                    KeyCode::Char(']') => {
                        if self.lints_active {
                            self.selection_anchor = None;
                            if let Some(off) = self.next_lint_offset() {
                                self.jump_to_flat_offset(off);
                            }
                        }
                    }
                    KeyCode::Char('[') => {
                        if self.lints_active {
                            self.selection_anchor = None;
                            if let Some(off) = self.prev_lint_offset() {
                                self.jump_to_flat_offset(off);
                            }
                        }
                    }
                    // v: enter Visual selection mode
                    KeyCode::Char('v') => {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                        self.vim_mode = VimMode::Visual;
                        self.status = "-- VISUAL --".to_string();
                    }
                    // y: yank selection (or current line if no selection)
                    KeyCode::Char('y') => {
                        if self.selection_anchor.is_some() {
                            self.copy_selection();
                            self.selection_anchor = None;
                        } else {
                            // yank current line
                            let line = self.editor_buffer.lines[self.editor_buffer.cursor_row].clone();
                            let yanked = line + "\n";
                            self.yank_buffer = yanked.clone();
                            self.clipboard_set(&yanked);
                        }
                    }
                    // p: paste at cursor (char-wise) or below line (line-wise)
                    KeyCode::Char('p') => {
                        self.selection_anchor = None;
                        let sys = self.clipboard_get().filter(|s| !s.is_empty());
                        let text = sys.unwrap_or_else(|| self.yank_buffer.clone());
                        if !text.is_empty() {
                            if text.ends_with('\n') {
                                // line-wise: open new line below and paste
                                self.editor_buffer.move_end();
                                self.editor_buffer.insert_newline();
                                let content = text.trim_end_matches('\n').to_string();
                                self.editor_buffer.insert_pasted_str(&content);
                            } else {
                                self.move_right_for_vim_append();
                                self.editor_buffer.insert_pasted_str(&text);
                            }
                            self.dirty = true;
                            self.last_edit = Some(Instant::now());
                        }
                    }
                    KeyCode::Char('P') => {
                        self.selection_anchor = None;
                        let sys = self.clipboard_get().filter(|s| !s.is_empty());
                        let text = sys.unwrap_or_else(|| self.yank_buffer.clone());
                        if !text.is_empty() {
                            if text.ends_with('\n') {
                                // line-wise: paste above current line
                                self.editor_buffer.move_home();
                                let content = text.trim_end_matches('\n').to_string();
                                self.editor_buffer.insert_pasted_str(&content);
                                self.editor_buffer.insert_newline();
                                // move cursor back up to the pasted content
                                self.move_visual_up();
                            } else {
                                self.editor_buffer.insert_pasted_str(&text);
                            }
                            self.dirty = true;
                            self.last_edit = Some(Instant::now());
                        }
                    }
                    // d: delete selection (no-op if no selection)
                    KeyCode::Char('d') => {
                        self.delete_selection();
                    }
                    _ => {}
                }
                Ok(false)
            }
            VimMode::Visual => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('v') => {
                        self.enter_vim_normal_mode("VIM NORMAL");
                    }
                    // Movements extend selection (do NOT clear anchor)
                    KeyCode::Char('h') | KeyCode::Left => self.editor_buffer.move_left(),
                    KeyCode::Char('j') | KeyCode::Down => self.move_visual_down(),
                    KeyCode::Char('k') | KeyCode::Up => self.move_visual_up(),
                    KeyCode::Char('l') | KeyCode::Right => self.editor_buffer.move_right(),
                    KeyCode::Char('0') | KeyCode::Home => self.editor_buffer.move_home(),
                    KeyCode::Char('$') | KeyCode::End => self.editor_buffer.move_end(),
                    // y: yank and exit Visual
                    KeyCode::Char('y') => {
                        self.copy_selection();
                        self.enter_vim_normal_mode("VIM NORMAL");
                    }
                    // d/x: delete selection and exit Visual
                    KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => {
                        self.delete_selection();
                        self.enter_vim_normal_mode("VIM NORMAL");
                    }
                    _ => {}
                }
                Ok(false)
            }
        }
    }

    fn apply_insert_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            // Movement keys: Shift extends selection, bare movement clears it
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
            | KeyCode::Home | KeyCode::End => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                match key.code {
                    KeyCode::Left => self.editor_buffer.move_left(),
                    KeyCode::Right => self.editor_buffer.move_right(),
                    KeyCode::Up => self.move_visual_up(),
                    KeyCode::Down => self.move_visual_down(),
                    KeyCode::Home => self.editor_buffer.move_home(),
                    KeyCode::End => self.editor_buffer.move_end(),
                    _ => unreachable!(),
                }
            }
            KeyCode::Enter => {
                self.delete_selection();
                self.editor_buffer.insert_newline();
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            KeyCode::Tab => {
                if self.selection_anchor.is_some() {
                    self.delete_selection();
                    self.editor_buffer.insert_str("    ");
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                } else if let Some(idx) = self.lint_index_at_cursor() {
                    self.apply_lint_fix(idx);
                } else {
                    self.editor_buffer.insert_str("    ");
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                }
            }
            KeyCode::Backspace => {
                if !self.delete_selection() {
                    self.editor_buffer.backspace();
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                }
            }
            KeyCode::Delete => {
                if !self.delete_selection() {
                    self.editor_buffer.delete();
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.delete_selection();
                self.editor_buffer.insert_char(c);
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "Search canceled".to_string();
            }
            KeyCode::Enter => {
                self.query = self.search_input.trim().to_string();
                self.refresh_notes()?;
                self.selected = 0;
                self.load_selected()?;
                self.mode = Mode::Normal;
                self.status = if self.query.is_empty() {
                    "Search cleared  (#tag /folder text)".to_string()
                } else {
                    format!("Search: {}  (#tag /folder text)", self.query)
                };
            }
            KeyCode::Backspace => {
                self.search_input.pop();
            }
            KeyCode::Char(c) => {
                self.search_input.push(c);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_command_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "Command canceled".to_string();
            }
            KeyCode::Enter => {
                let command = self.command_input.trim().to_string();
                self.command_input.clear();
                self.mode = Mode::Normal;
                return self.execute_command(&command);
            }
            KeyCode::Backspace => {
                self.command_input.pop();
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.command_input.push(c);
            }
            _ => {}
        }
        Ok(false)
    }

    fn execute_command(&mut self, command: &str) -> Result<bool> {
        if command.is_empty() {
            self.status = "No command".to_string();
            return Ok(false);
        }

        let mut parts = command.split_whitespace();
        let name = parts.next().unwrap_or_default().to_ascii_lowercase();

        match name.as_str() {
            "q" | "quit" => return Ok(true),
            "w" => {
                self.save_active_note()?;
                self.mode = Mode::Normal;
                self.status = "Saved".to_string();
            }
            "wq" | "x" => {
                self.save_active_note()?;
                self.mode = Mode::Normal;
                self.status = "Saved".to_string();
                return Ok(true);
            }
            "new" => {
                let id = self.store.create_note("Untitled", "")?;
                self.refresh_notes()?;
                self.select_by_id(id);
                self.load_selected()?;
                self.enter_edit_mode();
                self.status = "Created note".to_string();
            }
            "edit" => {
                if self.active_note_id.is_some() {
                    self.enter_edit_mode();
                }
            }
            "reload" => {
                self.refresh_notes()?;
                self.load_selected()?;
                self.status = "Reloaded".to_string();
            }
            "search" => {
                let query = parts.collect::<Vec<_>>().join(" ");
                self.query = query;
                self.refresh_notes()?;
                self.selected = 0;
                self.load_selected()?;
                self.status = if self.query.is_empty() {
                    "Search cleared".to_string()
                } else {
                    format!("Search: {}", self.query)
                };
            }
            "theme" => {
                let arg = parts.next().unwrap_or("");
                if let Some(theme) = ThemeName::from_label(arg) {
                    self.theme = theme;
                    self.persist_preferences();
                    self.status = format!("Theme -> {}", theme.label());
                } else {
                    self.status = "Usage: :theme neo-noir|paper|matrix".to_string();
                }
            }
            "keymap" => {
                let arg = parts.next().unwrap_or("");
                if let Some(keymap) = KeymapPreset::from_label(arg) {
                    self.keymap = keymap;
                    self.persist_preferences();
                    self.status = format!("Keymap -> {}", keymap.label());
                } else {
                    self.status = "Usage: :keymap default|vim".to_string();
                }
            }
            "density" => {
                let arg = parts.next().unwrap_or("");
                if arg.eq_ignore_ascii_case("toggle") {
                    self.density = self.density.toggle();
                    self.persist_preferences();
                    self.status = format!("Density -> {}", self.density.label());
                } else if let Some(density) = Density::from_label(arg) {
                    self.density = density;
                    self.persist_preferences();
                    self.status = format!("Density -> {}", density.label());
                } else {
                    self.status = "Usage: :density cozy|compact|toggle".to_string();
                }
            }
            "folder" => {
                let name = parts.collect::<Vec<_>>().join(" ");
                if let Some(id) = self.active_note_id {
                    self.store.set_folder(id, &name)?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.status = if name.trim().is_empty() {
                        "Folder cleared".to_string()
                    } else {
                        format!("Moved to folder: {}", name.trim())
                    };
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "help" => {
                self.status =
                    "Commands: :new :edit :search <q> :folder [name] :theme :keymap :density :reload :quit"
                        .to_string();
            }
            _ => {
                self.status = format!("Unknown command: {}", command);
            }
        }

        Ok(false)
    }

    fn refresh_notes(&mut self) -> Result<()> {
        self.notes = match self.store.list_notes(&self.query) {
            Ok(n) => n,
            Err(e) => {
                self.status = format!("Search error: {}", e);
                self.query.clear();
                self.store.list_notes("")?
            }
        };
        if self.notes.is_empty() {
            self.selected = 0;
            self.active_note_id = None;
            self.editor_buffer = EditorBuffer::new();
            self.dirty = false;
        } else if self.selected >= self.notes.len() {
            self.selected = self.notes.len() - 1;
        }
        Ok(())
    }

    fn load_selected(&mut self) -> Result<()> {
        if let Some(note) = self.notes.get(self.selected) {
            self.active_note_id = Some(note.id);
            if let Some(full) = self.store.get_note(note.id)? {
                self.editor_buffer = EditorBuffer::from_text(full.body);
                self.dirty = false;
            }
        } else {
            self.active_note_id = None;
            self.editor_buffer = EditorBuffer::new();
            self.dirty = false;
        }
        self.lints.clear();
        self.lints_active = false;
        self.selection_anchor = None;
        Ok(())
    }

    fn save_active_note(&mut self) -> Result<()> {
        if let Some(id) = self.active_note_id {
            self.store.update_note(id, &self.editor_buffer.to_text())?;
            self.refresh_notes()?;
            self.select_by_id(id);
            self.load_selected()?;
        }
        Ok(())
    }

    fn select_by_id(&mut self, id: i64) {
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            self.selected = pos;
        }
    }

    fn run_lints(&mut self) {
        let text = self.editor_buffer.to_text();
        let doc = Document::new_curated(&text, &PlainEnglish);
        self.lints = self.linter.lint(&doc);
    }

    fn clipboard_set(&mut self, text: &str) {
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(text);
        }
    }

    fn clipboard_get(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok()
    }

    fn char_offset_to_pos(text: &str, offset: usize) -> (usize, usize) {
        let mut row = 0;
        let mut col = 0;
        for (i, c) in text.chars().enumerate() {
            if i == offset {
                break;
            }
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }

    fn apply_lint_fix(&mut self, idx: usize) {
        let Some(lint) = self.lints.get(idx) else {
            return;
        };
        let span_start = lint.span.start;
        let span_end = lint.span.end;
        let Some(suggestion) = lint.suggestions.first().cloned() else {
            return;
        };

        let text = self.editor_buffer.to_text();
        let chars: Vec<char> = text.chars().collect();

        let new_chars: Vec<char> = match &suggestion {
            harper_core::linting::Suggestion::ReplaceWith(replacement) => {
                let mut v = chars[..span_start].to_vec();
                v.extend_from_slice(replacement);
                v.extend_from_slice(&chars[span_end..]);
                v
            }
            harper_core::linting::Suggestion::InsertAfter(insertion) => {
                let mut v = chars[..span_end].to_vec();
                v.extend_from_slice(insertion);
                v.extend_from_slice(&chars[span_end..]);
                v
            }
            harper_core::linting::Suggestion::Remove => {
                let mut v = chars[..span_start].to_vec();
                v.extend_from_slice(&chars[span_end..]);
                v
            }
        };

        let new_text: String = new_chars.into_iter().collect();
        let (row, col) = Self::char_offset_to_pos(&new_text, span_start);
        self.editor_buffer = EditorBuffer::from_text(new_text);
        self.editor_buffer.cursor_row = row;
        self.editor_buffer.cursor_col = col;
        self.dirty = true;
        self.run_lints();
    }

    // Flat char offset of the current cursor position
    fn cursor_flat_offset(&self) -> usize {
        let mut offset = 0;
        for (row, line) in self.editor_buffer.lines.iter().enumerate() {
            if row == self.editor_buffer.cursor_row {
                offset += self.editor_buffer.cursor_col;
                break;
            }
            offset += line.chars().count() + 1; // +1 for '\n'
        }
        offset
    }

    // Index of the lint whose span contains the cursor, if any
    fn lint_index_at_cursor(&self) -> Option<usize> {
        if !self.lints_active {
            return None;
        }
        let c = self.cursor_flat_offset();
        self.lints.iter().position(|l| c >= l.span.start && c < l.span.end)
    }

    // Flat offset of the next lint start (for ] navigation)
    fn next_lint_offset(&self) -> Option<usize> {
        if !self.lints_active {
            return None;
        }
        let c = self.cursor_flat_offset();
        self.lints
            .iter()
            .filter(|l| l.span.start > c)
            .min_by_key(|l| l.span.start)
            .map(|l| l.span.start)
    }

    // Flat offset of the prev lint start (for [ navigation)
    fn prev_lint_offset(&self) -> Option<usize> {
        if !self.lints_active {
            return None;
        }
        let c = self.cursor_flat_offset();
        self.lints
            .iter()
            .filter(|l| l.span.start < c)
            .max_by_key(|l| l.span.start)
            .map(|l| l.span.start)
    }

    // Move cursor to a flat char offset
    fn jump_to_flat_offset(&mut self, offset: usize) {
        let text = self.editor_buffer.to_text();
        let (row, col) = Self::char_offset_to_pos(&text, offset);
        let row = row.min(self.editor_buffer.lines.len().saturating_sub(1));
        let col = col.min(self.editor_buffer.lines[row].chars().count());
        self.editor_buffer.cursor_row = row;
        self.editor_buffer.cursor_col = col;
    }

    // Delete the selected region; returns true if anything was deleted.
    fn delete_selection(&mut self) -> bool {
        let Some(anchor) = self.selection_anchor.take() else {
            return false;
        };
        let cursor = self.cursor_flat_offset();
        let start = anchor.min(cursor);
        let end = anchor.max(cursor);
        if start == end {
            return false;
        }
        let text = self.editor_buffer.to_text();
        let chars: Vec<char> = text.chars().collect();
        let new_text: String = chars[..start].iter().chain(chars[end..].iter()).copied().collect();
        let (row, col) = Self::char_offset_to_pos(&new_text, start);
        let mut new_buf = EditorBuffer::from_text(new_text);
        new_buf.cursor_row = row.min(new_buf.lines.len().saturating_sub(1));
        new_buf.cursor_col = col.min(new_buf.lines[new_buf.cursor_row].chars().count());
        self.editor_buffer = new_buf;
        self.dirty = true;
        self.last_edit = Some(Instant::now());
        true
    }

    // Copy the selected region into the internal yank buffer and system clipboard.
    fn copy_selection(&mut self) {
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        let cursor = self.cursor_flat_offset();
        let start = anchor.min(cursor);
        let end = anchor.max(cursor);
        if start < end {
            let text = self.editor_buffer.to_text();
            let yanked: String = text.chars().skip(start).take(end - start).collect();
            self.yank_buffer = yanked.clone();
            self.clipboard_set(&yanked);
        }
    }

    fn move_visual_down(&mut self) {
        let width = self.editor_col_width;
        if width == 0 {
            self.editor_buffer.move_down();
            return;
        }
        let visual_col = self.editor_buffer.cursor_col % width;
        let next_sub_start = (self.editor_buffer.cursor_col / width + 1) * width;
        let line_len = self.editor_buffer.lines[self.editor_buffer.cursor_row].chars().count();
        if next_sub_start < line_len {
            // Stay on same logical line, advance one visual sub-row
            self.editor_buffer.cursor_col = (next_sub_start + visual_col).min(line_len);
        } else if self.editor_buffer.cursor_row + 1 < self.editor_buffer.lines.len() {
            // Move to next logical line, preserve visual column
            self.editor_buffer.cursor_row += 1;
            let next_len =
                self.editor_buffer.lines[self.editor_buffer.cursor_row].chars().count();
            self.editor_buffer.cursor_col = visual_col.min(next_len);
        }
    }

    fn move_visual_up(&mut self) {
        let width = self.editor_col_width;
        if width == 0 {
            self.editor_buffer.move_up();
            return;
        }
        let visual_col = self.editor_buffer.cursor_col % width;
        let sub_row = self.editor_buffer.cursor_col / width;
        let line_len = self.editor_buffer.lines[self.editor_buffer.cursor_row].chars().count();
        if sub_row > 0 {
            // Move to previous visual sub-row on same logical line
            let prev_sub_start = (sub_row - 1) * width;
            self.editor_buffer.cursor_col = (prev_sub_start + visual_col).min(line_len);
        } else if self.editor_buffer.cursor_row > 0 {
            // Move to last visual sub-row of previous logical line
            self.editor_buffer.cursor_row -= 1;
            let prev_len =
                self.editor_buffer.lines[self.editor_buffer.cursor_row].chars().count();
            let last_sub_start = if prev_len == 0 { 0 } else { (prev_len - 1) / width * width };
            self.editor_buffer.cursor_col = (last_sub_start + visual_col).min(prev_len);
        }
    }

    fn editor_view(&self, area: Rect, palette: Palette) -> (Text<'static>, u16, u16, u16) {
        if area.width == 0 || area.height == 0 {
            return (Text::default(), area.x, area.y, 0);
        }

        let width = area.width as usize;
        let height = area.height as usize;

        // Pre-compute lint row/col positions if active
        let lint_positions: Vec<(usize, usize, usize, usize)> = if self.lints_active {
            let text = self.editor_buffer.to_text();
            self.lints
                .iter()
                .map(|lint| {
                    let (sr, sc) = Self::char_offset_to_pos(&text, lint.span.start);
                    let (er, ec) = Self::char_offset_to_pos(&text, lint.span.end);
                    (sr, sc, er, ec)
                })
                .collect()
        } else {
            Vec::new()
        };

        // Pre-compute selection as (start_row, start_col, end_row, end_col)
        let selection_pos: Option<(usize, usize, usize, usize)> = {
            let anchor = self.selection_anchor;
            let cursor = self.cursor_flat_offset();
            if let Some(a) = anchor {
                let sel_start = a.min(cursor);
                let sel_end = a.max(cursor);
                if sel_start < sel_end {
                    let text = self.editor_buffer.to_text();
                    let (sr, sc) = Self::char_offset_to_pos(&text, sel_start);
                    let (er, ec) = Self::char_offset_to_pos(&text, sel_end);
                    Some((sr, sc, er, ec))
                } else {
                    None
                }
            } else {
                None
            }
        };

        let normal_style = Style::default().fg(palette.text);
        let lint_style = Style::default()
            .fg(palette.danger)
            .add_modifier(Modifier::UNDERLINED);
        let sel_style = Style::default().bg(palette.accent).fg(palette.bg);

        let mut all_visual_lines: Vec<Line<'static>> = Vec::new();
        let mut cursor_visual_row = 0usize;
        let mut cursor_visual_col = 0usize;

        for (logical_row, line) in self.editor_buffer.lines.iter().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            let len = chars.len();
            let is_cursor_row = logical_row == self.editor_buffer.cursor_row;

            // Cursor sub-row/col within this logical line
            let cursor_sub_row = if is_cursor_row {
                self.editor_buffer.cursor_col / width
            } else {
                0
            };

            // Number of visual sub-lines for content (empty line = 1 visual row)
            let content_sub_lines = if len == 0 { 1 } else { (len + width - 1) / width };
            // Ensure enough sub-lines to accommodate the cursor position
            let sub_lines = if is_cursor_row {
                content_sub_lines.max(cursor_sub_row + 1)
            } else {
                content_sub_lines
            };

            if is_cursor_row {
                cursor_visual_row = all_visual_lines.len() + cursor_sub_row;
                cursor_visual_col = self.editor_buffer.cursor_col % width;
            }

            // Lint column ranges for this logical row
            let lint_ranges: Vec<(usize, usize)> = lint_positions
                .iter()
                .filter_map(|&(sr, sc, er, ec)| {
                    if sr <= logical_row && logical_row <= er {
                        let col_start = if sr == logical_row { sc } else { 0 };
                        let col_end = if er == logical_row { ec } else { len };
                        if col_start < col_end { Some((col_start, col_end)) } else { None }
                    } else {
                        None
                    }
                })
                .collect();
            let merged_lints = merge_ranges(lint_ranges);

            // Selection column ranges for this logical row
            let sel_ranges: Vec<(usize, usize)> = if let Some((ss_r, ss_c, se_r, se_c)) = selection_pos {
                if ss_r <= logical_row && logical_row <= se_r {
                    let col_start = if ss_r == logical_row { ss_c } else { 0 };
                    let col_end = if se_r == logical_row { se_c } else { len };
                    if col_start < col_end { vec![(col_start, col_end)] } else { vec![] }
                } else {
                    vec![]
                }
            } else {
                vec![]
            };
            let merged_sel = merge_ranges(sel_ranges);

            for sub_idx in 0..sub_lines {
                let start_col = sub_idx * width;
                let end_col = (start_col + width).min(len);
                let sub_chars: Vec<char> = if start_col < len {
                    chars[start_col..end_col].to_vec()
                } else {
                    Vec::new()
                };
                let spans = build_spans_for_row(
                    &sub_chars,
                    start_col,
                    &merged_lints,
                    &merged_sel,
                    normal_style,
                    lint_style,
                    sel_style,
                );
                all_visual_lines.push(Line::from(spans));
            }
        }

        // Scroll so cursor stays within the viewport
        let visual_row_offset = cursor_visual_row.saturating_sub(height.saturating_sub(1));

        let cursor_x = area.x + cursor_visual_col as u16;
        let cursor_y = area.y + (cursor_visual_row - visual_row_offset) as u16;

        (Text::from(all_visual_lines), cursor_x, cursor_y, visual_row_offset as u16)
    }

    fn render_lint_popup(
        &self,
        frame: &mut Frame,
        editor_area: Rect,
        cursor_x: u16,
        cursor_y: u16,
        lint: &harper_core::linting::Lint,
        palette: Palette,
    ) {
        let message = lint.message.to_string();
        let sugg_lines: Vec<String> = lint
            .suggestions
            .iter()
            .take(4)
            .map(|s| match s {
                harper_core::linting::Suggestion::ReplaceWith(chars) => {
                    format!("  \u{2192} \"{}\"", chars.iter().collect::<String>())
                }
                harper_core::linting::Suggestion::InsertAfter(chars) => {
                    format!("  \u{2192} insert \"{}\"", chars.iter().collect::<String>())
                }
                harper_core::linting::Suggestion::Remove => "  \u{2192} remove".to_string(),
            })
            .collect();

        let hint = "  Tab: apply";

        let mut max_len = message.len().max(hint.len());
        for s in &sugg_lines {
            max_len = max_len.max(s.len());
        }
        let popup_width = ((max_len + 4) as u16).min(60);
        let popup_height = (1 + sugg_lines.len() + 1 + 2) as u16;

        let desired = Rect {
            x: cursor_x,
            y: cursor_y.saturating_add(1),
            width: popup_width,
            height: popup_height,
        };

        let actual = desired.intersection(editor_area);
        if actual.width < 4 || actual.height < 3 {
            return;
        }

        frame.render_widget(Clear, actual);

        let mut items: Vec<ListItem> = vec![ListItem::new(Line::styled(
            format!(" {}", message),
            Style::default()
                .fg(palette.danger)
                .add_modifier(Modifier::ITALIC),
        ))];
        for sugg in &sugg_lines {
            items.push(ListItem::new(Line::styled(
                sugg.clone(),
                Style::default().fg(palette.text),
            )));
        }
        items.push(ListItem::new(Line::styled(
            hint,
            Style::default().fg(palette.muted),
        )));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.danger))
            .style(Style::default().bg(palette.panel).fg(palette.text));
        frame.render_widget(List::new(items).block(block), actual);
    }

    fn render(&mut self, frame: &mut Frame) {
        let palette = self.theme.palette();

        frame.render_widget(
            Block::default().style(Style::default().bg(palette.bg)),
            frame.area(),
        );

        let status_height = if self.density == Density::Compact {
            1
        } else {
            2
        };
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(status_height),
            ])
            .split(frame.area());

        let query_tag = if self.query.is_empty() {
            "all".to_string()
        } else {
            format!("q:{}", self.query)
        };
        let (lint_tag, lint_count_color) = if self.lints_active {
            let count = self.lints.len();
            let color = if count > 0 { palette.danger } else { palette.ok };
            (format!("  lints:{}", count), color)
        } else {
            ("  lints:-".to_string(), palette.muted)
        };

        let top_text = Text::from(Line::from(vec![
            TSpan::styled(
                format!(
                    " anote  theme:{}  keymap:{}  density:{}  notes:{}  {}",
                    self.theme.label(),
                    self.keymap.label(),
                    self.density.label(),
                    self.notes.len(),
                    query_tag
                ),
                Style::default()
                    .bg(palette.panel)
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            TSpan::styled(
                lint_tag,
                Style::default()
                    .bg(palette.panel)
                    .fg(lint_count_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        let top = Paragraph::new(top_text);
        frame.render_widget(top, layout[0]);

        let split = if self.density == Density::Compact {
            [Constraint::Percentage(30), Constraint::Percentage(70)]
        } else {
            [Constraint::Percentage(34), Constraint::Percentage(66)]
        };

        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(split)
            .split(layout[1]);

        let list_items: Vec<ListItem> = if self.notes.is_empty() {
            vec![ListItem::new(Line::styled(
                "No notes. Press 'n' to create one.",
                Style::default().fg(palette.muted),
            ))]
        } else {
            self.notes
                .iter()
                .enumerate()
                .map(|(idx, n)| {
                    let ts = short_timestamp(&n.updated_at);
                    let selected_row = idx == self.selected;
                    let mut spans: Vec<TSpan<'static>> = Vec::new();

                    if !n.folder.is_empty() {
                        let folder_text = format!("{}/", n.folder);
                        let sp = if selected_row {
                            TSpan::styled(
                                folder_text,
                                Style::default()
                                    .bg(palette.accent)
                                    .fg(palette.bg)
                                    .add_modifier(Modifier::BOLD),
                            )
                        } else {
                            TSpan::styled(folder_text, Style::default().fg(palette.muted))
                        };
                        spans.push(sp);
                    }

                    let title_sp = if selected_row {
                        TSpan::styled(
                            n.title.clone(),
                            Style::default()
                                .bg(palette.accent)
                                .fg(palette.bg)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        TSpan::styled(n.title.clone(), Style::default().fg(palette.text))
                    };
                    spans.push(title_sp);

                    if !n.tags.is_empty() {
                        let pill_colors = self.theme.tag_pill_colors();
                        for tag in n.tags.split_whitespace().take(3) {
                            // gap before each pill (blends with row bg)
                            spans.push(if selected_row {
                                TSpan::styled(" ", Style::default().bg(palette.accent).fg(palette.bg))
                            } else {
                                TSpan::styled(" ", Style::default())
                            });
                            // pill — distinct color per tag, same on selected/unselected
                            spans.push(TSpan::styled(
                                format!(" #{} ", tag),
                                pill_style_for_tag(tag, pill_colors),
                            ));
                        }
                    }

                    let ts_text = format!("  {}", ts);
                    let ts_sp = if selected_row {
                        TSpan::styled(
                            ts_text,
                            Style::default()
                                .bg(palette.accent)
                                .fg(palette.bg)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        TSpan::styled(ts_text, Style::default().fg(palette.muted))
                    };
                    spans.push(ts_sp);

                    ListItem::new(Line::from(spans))
                })
                .collect()
        };

        let list_border_color = if self.mode == Mode::Normal {
            palette.accent
        } else {
            palette.muted
        };

        let list_block = Block::default()
            .borders(Borders::ALL)
            .title(" Notes ")
            .style(Style::default().bg(palette.panel).fg(palette.text))
            .border_style(Style::default().fg(list_border_color));
        frame.render_widget(List::new(list_items).block(list_block), main[0]);

        let meta_base = Style::default()
            .bg(palette.panel)
            .fg(palette.muted)
            .add_modifier(Modifier::ITALIC);
        let active_meta_line: Line<'static> = if let Some(summary) = self.active_summary() {
            let mut spans: Vec<TSpan<'static>> = Vec::new();
            spans.push(TSpan::styled(
                format!("id:{}  updated:{}", summary.id, summary.updated_at),
                meta_base,
            ));
            if !summary.folder.is_empty() {
                spans.push(TSpan::styled(
                    format!("  folder:{}", summary.folder),
                    meta_base,
                ));
            }
            if !summary.tags.is_empty() {
                spans.push(TSpan::styled("  ", meta_base));
                let pill_colors = self.theme.tag_pill_colors();
                for tag in summary.tags.split_whitespace() {
                    spans.push(TSpan::styled(
                        format!(" #{} ", tag),
                        pill_style_for_tag(tag, pill_colors),
                    ));
                    spans.push(TSpan::styled(" ", meta_base));
                }
            }
            Line::from(spans)
        } else {
            Line::styled("no note selected", meta_base)
        };

        let editor_title = match self.mode {
            Mode::Normal => " Preview ",
            Mode::Edit if self.keymap == KeymapPreset::Vim && self.vim_mode == VimMode::Normal => {
                " Edit (vim normal) "
            }
            Mode::Edit if self.keymap == KeymapPreset::Vim && self.vim_mode == VimMode::Insert => {
                " Edit (vim insert) "
            }
            Mode::Edit if self.keymap == KeymapPreset::Vim && self.vim_mode == VimMode::Visual => {
                " Edit (vim visual) "
            }
            Mode::Edit => " Edit ",
            Mode::Search => " Preview ",
            Mode::Command => " Preview ",
        };

        let editor_border_color = if self.mode == Mode::Edit {
            palette.accent
        } else {
            palette.muted
        };

        let editor_block = Block::default()
            .borders(Borders::ALL)
            .title(editor_title)
            .style(Style::default().bg(palette.panel).fg(palette.text))
            .border_style(Style::default().fg(editor_border_color));
        let editor_inner = editor_block.inner(main[1]);
        frame.render_widget(editor_block, main[1]);

        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(editor_inner);

        self.editor_col_width = editor_layout[1].width as usize;

        let meta = Paragraph::new(active_meta_line);
        frame.render_widget(meta, editor_layout[0]);

        let (editor_text, cursor_x, cursor_y, scroll_y) =
            self.editor_view(editor_layout[1], palette);
        let editor = Paragraph::new(editor_text)
            .style(Style::default().fg(palette.text).bg(palette.panel))
            .scroll((scroll_y, 0));
        frame.render_widget(editor, editor_layout[1]);

        if self.mode == Mode::Edit {
            frame.set_cursor_position((cursor_x, cursor_y));
        }

        if self.mode == Mode::Edit {
            if let Some(idx) = self.lint_index_at_cursor() {
                if let Some(lint) = self.lints.get(idx) {
                    if !lint.suggestions.is_empty() {
                        self.render_lint_popup(
                            frame,
                            editor_layout[1],
                            cursor_x,
                            cursor_y,
                            lint,
                            palette,
                        );
                    }
                }
            }
        }

        let mode_text = match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Edit => "EDIT",
            Mode::Search => "SEARCH",
            Mode::Command => "COMMAND",
        };
        let dirty_text = if self.dirty { "*" } else { "" };

        let lint_hint = if self.mode == Mode::Edit {
            if self.lints_active {
                "  Ctrl+L re-lint  Tab fix  ]/[ jump"
            } else {
                "  Ctrl+L lint"
            }
        } else {
            ""
        };

        let status_line = match self.mode {
            Mode::Search => format!("/{search}", search = self.search_input),
            Mode::Command => format!(":{}", self.command_input),
            _ => {
                let delete_hint = if self.mode == Mode::Normal {
                    "  n new  d delete"
                } else {
                    ""
                };
                format!(
                    "[{mode}] {status} {dirty} | : command{delete_hint} | F6 theme | F7 keymap | F8 density | q quit{lint_hint}",
                    mode = mode_text,
                    status = self.status,
                    dirty = dirty_text,
                    delete_hint = delete_hint,
                    lint_hint = lint_hint,
                )
            }
        };

        let status = Paragraph::new(status_line).style(
            Style::default()
                .bg(palette.panel)
                .fg(if self.mode == Mode::Command {
                    palette.accent
                } else if self.dirty {
                    palette.danger
                } else {
                    palette.ok
                })
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(status, layout[2]);
    }
}

fn pill_style_for_tag(tag: &str, colors: &[(Color, Color)]) -> Style {
    let idx = tag
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize))
        % colors.len();
    let (bg, fg) = colors[idx];
    Style::default().bg(bg).fg(fg)
}

fn short_timestamp(ts: &str) -> String {
    ts.get(0..16).unwrap_or(ts).to_string()
}

fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|&(s, _)| s);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    merged
}

fn build_spans_for_row(
    visible_chars: &[char],
    col_offset: usize,
    lint_ranges: &[(usize, usize)],
    sel_ranges: &[(usize, usize)],
    normal: Style,
    lint: Style,
    selected: Style,
) -> Vec<TSpan<'static>> {
    if visible_chars.is_empty() {
        return vec![];
    }

    // Categories: 0 = normal, 1 = lint, 2 = selected (selection wins)
    let mut spans: Vec<TSpan<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_cat: u8 = 0;

    for (i, &c) in visible_chars.iter().enumerate() {
        let abs_col = col_offset + i;
        let in_sel = sel_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_lint = lint_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let cat: u8 = if in_sel { 2 } else if in_lint { 1 } else { 0 };

        if cat != current_cat {
            if !current_text.is_empty() {
                let style = match current_cat { 2 => selected, 1 => lint, _ => normal };
                spans.push(TSpan::styled(current_text.clone(), style));
                current_text.clear();
            }
            current_cat = cat;
        }
        current_text.push(c);
    }

    if !current_text.is_empty() {
        let style = match current_cat { 2 => selected, 1 => lint, _ => normal };
        spans.push(TSpan::styled(current_text, style));
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::{EditorBuffer, normalize_pasted_text};

    #[test]
    fn insert_and_newline_roundtrip() {
        let mut buf = EditorBuffer::new();
        buf.insert_str("hello");
        buf.insert_newline();
        buf.insert_str("world");
        assert_eq!(buf.to_text(), "hello\nworld");
    }

    #[test]
    fn backspace_merges_lines() {
        let mut buf = EditorBuffer::from_text("ab\ncd".to_string());
        buf.cursor_row = 1;
        buf.cursor_col = 0;
        buf.backspace();
        assert_eq!(buf.to_text(), "abcd");
        assert_eq!(buf.cursor_row, 0);
        assert_eq!(buf.cursor_col, 2);
    }

    #[test]
    fn delete_joins_with_next_line() {
        let mut buf = EditorBuffer::from_text("ab\ncd".to_string());
        buf.cursor_row = 0;
        buf.cursor_col = 2;
        buf.delete();
        assert_eq!(buf.to_text(), "abcd");
    }

    #[test]
    fn up_down_clamps_column_to_line_len() {
        let mut buf = EditorBuffer::from_text("long\nxy".to_string());
        buf.cursor_row = 0;
        buf.cursor_col = 4;
        buf.move_down();
        assert_eq!(buf.cursor_row, 1);
        assert_eq!(buf.cursor_col, 2);
    }

    #[test]
    fn open_line_above_preserves_current_row_position() {
        let mut buf = EditorBuffer::from_text("one\ntwo".to_string());
        buf.cursor_row = 1;
        buf.cursor_col = 1;
        buf.open_line_above();
        assert_eq!(buf.to_text(), "one\n\ntwo");
        assert_eq!(buf.cursor_row, 1);
        assert_eq!(buf.cursor_col, 0);
    }

    #[test]
    fn pasted_tabs_expand_to_spaces_from_cursor_column() {
        let mut buf = EditorBuffer::from_text("ab".to_string());
        buf.cursor_col = 2;
        buf.insert_pasted_str("\titem");
        assert_eq!(buf.to_text(), "ab  item");
    }

    #[test]
    fn pasted_text_normalizes_crlf_and_tabs_across_lines() {
        let mut buf = EditorBuffer::new();
        buf.insert_pasted_str("one\r\ntwo\tthree\r\n\tfour");
        assert_eq!(buf.to_text(), "one\ntwo three\n    four");
    }

    #[test]
    fn normalize_paste_keeps_tab_stops_consistent() {
        assert_eq!(normalize_pasted_text("\tX", 0, 4), "    X");
        assert_eq!(normalize_pasted_text("\tX", 3, 4), " X");
    }
}
