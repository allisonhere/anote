use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
        EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use edtui::{
    EditorEventHandler, EditorMode, EditorState as EdtuiState, EditorTheme, EditorView,
    LineNumbers, Lines,
    actions::{Execute, InsertChar},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span as TSpan, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use harper_core::linting::{LintGroup, Linter};
use pulldown_cmark::{
    Event as MdEvent, HeadingLevel, Options as MdOptions, Parser as MdParser, Tag as MdTag,
    TagEnd as MdTagEnd,
};
use harper_core::parsers::PlainEnglish;
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

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
    Find,
    Help,
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

#[derive(Debug, Clone, PartialEq)]
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

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    fn move_word_left(&mut self) {
        if self.cursor_col == 0 {
            if self.cursor_row > 0 {
                self.cursor_row -= 1;
                self.cursor_col = self.current_line_len();
            }
            return;
        }
        let line: Vec<char> = self.lines[self.cursor_row].chars().collect();
        let mut col = self.cursor_col;
        // skip whitespace/non-word to the left
        while col > 0 && !Self::is_word_char(line[col - 1]) {
            col -= 1;
        }
        // skip word chars to the left
        while col > 0 && Self::is_word_char(line[col - 1]) {
            col -= 1;
        }
        self.cursor_col = col;
    }

    fn move_word_right(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col >= line_len {
            if self.cursor_row + 1 < self.lines.len() {
                self.cursor_row += 1;
                self.cursor_col = 0;
            }
            return;
        }
        let line: Vec<char> = self.lines[self.cursor_row].chars().collect();
        let mut col = self.cursor_col;
        // skip word chars to the right
        while col < line_len && Self::is_word_char(line[col]) {
            col += 1;
        }
        // skip whitespace/non-word to the right
        while col < line_len && !Self::is_word_char(line[col]) {
            col += 1;
        }
        self.cursor_col = col;
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
    editor_state: EdtuiState,
    editor_events: EditorEventHandler,
    query: String,
    search_input: String,
    command_input: String,
    mode: Mode,
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
    editor_row_height: usize,
    selection_anchor: Option<usize>,
    yank_buffer: String,
    clipboard: Option<Clipboard>,
    undo_stack: Vec<EditorBuffer>,
    redo_stack: Vec<EditorBuffer>,
    find_query: String,
    find_matches: Vec<usize>, // flat char offsets of match starts
    find_cursor: usize,       // index into find_matches
    find_committed: bool,     // true = navigation phase; false = typing phase
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    notes_pane_collapsed: bool,
    preview_scroll: u16,
    help_scroll: u16,
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
            editor_state: EdtuiState::new(Lines::from("")),
            editor_events: EditorEventHandler::emacs_mode(),
            query: String::new(),
            search_input: String::new(),
            command_input: String::new(),
            mode: Mode::Normal,
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
            editor_row_height: 40,
            selection_anchor: None,
            yank_buffer: String::new(),
            clipboard: Clipboard::new().ok(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            find_query: String::new(),
            find_matches: Vec::new(),
            find_cursor: 0,
            find_committed: false,
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            notes_pane_collapsed: false,
            preview_scroll: 0,
            help_scroll: 0,
        };
        app.apply_editor_keymap();
        app.refresh_notes()?;
        app.load_selected()?;
        Ok(app)
    }

    pub fn run(mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let loop_result = self.event_loop(&mut terminal);

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
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
                let event = event::read()?;
                if self.handle_event(event)? {
                    return Ok(());
                }
            }

            if self.dirty {
                if let Some(t) = self.last_edit {
                    if t.elapsed() >= Duration::from_secs(AUTO_SAVE_SECS) {
                        let _ = self.save_active_note();
                        // For vim mode, restore edtui cursor/mode after save since
                        // save_active_note may reorder the note list but leaves editor_state
                        // cursor unclamped. Default mode uses editor_buffer directly — no sync needed.
                        if self.keymap == KeymapPreset::Vim {
                            let row = self.editor_state.cursor.row
                                .min(self.editor_buffer.lines.len().saturating_sub(1));
                            let col = self.editor_state.cursor.col
                                .min(self.editor_buffer.current_line_len());
                            self.editor_state.cursor.row = row;
                            self.editor_state.cursor.col = col;
                        }
                        self.last_edit = None;
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Key(key) => self.handle_key(key),
            // Bracketed paste: handle directly for default mode to avoid stale editor_state.
            Event::Paste(text) if self.mode == Mode::Edit => {
                match self.keymap {
                    KeymapPreset::Default => {
                        self.push_undo();
                        self.delete_selection();
                        self.editor_buffer.insert_pasted_str(&text);
                        self.dirty = true;
                        self.last_edit = Some(Instant::now());
                        if self.lints_active {
                            self.run_lints();
                        }
                    }
                    KeymapPreset::Vim => {
                        let before = self.editor_state.lines.to_string();
                        self.editor_events.on_event(Event::Paste(text), &mut self.editor_state);
                        self.sync_after_editor_event(before);
                    }
                }
                Ok(false)
            }
            Event::Mouse(_) if self.mode == Mode::Edit => {
                let before = self.editor_state.lines.to_string();
                self.editor_events.on_event(event, &mut self.editor_state);
                self.sync_after_editor_event(before);
                Ok(false)
            }
            _ => Ok(false),
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
                self.apply_editor_keymap();
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
            Mode::Find => self.handle_find_key(key),
            Mode::Help => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                        self.mode = Mode::Normal;
                        self.help_scroll = 0;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.help_scroll = self.help_scroll.saturating_add(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.help_scroll = self.help_scroll.saturating_sub(1);
                    }
                    KeyCode::PageDown => {
                        self.help_scroll = self.help_scroll.saturating_add(10);
                    }
                    KeyCode::PageUp => {
                        self.help_scroll = self.help_scroll.saturating_sub(10);
                    }
                    _ => {}
                }
                Ok(false)
            }
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

    fn apply_editor_keymap(&mut self) {
        if self.keymap == KeymapPreset::Vim {
            self.editor_events = EditorEventHandler::vim_mode();
            self.editor_state.mode = EditorMode::Normal;
            self.sync_state_from_editor_buffer();
        }
        // Default mode uses EditorBuffer directly; no edtui setup needed.
    }

    fn sync_state_from_editor_buffer(&mut self) {
        let text = self.editor_buffer.to_text();
        let mut state = EdtuiState::new(Lines::from(text.as_str()));
        state.cursor.row = self
            .editor_buffer
            .cursor_row
            .min(self.editor_buffer.lines.len().saturating_sub(1));
        state.cursor.col = self.editor_buffer.cursor_col.min(self.editor_buffer.current_line_len());
        state.mode = match self.keymap {
            KeymapPreset::Default => EditorMode::Insert,
            KeymapPreset::Vim => self.editor_state.mode,
        };
        self.editor_state = state;
    }

    fn sync_editor_buffer_from_state(&mut self) {
        self.editor_buffer = EditorBuffer::from_text(self.editor_state.lines.to_string());
        self.editor_buffer.cursor_row = self
            .editor_state
            .cursor
            .row
            .min(self.editor_buffer.lines.len().saturating_sub(1));
        self.editor_buffer.cursor_col = self.editor_state.cursor.col.min(
            self.editor_buffer.lines[self.editor_buffer.cursor_row]
                .chars()
                .count(),
        );
    }

    // Update editor_state cursor position from editor_buffer without recreating state.
    // Use this after modifying editor_buffer cursor in vim mode to avoid losing edtui undo history.
    fn sync_cursor_to_state(&mut self) {
        let row = self.editor_buffer.cursor_row
            .min(self.editor_buffer.lines.len().saturating_sub(1));
        let col = self.editor_buffer.cursor_col
            .min(self.editor_buffer.lines[row].chars().count());
        self.editor_state.cursor.row = row;
        self.editor_state.cursor.col = col;
    }

    fn sync_after_editor_event(&mut self, before: String) {
        self.sync_editor_buffer_from_state();
        if self.editor_buffer.to_text() != before {
            self.dirty = true;
            self.last_edit = Some(Instant::now());
            if self.lints_active {
                self.run_lints();
            }
        }
    }

    fn enter_edit_mode(&mut self) {
        self.mode = Mode::Edit;
        self.editor_state.cursor.row = self.editor_buffer.lines.len().saturating_sub(1);
        self.editor_state.cursor.col = self.editor_buffer.current_line_len();
        self.editor_state.mode = match self.keymap {
            KeymapPreset::Default => EditorMode::Insert,
            KeymapPreset::Vim => EditorMode::Normal,
        };
        self.status = if self.keymap == KeymapPreset::Vim {
            "Edit mode (vim normal)".to_string()
        } else {
            "Edit mode".to_string()
        };
        self.sync_editor_buffer_from_state();
    }

    fn active_summary(&self) -> Option<&NoteSummary> {
        self.notes.get(self.selected)
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
        if key.code == KeyCode::Char('?') {
            self.mode = Mode::Help;
            return Ok(false);
        }

        if key.code == KeyCode::Char('q') {
            return Ok(true);
        }

        if key.code == KeyCode::Char('\\') {
            self.notes_pane_collapsed = !self.notes_pane_collapsed;
            return Ok(false);
        }

        if self.keymap == KeymapPreset::Vim && key.code == KeyCode::Char('l') {
            if self.active_note_id.is_some() {
                self.enter_edit_mode();
                self.status = "Preview pane".to_string();
            }
            return Ok(false);
        }

        if self.notes_pane_collapsed {
            if self.normal_is_down(&key) || key.code == KeyCode::PageDown {
                let step: u16 = if key.code == KeyCode::PageDown { 20 } else { 1 };
                self.preview_scroll = self.preview_scroll.saturating_add(step);
                return Ok(false);
            }
            if self.normal_is_up(&key) || key.code == KeyCode::PageUp {
                let step: u16 = if key.code == KeyCode::PageUp { 20 } else { 1 };
                self.preview_scroll = self.preview_scroll.saturating_sub(step);
                return Ok(false);
            }
        } else {
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
        // Ctrl+S: save and stay in edit mode
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.save_active_note()?;
            self.status = "Saved".to_string();
            return Ok(false);
        }

        // Ctrl+F: find within note (default keymap only; vim uses edtui's / search)
        if is_ctrl_char(&key, 'f') && self.keymap == KeymapPreset::Default {
            self.find_query.clear();
            self.find_matches.clear();
            self.find_cursor = 0;
            self.find_committed = false;
            self.mode = Mode::Find;
            self.status = "Find:  type to search  Esc cancel".to_string();
            return Ok(false);
        }

        // Ctrl+L: lint (both keymaps)
        if is_ctrl_char(&key, 'l') {
            self.run_lints();
            self.lints_active = true;
            return Ok(false);
        }

        // Tab: lint fix or 4 spaces (both keymaps)
        if key.code == KeyCode::Tab && !key.modifiers.contains(KeyModifiers::SHIFT) {
            if let Some(idx) = self.lint_index_at_cursor() {
                self.push_undo();
                self.apply_lint_fix(idx);
            } else if self.keymap == KeymapPreset::Vim {
                for _ in 0..4 {
                    InsertChar(' ').execute(&mut self.editor_state);
                }
                self.sync_editor_buffer_from_state();
                self.dirty = true;
                self.last_edit = Some(Instant::now());
                if self.lints_active {
                    self.run_lints();
                }
            } else {
                self.push_undo();
                self.delete_selection();
                self.editor_buffer.insert_str("    ");
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            return Ok(false);
        }

        // Lint jumps (both keymaps)
        if key.code == KeyCode::Char(']') && self.lints_active {
            self.selection_anchor = None;
            if let Some(off) = self.next_lint_offset() {
                self.jump_to_flat_offset(off);
                self.sync_cursor_to_state();
            }
            return Ok(false);
        }
        if key.code == KeyCode::Char('[') && self.lints_active {
            self.selection_anchor = None;
            if let Some(off) = self.prev_lint_offset() {
                self.jump_to_flat_offset(off);
                self.sync_cursor_to_state();
            }
            return Ok(false);
        }

        match self.keymap {
            KeymapPreset::Default => self.handle_edit_key_default(key),
            KeymapPreset::Vim => self.handle_edit_key_vim_edtui(key),
        }
    }

    fn handle_edit_key_default(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            self.selection_anchor = None;
            self.status = "Normal mode".to_string();
            return Ok(false);
        }
        if is_ctrl_char(&key, 'z') {
            if let Some(snapshot) = self.undo_stack.pop() {
                self.redo_stack.push(self.editor_buffer.clone());
                self.editor_buffer = snapshot;
                self.dirty = true;
                self.last_edit = Some(Instant::now());
                self.status = "Undo".to_string();
            }
            return Ok(false);
        }
        if is_ctrl_char(&key, 'y') {
            if let Some(snapshot) = self.redo_stack.pop() {
                self.undo_stack.push(self.editor_buffer.clone());
                self.editor_buffer = snapshot;
                self.dirty = true;
                self.last_edit = Some(Instant::now());
                self.status = "Redo".to_string();
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
        if is_ctrl_char(&key, 'c') {
            self.copy_selection();
            self.selection_anchor = None;
            return Ok(false);
        }
        if is_ctrl_char(&key, 'x') {
            self.copy_selection();
            self.push_undo();
            self.delete_selection();
            return Ok(false);
        }
        if is_ctrl_char(&key, 'v') {
            self.push_undo();
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
        self.apply_insert_key(key);
        Ok(false)
    }

    fn handle_edit_key_vim_edtui(&mut self, key: KeyEvent) -> Result<bool> {
        if self.editor_state.mode == EditorMode::Normal && key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            self.selection_anchor = None;
            self.status = "Normal mode".to_string();
            return Ok(false);
        }

        // p/P in normal mode: prefer system clipboard over edtui's internal yank buffer
        if self.editor_state.mode == EditorMode::Normal
            && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
            && key.modifiers.is_empty()
        {
            let sys = self.clipboard_get().filter(|s| !s.is_empty());
            if let Some(text) = sys {
                use edtui::actions::MoveForward;
                let before = self.editor_state.lines.to_string();
                if key.code == KeyCode::Char('p') {
                    // paste after cursor: advance one then insert
                    MoveForward(1).execute(&mut self.editor_state);
                }
                for c in text.chars() {
                    InsertChar(c).execute(&mut self.editor_state);
                }
                self.sync_after_editor_event(before);
                return Ok(false);
            }
            // no system clipboard content — fall through to edtui's p (uses its own yank)
        }

        let before = self.editor_state.lines.to_string();
        self.editor_events.on_key_event(key, &mut self.editor_state);
        self.sync_after_editor_event(before);
        Ok(false)
    }


    fn apply_insert_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Ctrl+Left/Right: word jump
            KeyCode::Left if ctrl => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                self.editor_buffer.move_word_left();
            }
            KeyCode::Right if ctrl => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                self.editor_buffer.move_word_right();
            }
            // Ctrl+Home/End: jump to doc start/end
            KeyCode::Home if ctrl => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                self.editor_buffer.cursor_row = 0;
                self.editor_buffer.cursor_col = 0;
            }
            KeyCode::End if ctrl => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                self.editor_buffer.set_cursor_to_end();
            }
            // PageUp/PageDown
            KeyCode::PageDown => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                self.move_page_down();
            }
            KeyCode::PageUp => {
                if shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor_flat_offset());
                    }
                } else {
                    self.selection_anchor = None;
                }
                self.move_page_up();
            }
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
                self.push_undo();
                self.delete_selection();
                self.editor_buffer.insert_newline();
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            KeyCode::Tab => {
                if self.selection_anchor.is_some() {
                    self.push_undo();
                    self.delete_selection();
                    self.editor_buffer.insert_str("    ");
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                } else if let Some(idx) = self.lint_index_at_cursor() {
                    self.push_undo();
                    self.apply_lint_fix(idx);
                } else {
                    self.push_undo();
                    self.editor_buffer.insert_str("    ");
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                }
            }
            KeyCode::Backspace => {
                self.push_undo();
                if !self.delete_selection() {
                    self.editor_buffer.backspace();
                    self.dirty = true;
                    self.last_edit = Some(Instant::now());
                }
            }
            KeyCode::Delete => {
                self.push_undo();
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
                self.push_undo();
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

    fn handle_find_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc {
            self.find_matches.clear();
            self.find_committed = false;
            self.mode = Mode::Edit;
            self.status = "Find closed".to_string();
            return Ok(false);
        }

        if self.find_committed {
            // Navigation phase
            match key.code {
                KeyCode::Enter => {
                    // Confirm position — drop into edit mode at the active match
                    self.find_matches.clear();
                    self.find_committed = false;
                    self.mode = Mode::Edit;
                    self.status = "Edit mode".to_string();
                }
                KeyCode::Down | KeyCode::Char('n') => {
                    if !self.find_matches.is_empty() {
                        self.find_cursor = (self.find_cursor + 1) % self.find_matches.len();
                        self.jump_to_flat_offset(self.find_matches[self.find_cursor]);
                        self.update_find_status();
                    }
                }
                KeyCode::Up | KeyCode::Char('N') => {
                    if !self.find_matches.is_empty() {
                        self.find_cursor = if self.find_cursor == 0 {
                            self.find_matches.len() - 1
                        } else {
                            self.find_cursor - 1
                        };
                        self.jump_to_flat_offset(self.find_matches[self.find_cursor]);
                        self.update_find_status();
                    }
                }
                KeyCode::Backspace => {
                    // Drop back to typing phase
                    self.find_committed = false;
                    self.find_query.pop();
                    self.run_find();
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    // Any printable char restarts typing phase with that char
                    self.find_committed = false;
                    self.find_query.clear();
                    self.find_query.push(c);
                    self.run_find();
                }
                _ => {}
            }
        } else {
            // Typing phase
            match key.code {
                KeyCode::Enter | KeyCode::Tab => {
                    if !self.find_matches.is_empty() {
                        self.find_committed = true;
                        self.jump_to_flat_offset(self.find_matches[self.find_cursor]);
                        self.update_find_status();
                    }
                }
                KeyCode::Down => {
                    if !self.find_matches.is_empty() {
                        self.find_cursor = (self.find_cursor + 1) % self.find_matches.len();
                        self.jump_to_flat_offset(self.find_matches[self.find_cursor]);
                        self.update_find_status();
                    }
                }
                KeyCode::Up => {
                    if !self.find_matches.is_empty() {
                        self.find_cursor = if self.find_cursor == 0 {
                            self.find_matches.len() - 1
                        } else {
                            self.find_cursor - 1
                        };
                        self.jump_to_flat_offset(self.find_matches[self.find_cursor]);
                        self.update_find_status();
                    }
                }
                KeyCode::Backspace => {
                    self.find_query.pop();
                    self.run_find();
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.find_query.push(c);
                    self.run_find();
                }
                _ => {}
            }
        }

        Ok(false)
    }

    fn update_find_status(&mut self) {
        if self.find_matches.is_empty() {
            self.status = format!("Find: {}  [no matches]", self.find_query);
        } else if self.find_committed {
            self.status = format!(
                "Find: {}  [{}/{}]  n/↓ next  N/↑ prev  Enter edit here  Esc close",
                self.find_query,
                self.find_cursor + 1,
                self.find_matches.len(),
            );
        } else {
            self.status = format!(
                "Find: {}  [{} matches]  Enter confirm  Esc cancel",
                self.find_query,
                self.find_matches.len(),
            );
        }
    }

    fn run_find(&mut self) {
        self.find_matches.clear();
        self.find_cursor = 0;
        if self.find_query.is_empty() {
            self.status = "Find: ".to_string();
            return;
        }
        let flat = self.editor_buffer.to_text();
        let query_lower = self.find_query.to_ascii_lowercase();
        let flat_lower = flat.to_ascii_lowercase();
        let chars: Vec<char> = flat_lower.chars().collect();
        let qchars: Vec<char> = query_lower.chars().collect();
        let qlen = qchars.len();
        if qlen == 0 {
            return;
        }
        let n = chars.len();
        for i in 0..=n.saturating_sub(qlen) {
            if chars[i..i + qlen] == qchars[..] {
                self.find_matches.push(i);
            }
        }
        if !self.find_matches.is_empty() {
            // jump to nearest match from current cursor position
            let cur = self.cursor_flat_offset();
            let nearest = self
                .find_matches
                .iter()
                .enumerate()
                .min_by_key(|&(_, &off)| {
                    let d = if off >= cur { off - cur } else { cur - off };
                    d
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.find_cursor = nearest;
            self.jump_to_flat_offset(self.find_matches[self.find_cursor]);
        }
        self.update_find_status();
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
                    self.apply_editor_keymap();
                    self.persist_preferences();
                    self.status = format!("Keymap -> {}", keymap.label());
                } else {
                    self.status = "Usage: :keymap default|vim".to_string();
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
            "pin" => {
                if let Some(id) = self.active_note_id {
                    self.store.set_pinned(id, true)?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.status = "Note pinned".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "unpin" => {
                if let Some(id) = self.active_note_id {
                    self.store.set_pinned(id, false)?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.status = "Note unpinned".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "archive" => {
                if let Some(id) = self.active_note_id {
                    self.store.set_archived(id, true)?;
                    self.refresh_notes()?;
                    self.selected = self.selected.min(self.notes.len().saturating_sub(1));
                    self.load_selected()?;
                    self.status = "Note archived".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "unarchive" => {
                if let Some(id) = self.active_note_id {
                    self.store.set_archived(id, false)?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.status = "Note unarchived".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "help" => {
                self.status =
                    "Commands: :new :edit :search <q> :folder :pin :unpin :archive :unarchive :theme :keymap :reload :quit"
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
            self.sync_state_from_editor_buffer();
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
                self.sync_state_from_editor_buffer();
                self.dirty = false;
            }
        } else {
            self.active_note_id = None;
            self.editor_buffer = EditorBuffer::new();
            self.sync_state_from_editor_buffer();
            self.dirty = false;
        }
        self.lints.clear();
        self.lints_active = false;
        self.selection_anchor = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.preview_scroll = 0;
        Ok(())
    }

    fn save_active_note(&mut self) -> Result<()> {
        if let Some(id) = self.active_note_id {
            self.store.update_note(id, &self.editor_buffer.to_text())?;
            self.refresh_notes()?;
            self.select_by_id(id);
            if self.mode == Mode::Edit {
                self.dirty = false;
            } else {
                self.load_selected()?;
            }
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
            if cb.set_text(text).is_ok() {
                return;
            }
        }
        // Fallback: shell clipboard tools
        let _ = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.as_mut().map(|s| s.write_all(text.as_bytes()));
                c.wait()
            });
        let _ = std::process::Command::new("xclip")
            .args(["-sel", "clip"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.as_mut().map(|s| s.write_all(text.as_bytes()));
                c.wait()
            });
    }

    fn clipboard_get(&mut self) -> Option<String> {
        if let Some(text) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) {
            return Some(text);
        }
        // Fallback: shell clipboard tools
        if let Ok(out) = std::process::Command::new("wl-paste").arg("--no-newline").output() {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    return Some(s);
                }
            }
        }
        if let Ok(out) = std::process::Command::new("xclip").args(["-sel", "clip", "-o"]).output() {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    return Some(s);
                }
            }
        }
        if let Ok(out) = std::process::Command::new("xsel").arg("--clipboard").arg("--output").output() {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    return Some(s);
                }
            }
        }
        None
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
        self.editor_buffer.cursor_row = row.min(self.editor_buffer.lines.len().saturating_sub(1));
        self.editor_buffer.cursor_col =
            col.min(self.editor_buffer.lines[self.editor_buffer.cursor_row].chars().count());
        self.sync_state_from_editor_buffer();
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
        self.sync_state_from_editor_buffer();
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

    fn push_undo(&mut self) {
        if self.undo_stack.last() != Some(&self.editor_buffer) {
            self.undo_stack.push(self.editor_buffer.clone());
            if self.undo_stack.len() > 200 {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
        }
    }

    fn move_page_down(&mut self) {
        let n = self.editor_row_height.max(1);
        for _ in 0..n {
            self.move_visual_down();
        }
    }

    fn move_page_up(&mut self) {
        let n = self.editor_row_height.max(1);
        for _ in 0..n {
            self.move_visual_up();
        }
    }

    fn compute_syntax_highlights(&self, palette: Palette) -> Vec<Vec<(usize, usize, Style)>> {
        let theme = self.theme_set.themes.get("base16-ocean.dark")
            .or_else(|| self.theme_set.themes.values().next());
        let Some(theme) = theme else {
            return self.editor_buffer.lines.iter().map(|_| vec![]).collect();
        };

        let mut result: Vec<Vec<(usize, usize, Style)>> = Vec::new();
        let mut in_code_block = false;
        let mut highlighter: Option<HighlightLines> = None;

        for line in &self.editor_buffer.lines {
            let trimmed = line.trim_end();

            if !in_code_block {
                // Check for opening fence
                if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                    let fence_char = if trimmed.starts_with("```") { "```" } else { "~~~" };
                    let lang = trimmed.trim_start_matches(fence_char).trim();
                    let syntax = if lang.is_empty() {
                        self.syntax_set.find_syntax_plain_text()
                    } else {
                        self.syntax_set.find_syntax_by_token(lang)
                            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
                    };
                    highlighter = Some(HighlightLines::new(syntax, theme));
                    in_code_block = true;

                    // Style the fence line as muted
                    let muted_style = Style::default().fg(palette.muted);
                    let len = line.chars().count();
                    let row_ranges = if len > 0 { vec![(0, len, muted_style)] } else { vec![] };
                    result.push(row_ranges);
                    continue;
                }

                // Normal markdown line
                let row_ranges = markdown_highlight_line(line, palette);
                result.push(row_ranges);
            } else {
                // Inside a code block
                // Check for closing fence
                if trimmed == "```" || trimmed == "~~~" {
                    in_code_block = false;
                    highlighter = None;

                    let muted_style = Style::default().fg(palette.muted);
                    let len = line.chars().count();
                    let row_ranges = if len > 0 { vec![(0, len, muted_style)] } else { vec![] };
                    result.push(row_ranges);
                    continue;
                }

                // Highlighted code line
                let mut row_ranges: Vec<(usize, usize, Style)> = Vec::new();
                if let Some(hl) = highlighter.as_mut() {
                    let line_with_newline = format!("{}\n", line);
                    if let Ok(tokens) = hl.highlight_line(&line_with_newline, &self.syntax_set) {
                        let mut col: usize = 0;
                        for (syntect_style, token_str) in &tokens {
                            let char_count = token_str.chars().count();
                            // Strip trailing newline from last token
                            let visible_chars = token_str.trim_end_matches('\n').chars().count();
                            if visible_chars > 0 {
                                let fg = syntect_style.foreground;
                                let ratatui_color = Color::Rgb(fg.r, fg.g, fg.b);
                                let style = Style::default().fg(ratatui_color);
                                row_ranges.push((col, col + visible_chars, style));
                            }
                            col += char_count;
                        }
                    }
                }
                result.push(row_ranges);
            }
        }

        result
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
        let find_style = Style::default().bg(palette.muted).fg(palette.bg);
        let find_active_style = Style::default().bg(palette.ok).fg(palette.bg).add_modifier(Modifier::BOLD);

        // Pre-compute find match row/col positions
        let (find_positions, find_active_pos): (Vec<(usize, usize, usize)>, Option<(usize, usize, usize)>) =
            if !self.find_matches.is_empty() {
                let text = self.editor_buffer.to_text();
                let qlen = self.find_query.chars().count();
                let positions: Vec<(usize, usize, usize)> = self.find_matches
                    .iter()
                    .map(|&off| {
                        let (r, c) = Self::char_offset_to_pos(&text, off);
                        (r, c, qlen)
                    })
                    .collect();
                let active = positions.get(self.find_cursor).copied();
                (positions, active)
            } else {
                (Vec::new(), None)
            };

        // Pre-compute syntax highlights (only in edit/find mode)
        let syntax_highlights: Vec<Vec<(usize, usize, Style)>> =
            if self.mode == Mode::Edit || self.mode == Mode::Find {
                self.compute_syntax_highlights(palette)
            } else {
                Vec::new()
            };

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

            // Find match column ranges for this logical row
            let find_ranges: Vec<(usize, usize)> = find_positions
                .iter()
                .filter_map(|&(r, c, qlen)| {
                    if r == logical_row {
                        Some((c, c + qlen))
                    } else {
                        None
                    }
                })
                .collect();
            let merged_find = merge_ranges(find_ranges);

            // Active (focused) find match for this row
            let active_find_range: Vec<(usize, usize)> = match find_active_pos {
                Some((r, c, qlen)) if r == logical_row => vec![(c, c + qlen)],
                _ => vec![],
            };

            let syn_ranges = syntax_highlights.get(logical_row).map(|v| v.as_slice()).unwrap_or(&[]);

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
                    &merged_find,
                    &active_find_range,
                    syn_ranges,
                    normal_style,
                    lint_style,
                    sel_style,
                    find_style,
                    find_active_style,
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

        let split = if matches!(self.mode, Mode::Edit | Mode::Find) || self.notes_pane_collapsed {
            [Constraint::Length(0), Constraint::Percentage(100)]
        } else if self.density == Density::Compact {
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

                    if n.pinned {
                        spans.push(if selected_row {
                            TSpan::styled("* ", Style::default().bg(palette.accent).fg(palette.bg).add_modifier(Modifier::BOLD))
                        } else {
                            TSpan::styled("* ", Style::default().fg(palette.ok).add_modifier(Modifier::BOLD))
                        });
                    }
                    if n.archived {
                        spans.push(if selected_row {
                            TSpan::styled("[arch] ", Style::default().bg(palette.accent).fg(palette.bg))
                        } else {
                            TSpan::styled("[arch] ", Style::default().fg(palette.muted))
                        });
                    }

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
        if self.mode != Mode::Edit {
            frame.render_widget(List::new(list_items).block(list_block), main[0]);
        }

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
            Mode::Edit if self.keymap == KeymapPreset::Vim && self.editor_state.mode == EditorMode::Normal => {
                " Edit (vim normal) "
            }
            Mode::Edit if self.keymap == KeymapPreset::Vim && self.editor_state.mode == EditorMode::Insert => {
                " Edit (vim insert) "
            }
            Mode::Edit if self.keymap == KeymapPreset::Vim && self.editor_state.mode == EditorMode::Visual => {
                " Edit (vim visual) "
            }
            Mode::Edit => " Edit ",
            Mode::Search => " Preview ",
            Mode::Command => " Preview ",
            Mode::Find => " Edit (find) ",
            Mode::Help => " Preview ",
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
        self.editor_row_height = editor_layout[1].height as usize;

        let meta = Paragraph::new(active_meta_line);
        frame.render_widget(meta, editor_layout[0]);

        let mut cursor_x = editor_layout[1].x;
        let mut cursor_y = editor_layout[1].y;

        if self.mode == Mode::Edit && self.keymap == KeymapPreset::Vim {
            let editor_theme = EditorTheme::default()
                .base(Style::default().fg(palette.text).bg(palette.panel))
                .selection_style(Style::default().fg(palette.bg).bg(palette.ok))
                .line_numbers_style(Style::default().fg(palette.muted).bg(palette.panel))
                .hide_cursor()
                .hide_status_line();
            EditorView::new(&mut self.editor_state)
                .theme(editor_theme)
                .wrap(false)
                .tab_width(EditorBuffer::TAB_WIDTH)
                .line_numbers(LineNumbers::Absolute)
                .render(editor_layout[1], frame.buffer_mut());
            if let Some(pos) = self.editor_state.cursor_screen_position() {
                cursor_x = pos.x;
                cursor_y = pos.y;
            }
        } else if self.mode == Mode::Edit || self.mode == Mode::Find {
            // Default edit mode: raw buffer view with cursor
            let (editor_text, preview_x, preview_y, scroll_y) =
                self.editor_view(editor_layout[1], palette);
            cursor_x = preview_x;
            cursor_y = preview_y;
            let editor = Paragraph::new(editor_text)
                .style(Style::default().fg(palette.text).bg(palette.panel))
                .scroll((scroll_y, 0));
            frame.render_widget(editor, editor_layout[1]);
        } else {
            // Normal/Search/Command mode: markdown preview
            let md_text = render_markdown_preview(
                &self.editor_buffer.to_text(),
                palette,
                editor_layout[1].width as usize,
            );
            let preview = Paragraph::new(md_text)
                .style(Style::default().fg(palette.text).bg(palette.panel))
                .scroll((self.preview_scroll, 0));
            frame.render_widget(preview, editor_layout[1]);
        }

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
            Mode::Find => "FIND",
            Mode::Help => "HELP",
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
            Mode::Edit | Mode::Find => {
                format!(
                    "[{mode}] {status} {dirty} | Esc exit | Ctrl+S save | F6 theme | F7 keymap{lint_hint}",
                    mode = mode_text,
                    status = self.status,
                    dirty = dirty_text,
                    lint_hint = lint_hint,
                )
            }
            _ => {
                format!(
                    "[{mode}] {status} {dirty} | : command  n new  d delete  \\ pane | F6 theme | F7 keymap | ? help | q quit",
                    mode = mode_text,
                    status = self.status,
                    dirty = dirty_text,
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

        if self.mode == Mode::Help {
            self.render_help_overlay(frame, palette);
        }

    }

    fn render_help_overlay(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        let w = area.width.min(52);
        let h = area.height.min(50);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect { x, y, width: w, height: h };

        frame.render_widget(Clear, popup);

        // Inner usable width (minus border)
        let inner_w = (w.saturating_sub(2)) as usize;
        let kw = 16usize; // key column width
        let dw = inner_w.saturating_sub(kw + 2); // description width

        let bold = |s: &str| TSpan::styled(s.to_string(), Style::default().add_modifier(Modifier::BOLD));
        let dim  = |s: &str| TSpan::styled(s.to_string(), Style::default().fg(palette.muted));
        let key  = |s: &str| TSpan::styled(s.to_string(), Style::default().fg(palette.accent).add_modifier(Modifier::BOLD));
        let txt  = |s: &str| TSpan::raw(s.to_string());

        let pad = || Line::raw("");

        let heading = |label: &str| Line::from(vec![bold(label)]);

        let row = |k: &str, d: &str| {
            let k_str = format!("  {:<kw$}", k, kw = kw);
            let d_str = format!("{:.dw$}", d, dw = dw);
            Line::from(vec![key(&k_str), txt(&d_str)])
        };

        let lines: Vec<Line> = vec![
            pad(),
            heading("  NORMAL MODE"),
            row("j/k  ↑↓",        "navigate notes"),
            row("Enter / e",       "open note"),
            row("n",               "new note"),
            row("d d",             "delete note"),
            row("/",               "search notes"),
            row(":",               "command palette"),
            row("\\",              "toggle notes pane"),
            row("q",               "quit"),
            pad(),
            heading("  COLLAPSED PANE"),
            row("j/k  ↑↓",        "scroll preview"),
            row("PgDn / PgUp",     "scroll fast"),
            pad(),
            heading("  EDIT MODE"),
            row("Esc",             "exit to preview"),
            row("Ctrl+S",          "save"),
            row("Ctrl+Z / Y",      "undo / redo"),
            row("Ctrl+C / X",      "copy / cut"),
            row("Ctrl+V",          "paste"),
            row("Ctrl+L",          "spell/grammar lint"),
            row("Tab",             "apply lint fix"),
            row("] / [",           "next / prev lint"),
            pad(),
            Line::from(vec![bold("  DEFAULT "), dim("(default keymap)")]),
            row("Ctrl+F",          "find in note"),
            row("Ctrl+A",          "select all"),
            pad(),
            Line::from(vec![bold("  VIM "), dim("(vim keymap)")]),
            row("h j k l",         "move cursor"),
            row("i / a",           "insert mode"),
            row("v",               "visual select"),
            row("y / d",           "yank / delete"),
            row("p / P",           "paste (clipboard)"),
            row("u / Ctrl+R",      "undo / redo"),
            pad(),
            heading("  SEARCH  (/)"),
            row("#tag",            "filter by tag"),
            row("/folder",         "filter by folder"),
            row(":archived",       "show archived"),
            pad(),
            heading("  COMMANDS  (:)"),
            row(":new",            "create note"),
            row(":folder <name>",  "move to folder"),
            row(":pin / :unpin",   "pin to top"),
            row(":archive",        "hide from list"),
            row(":unarchive",      "restore archived"),
            row(":search <q>",     "search"),
            row(":theme <name>",   "neo-noir|paper|matrix"),
            row(":keymap <name>",  "default|vim"),
            row(":reload",         "refresh list"),
            pad(),
            Line::from(vec![dim("  F6 theme  F7 keymap  "), key("?/Esc"), dim(" close")]),
            pad(),
        ];

        let block = Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.accent))
            .style(Style::default().bg(palette.bg).fg(palette.text));

        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let total = lines.len() as u16;
        let visible = inner.height;
        let max_scroll = total.saturating_sub(visible);
        self.help_scroll = self.help_scroll.min(max_scroll);

        let para = Paragraph::new(lines)
            .style(Style::default().bg(palette.bg).fg(palette.text))
            .scroll((self.help_scroll, 0));
        frame.render_widget(para, inner);
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

fn markdown_highlight_line(line: &str, palette: Palette) -> Vec<(usize, usize, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if len == 0 {
        return vec![];
    }

    let muted_style = Style::default().fg(palette.muted);
    let accent_style = Style::default().fg(palette.accent).add_modifier(Modifier::BOLD);
    let ok_style = Style::default().fg(palette.ok);

    // Headings: starts with one or more '#' followed by a space
    let heading_level: usize = {
        let mut level = 0;
        for &c in &chars {
            if c == '#' { level += 1; } else { break; }
        }
        level
    };
    if heading_level > 0 && heading_level < len && chars[heading_level] == ' ' {
        let mut ranges = vec![
            (0, heading_level + 1, muted_style),            // "## " prefix
            (heading_level + 1, len, accent_style),          // heading text
        ];
        // Remove zero-width range if heading fills the line
        ranges.retain(|&(s, e, _)| s < e);
        return ranges;
    }

    // Horizontal rule: trimmed line is 3+ chars all same (---, ***, ___)
    {
        let trimmed: Vec<char> = line.trim().chars().collect();
        if trimmed.len() >= 3 {
            let first = trimmed[0];
            if (first == '-' || first == '*' || first == '_') && trimmed.iter().all(|&c| c == first) {
                return vec![(0, len, muted_style)];
            }
        }
    }

    // Blockquote: starts with "> "
    if chars.len() >= 2 && chars[0] == '>' && chars[1] == ' ' {
        return vec![(0, 2, muted_style)];
    }

    // List marker: optional whitespace then "- ", "* ", or "+ " followed by text
    {
        let mut idx = 0;
        while idx < chars.len() && chars[idx] == ' ' {
            idx += 1;
        }
        if idx < chars.len()
            && (chars[idx] == '-' || chars[idx] == '*' || chars[idx] == '+')
            && idx + 1 < chars.len()
            && chars[idx + 1] == ' '
        {
            // Style the marker (including leading spaces) as accent
            let marker_end = idx + 2;
            if marker_end < len {
                return vec![(0, marker_end, accent_style)];
            } else {
                return vec![(0, len, accent_style)];
            }
        }
    }

    // Inline patterns scan (for non-heading, non-special lines)
    let mut ranges: Vec<(usize, usize, Style)> = Vec::new();
    let mut i = 0;
    while i < len {
        if chars[i] == '`' {
            // Inline code: find matching backtick
            let start = i;
            i += 1;
            let content_start = i;
            while i < len && chars[i] != '`' {
                i += 1;
            }
            if i < len {
                // Found closing backtick
                let content_end = i;
                i += 1; // consume closing backtick
                // Style opening backtick as muted
                ranges.push((start, start + 1, muted_style));
                // Style content as ok
                if content_start < content_end {
                    ranges.push((content_start, content_end, ok_style));
                }
                // Style closing backtick as muted
                ranges.push((content_end, content_end + 1, muted_style));
            }
            // else: no closing backtick found, no special styling
        } else if chars[i] == '*' {
            // Check for bold (**...**)
            if i + 1 < len && chars[i + 1] == '*' {
                // Bold: find closing **
                let start = i;
                i += 2; // skip opening **
                let content_start = i;
                let mut found = false;
                while i + 1 < len {
                    if chars[i] == '*' && chars[i + 1] == '*' {
                        found = true;
                        break;
                    }
                    i += 1;
                }
                if found {
                    let content_end = i;
                    i += 2; // consume closing **
                    ranges.push((start, start + 2, muted_style)); // opening **
                    if content_start < content_end {
                        ranges.push((content_start, content_end, Style::default().add_modifier(Modifier::BOLD)));
                    }
                    ranges.push((content_end, content_end + 2, muted_style)); // closing **
                }
                // else: no closing **, skip
            } else {
                // Italic: single * ... *
                let start = i;
                i += 1; // skip opening *
                let content_start = i;
                let mut found = false;
                while i < len {
                    if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
                        found = true;
                        break;
                    }
                    i += 1;
                }
                if found {
                    let content_end = i;
                    i += 1; // consume closing *
                    ranges.push((start, start + 1, muted_style)); // opening *
                    if content_start < content_end {
                        ranges.push((content_start, content_end, Style::default().add_modifier(Modifier::ITALIC)));
                    }
                    ranges.push((content_end, content_end + 1, muted_style)); // closing *
                }
                // else: no closing *, skip
            }
        } else {
            i += 1;
        }
    }

    ranges
}

fn build_spans_for_row(
    visible_chars: &[char],
    col_offset: usize,
    lint_ranges: &[(usize, usize)],
    sel_ranges: &[(usize, usize)],
    find_ranges: &[(usize, usize)],
    find_active_ranges: &[(usize, usize)],
    syn_ranges: &[(usize, usize, Style)],
    normal: Style,
    lint: Style,
    selected: Style,
    find_match: Style,
    find_active: Style,
) -> Vec<TSpan<'static>> {
    if visible_chars.is_empty() {
        return vec![];
    }

    // Categories: 0 = syntax/normal, 1 = lint, 2 = find match, 3 = active find match, 4 = selection (wins all)
    let mut spans: Vec<TSpan<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_cat: u8 = 0;
    let mut current_syn_style: Style = normal;

    for (i, &c) in visible_chars.iter().enumerate() {
        let abs_col = col_offset + i;
        let in_sel         = sel_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_find_active = find_active_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_find        = find_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_lint        = lint_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let cat: u8 = if in_sel { 4 } else if in_find_active { 3 } else if in_find { 2 } else if in_lint { 1 } else { 0 };

        let syn_style = if cat == 0 {
            syn_ranges
                .iter()
                .find(|&&(s, e, _)| abs_col >= s && abs_col < e)
                .map(|&(_, _, st)| st)
                .unwrap_or(normal)
        } else {
            normal
        };

        let flush = cat != current_cat || (cat == 0 && syn_style != current_syn_style);
        if flush {
            if !current_text.is_empty() {
                let style = match current_cat {
                    4 => selected,
                    3 => find_active,
                    2 => find_match,
                    1 => lint,
                    _ => current_syn_style,
                };
                spans.push(TSpan::styled(current_text.clone(), style));
                current_text.clear();
            }
            current_cat = cat;
            current_syn_style = syn_style;
        }
        current_text.push(c);
    }

    if !current_text.is_empty() {
        let style = match current_cat {
            4 => selected,
            3 => find_active,
            2 => find_match,
            1 => lint,
            _ => current_syn_style,
        };
        spans.push(TSpan::styled(current_text, style));
    }

    spans
}

fn render_markdown_preview(text: &str, palette: Palette, _width: usize) -> Text<'static> {
    let opts = MdOptions::ENABLE_STRIKETHROUGH | MdOptions::ENABLE_TABLES;
    let parser = MdParser::new_ext(text, opts);

    let heading_style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let h1_style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let bold_style = Style::default().add_modifier(Modifier::BOLD);
    let italic_style = Style::default().add_modifier(Modifier::ITALIC);
    let code_style = Style::default().fg(palette.ok);
    let rule_style = Style::default().fg(palette.muted);
    let normal_style = Style::default().fg(palette.text);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<TSpan<'static>> = Vec::new();
    let mut in_heading: Option<HeadingLevel> = None;
    let mut in_bold = false;
    let mut in_italic = false;
    let in_code = false;
    let mut in_code_block = false;
    let mut list_depth: usize = 0;
    let mut is_list_item = false;
    let mut list_item_first = false;

    let flush_line = |spans: &mut Vec<TSpan<'static>>, lines: &mut Vec<Line<'static>>| {
        lines.push(Line::from(std::mem::take(spans)));
    };

    for event in parser {
        match event {
            MdEvent::Start(MdTag::Heading { level, .. }) => {
                in_heading = Some(level);
            }
            MdEvent::End(MdTagEnd::Heading(_)) => {
                flush_line(&mut current_spans, &mut lines);
                in_heading = None;
            }
            MdEvent::Start(MdTag::Paragraph) => {}
            MdEvent::End(MdTagEnd::Paragraph) => {
                flush_line(&mut current_spans, &mut lines);
                lines.push(Line::from(vec![])); // blank line after paragraph
            }
            MdEvent::Start(MdTag::Strong) => in_bold = true,
            MdEvent::End(MdTagEnd::Strong) => in_bold = false,
            MdEvent::Start(MdTag::Emphasis) => in_italic = true,
            MdEvent::End(MdTagEnd::Emphasis) => in_italic = false,
            MdEvent::Start(MdTag::CodeBlock(_)) => {
                in_code_block = true;
                lines.push(Line::from(vec![]));
            }
            MdEvent::End(MdTagEnd::CodeBlock) => {
                in_code_block = false;
                lines.push(Line::from(vec![]));
            }
            MdEvent::Code(s) => {
                let style = code_style;
                current_spans.push(TSpan::styled(format!("`{}`", s), style));
            }
            MdEvent::Start(MdTag::List(_)) => {
                list_depth += 1;
            }
            MdEvent::End(MdTagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    lines.push(Line::from(vec![]));
                }
            }
            MdEvent::Start(MdTag::Item) => {
                is_list_item = true;
                list_item_first = true;
            }
            MdEvent::End(MdTagEnd::Item) => {
                flush_line(&mut current_spans, &mut lines);
                is_list_item = false;
            }
            MdEvent::Rule => {
                lines.push(Line::from(vec![TSpan::styled(
                    "─".repeat(40),
                    rule_style,
                )]));
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => {
                flush_line(&mut current_spans, &mut lines);
            }
            MdEvent::Text(s) => {
                let style = if in_code_block {
                    code_style
                } else if let Some(level) = in_heading {
                    match level {
                        HeadingLevel::H1 => h1_style,
                        _ => heading_style,
                    }
                } else if in_bold && in_italic {
                    bold_style.add_modifier(Modifier::ITALIC)
                } else if in_bold {
                    bold_style
                } else if in_italic {
                    italic_style
                } else if in_code {
                    code_style
                } else {
                    normal_style
                };

                if in_code_block {
                    // Emit each line of the code block separately
                    let lines_vec: Vec<&str> = s.lines().collect();
                    for (i, line) in lines_vec.iter().enumerate() {
                        let indent = "  ".repeat(list_depth.max(1).saturating_sub(1) + 1);
                        current_spans.push(TSpan::styled(
                            format!("{}{}", indent, line),
                            style,
                        ));
                        if i + 1 < lines_vec.len() {
                            flush_line(&mut current_spans, &mut lines);
                        }
                    }
                } else {
                    let prefix = if is_list_item && list_item_first {
                        list_item_first = false;
                        let indent = "  ".repeat(list_depth.saturating_sub(1));
                        format!("{indent}• ")
                    } else {
                        String::new()
                    };
                    let display = format!("{}{}", prefix, s);
                    current_spans.push(TSpan::styled(display, style));
                }
            }
            MdEvent::InlineHtml(_) | MdEvent::Html(_) => {}
            MdEvent::Start(MdTag::BlockQuote(_)) | MdEvent::End(MdTagEnd::BlockQuote(_)) => {}
            MdEvent::Start(MdTag::Link { dest_url, .. }) => {
                current_spans.push(TSpan::styled("[", rule_style));
                let _ = dest_url;
            }
            MdEvent::End(MdTagEnd::Link) => {
                current_spans.push(TSpan::styled("]", rule_style));
            }
            MdEvent::Start(MdTag::Image { .. }) | MdEvent::End(MdTagEnd::Image) => {}
            MdEvent::Start(MdTag::Table(_)) | MdEvent::End(MdTagEnd::Table) => {
                flush_line(&mut current_spans, &mut lines);
            }
            MdEvent::Start(MdTag::TableHead)
            | MdEvent::End(MdTagEnd::TableHead)
            | MdEvent::Start(MdTag::TableRow)
            | MdEvent::End(MdTagEnd::TableRow) => {
                flush_line(&mut current_spans, &mut lines);
            }
            MdEvent::Start(MdTag::TableCell) => {
                current_spans.push(TSpan::styled("│ ", rule_style));
            }
            MdEvent::End(MdTagEnd::TableCell) => {
                current_spans.push(TSpan::styled(" ", normal_style));
            }
            _ => {}
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    // Remove trailing blank lines
    while lines.last().map_or(false, |l: &Line<'_>| l.spans.is_empty()) {
        lines.pop();
    }

    let _ = in_code;

    Text::from(lines)
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
