use std::collections::{HashMap, HashSet};
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
    widgets::Wrap,
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span as TSpan, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use harper_core::linting::{LintGroup, Linter};
use pulldown_cmark::{
    CodeBlockKind, Event as MdEvent, HeadingLevel, Options as MdOptions, Parser as MdParser,
    Tag as MdTag, TagEnd as MdTagEnd,
};
use harper_core::parsers::PlainEnglish;
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::config::AppConfig;
use crate::storage::{NoteSummary, Store, TagEntry};

// arboard is optional at runtime (no display server): treat errors as no-op.
use arboard::Clipboard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Edit,
    Search,
    Command,
    Find,
    Switcher,
    ArchiveBrowser,
    TrashBrowser,
    Tags,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagBrowserMode {
    Browse,
    Create,
    Color,
    DeleteConfirm,
}

#[derive(Debug, Clone)]
enum TreeItem {
    Folder {
        name: String,
        expanded: bool,
        note_count: usize,
    },
    Note(NoteSummary),
}

impl TreeItem {
    fn is_note(&self) -> bool { matches!(self, TreeItem::Note(_)) }
    fn note(&self) -> Option<&NoteSummary> {
        match self { TreeItem::Note(n) => Some(n), _ => None }
    }
    fn folder_name(&self) -> Option<&str> {
        match self { TreeItem::Folder { name, .. } => Some(name), _ => None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeInlineMode {
    None,
    CreateFolder,
    RenameFolder(String),
    RenameNote(i64),
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

    fn tag_color_choices(self) -> &'static [TagColorChoice] {
        &TAG_COLOR_CHOICES
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Manual,
    Updated,
    Title,
}

impl SortMode {
    fn next(self) -> Self {
        match self {
            Self::Manual => Self::Updated,
            Self::Updated => Self::Title,
            Self::Title => Self::Manual,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Updated => "updated",
            Self::Title => "title",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "updated" | "recent" => Some(Self::Updated),
            "title" | "alpha" | "alphabetical" => Some(Self::Title),
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

#[derive(Debug, Clone, Copy)]
struct TagColorChoice {
    key: &'static str,
    label: &'static str,
    neo: (Color, Color),
    paper: (Color, Color),
    matrix: (Color, Color),
}

impl TagColorChoice {
    fn colors(self, theme: ThemeName) -> (Color, Color) {
        match theme {
            ThemeName::NeoNoir => self.neo,
            ThemeName::Paper => self.paper,
            ThemeName::Matrix => self.matrix,
        }
    }
}

const TAG_COLOR_CHOICES: [TagColorChoice; 8] = [
    TagColorChoice {
        key: "sky",
        label: "Sky",
        neo: (Color::Rgb(56, 189, 248), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(29, 78, 216), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(34, 211, 238), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "violet",
        label: "Violet",
        neo: (Color::Rgb(167, 139, 250), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(126, 34, 206), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(167, 139, 250), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "green",
        label: "Green",
        neo: (Color::Rgb(74, 222, 128), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(21, 128, 61), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(163, 230, 53), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "orange",
        label: "Orange",
        neo: (Color::Rgb(251, 146, 60), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(194, 65, 12), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(96, 165, 250), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "pink",
        label: "Pink",
        neo: (Color::Rgb(244, 114, 182), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(190, 24, 93), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(244, 114, 182), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "yellow",
        label: "Yellow",
        neo: (Color::Rgb(250, 204, 21), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(161, 98, 7), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(250, 204, 21), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "teal",
        label: "Teal",
        neo: (Color::Rgb(45, 212, 191), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(15, 118, 110), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(52, 211, 153), Color::Rgb(4, 16, 10)),
    },
    TagColorChoice {
        key: "red",
        label: "Red",
        neo: (Color::Rgb(248, 113, 113), Color::Rgb(12, 14, 18)),
        paper: (Color::Rgb(185, 28, 28), Color::Rgb(246, 242, 230)),
        matrix: (Color::Rgb(248, 113, 113), Color::Rgb(4, 16, 10)),
    },
];

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

    fn open_line_above(&mut self) {
        self.lines.insert(self.cursor_row, String::new());
        self.cursor_col = 0;
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
    tree: Vec<TreeItem>,
    tree_cursor: usize,
    tree_expanded: std::collections::HashSet<String>,
    selected_note_ids: HashSet<i64>,
    tree_inline_input: String,
    tree_inline_mode: TreeInlineMode,
    active_note_id: Option<i64>,
    editor_buffer: EditorBuffer,
    editor_state: EdtuiState,
    editor_events: EditorEventHandler,
    query: String,
    search_input: String,
    command_input: String,
    switcher_input: String,
    switcher_cursor: usize,
    switcher_results: Vec<NoteSummary>,
    note_browser_input: String,
    note_browser_cursor: usize,
    note_browser_results: Vec<NoteSummary>,
    note_browser_selected_ids: HashSet<i64>,
    tag_browser_cursor: usize,
    tag_browser_entries: Vec<TagEntry>,
    tag_browser_mode: TagBrowserMode,
    tag_browser_input: String,
    tag_color_cursor: usize,
    tag_color_overrides: HashMap<String, String>,
    mode: Mode,
    status: String,
    dirty: bool,
    theme: ThemeName,
    keymap: KeymapPreset,
    density: Density,
    sort_mode: SortMode,
    config_path: PathBuf,
    linter: LintGroup,
    lints: Vec<harper_core::linting::Lint>,
    lints_active: bool,
    last_edit: Option<Instant>,
    delete_pending: bool,
    archive_pending: bool,
    editor_col_width: usize,
    editor_row_height: usize,
    selection_anchor: Option<usize>,
    yank_buffer: String,
    clipboard: Option<Clipboard>,
    undo_stack: Vec<EditorBuffer>,
    redo_stack: Vec<EditorBuffer>,
    find_query: String,
    find_matches: Vec<usize>,
    find_cursor: usize,
    find_committed: bool,
    pre_search_query: String,
    pre_search_cursor: usize,
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    notes_pane_collapsed: bool,
    preview_scroll: u16,
    help_scroll: u16,
    editor_scroll: usize,
    quit_pending: bool,
}

impl App {
    pub fn new(store: Store) -> Result<Self> {
        let (config, config_path) = AppConfig::load_default()?;

        let mut app = Self {
            store,
            tree: Vec::new(),
            tree_cursor: 0,
            tree_expanded: std::collections::HashSet::new(),
            selected_note_ids: HashSet::new(),
            tree_inline_input: String::new(),
            tree_inline_mode: TreeInlineMode::None,
            active_note_id: None,
            editor_buffer: EditorBuffer::new(),
            editor_state: EdtuiState::new(Lines::from("")),
            editor_events: EditorEventHandler::emacs_mode(),
            query: String::new(),
            search_input: String::new(),
            command_input: String::new(),
            switcher_input: String::new(),
            switcher_cursor: 0,
            switcher_results: Vec::new(),
            note_browser_input: String::new(),
            note_browser_cursor: 0,
            note_browser_results: Vec::new(),
            note_browser_selected_ids: HashSet::new(),
            tag_browser_cursor: 0,
            tag_browser_entries: Vec::new(),
            tag_browser_mode: TagBrowserMode::Browse,
            tag_browser_input: String::new(),
            tag_color_cursor: 0,
            tag_color_overrides: HashMap::new(),
            mode: Mode::Normal,
            status: "Ready".to_string(),
            dirty: false,
            theme: ThemeName::from_label(&config.theme).unwrap_or(ThemeName::NeoNoir),
            keymap: KeymapPreset::from_label(&config.keymap).unwrap_or(KeymapPreset::Default),
            density: Density::from_label(&config.density).unwrap_or(Density::Cozy),
            sort_mode: SortMode::from_label(&config.sort).unwrap_or(SortMode::Manual),
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
            archive_pending: false,
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
            pre_search_query: String::new(),
            pre_search_cursor: 0,
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            notes_pane_collapsed: false,
            preview_scroll: 0,
            help_scroll: 0,
            editor_scroll: 0,
            quit_pending: false,
        };
        app.apply_editor_keymap();
        app.refresh_notes()?;
        if let Some(id) = config.last_open_note_id {
            app.select_by_id(id);
        }
        app.sync_active_note_from_cursor()?;
        Ok(app)
    }

    /// Pre-select a note by ID before entering the TUI.
    /// If `edit` is true, enters edit mode immediately.
    pub fn open_note_id(&mut self, id: i64, edit: bool) -> Result<()> {
        self.select_by_id(id);
        self.sync_active_note_from_cursor()?;
        if edit {
            self.enter_edit_mode();
        }
        Ok(())
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
                        self.status = "Saved".to_string();
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
            Event::Mouse(_) if self.mode == Mode::Edit => match self.keymap {
                KeymapPreset::Default => Ok(false),
                KeymapPreset::Vim => {
                    let before = self.editor_state.lines.to_string();
                    self.editor_events.on_event(event, &mut self.editor_state);
                    self.sync_after_editor_event(before);
                    Ok(false)
                }
            },
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
            KeyCode::F(9) => {
                self.set_sort_mode(self.sort_mode.next())?;
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
            Mode::Switcher => self.handle_switcher_key(key),
            Mode::ArchiveBrowser => self.handle_archive_browser_key(key),
            Mode::TrashBrowser => self.handle_trash_browser_key(key),
            Mode::Tags => self.handle_tags_key(key),
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
            sort: self.sort_mode.label().to_string(),
            last_open_note_id: self.active_note_id,
        };
        if let Err(err) = config.save(&self.config_path) {
            self.status = format!("Config save failed: {}", err);
        }
    }

    fn set_sort_mode(&mut self, sort_mode: SortMode) -> Result<()> {
        self.sort_mode = sort_mode;
        self.refresh_notes()?;
        if let Some(id) = self.active_note_id {
            self.select_by_id(id);
        }
        self.sync_active_note_from_cursor()?;
        self.persist_preferences();
        self.status = format!("Sort -> {}", self.sort_mode.label());
        Ok(())
    }

    fn save_and_quit(&mut self) -> Result<bool> {
        if self.dirty {
            self.save_active_note()?;
        }
        Ok(true)
    }

    fn request_quit(&mut self) -> Result<bool> {
        if self.dirty && !self.quit_pending {
            self.quit_pending = true;
            self.status = "Unsaved changes. q quit without saving, :wq save and quit, any other key cancel".to_string();
            return Ok(false);
        }
        Ok(true)
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
        let target_row = self.preview_scroll_target_row();
        self.editor_buffer.cursor_row = target_row.min(self.editor_buffer.lines.len().saturating_sub(1));
        self.editor_buffer.cursor_col = 0;
        self.editor_scroll = 0;
        self.editor_state.mode = match self.keymap {
            KeymapPreset::Default => EditorMode::Insert,
            KeymapPreset::Vim => EditorMode::Normal,
        };
        self.status = if self.keymap == KeymapPreset::Vim {
            "Edit mode (vim normal)".to_string()
        } else {
            "Edit mode".to_string()
        };
        // Vim: push editor_buffer → edtui state so edtui has current content.
        // Default: editor_buffer is the source of truth; no sync needed.
        if self.keymap == KeymapPreset::Vim {
            self.sync_state_from_editor_buffer();
        }
    }

    fn active_summary(&self) -> Option<&NoteSummary> {
        self.tree.get(self.tree_cursor).and_then(|item| item.note())
    }

    fn normalize_note_orders_in_group(&mut self, folder: &str) -> Result<()> {
        let ids: Vec<i64> = self.tree.iter()
            .filter_map(|item| item.note())
            .filter(|n| n.folder == folder)
            .map(|n| n.id)
            .collect();
        let base: i64 = if folder.is_empty() {
            self.store.list_folders()?
                .iter().map(|f| f.sort_order).max().unwrap_or(0)
        } else {
            0
        };
        for (i, &id) in ids.iter().enumerate() {
            self.store.set_note_order(id, base + (i as i64 + 1) * 10)?;
        }
        Ok(())
    }

    fn toggle_folder(&mut self, name: &str, expanded: bool) -> Result<()> {
        if expanded {
            self.tree_expanded.remove(name);
            self.status = format!("Collapsed folder '{}'", name);
        } else {
            self.tree_expanded.insert(name.to_string());
            self.status = format!("Expanded folder '{}'", name);
        }
        self.rebuild_tree()?;
        if let Some(pos) = self
            .tree
            .iter()
            .position(|item| item.folder_name() == Some(name))
        {
            self.tree_cursor = pos;
        }
        Ok(())
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

    fn preview_scroll_target_row(&self) -> usize {
        let width = self.editor_col_width.max(1);
        let mut visual_rows = 0usize;

        for (idx, line) in self.editor_buffer.lines.iter().enumerate() {
            let line_len = line.chars().count();
            let wraps = line_len.max(1).div_ceil(width);
            let next = visual_rows + wraps;
            if (self.preview_scroll as usize) < next {
                return idx;
            }
            visual_rows = next;
        }

        self.editor_buffer.lines.len().saturating_sub(1)
    }

    fn restore_search_cursor(&mut self, cursor: usize) -> Result<()> {
        if self.tree.is_empty() {
            self.tree_cursor = 0;
            self.sync_active_note_from_cursor()?;
            return Ok(());
        }

        self.tree_cursor = cursor.min(self.tree.len().saturating_sub(1));
        self.sync_active_note_from_cursor()?;
        Ok(())
    }

    fn refresh_search_results_preserving_selection(&mut self) -> Result<()> {
        let active_id = self.active_note_id;
        self.refresh_notes()?;

        if let Some(id) = active_id {
            if let Some(pos) = self
                .tree
                .iter()
                .position(|item| item.note().map(|n| n.id == id).unwrap_or(false))
            {
                self.tree_cursor = pos;
                self.sync_active_note_from_cursor()?;
                return Ok(());
            }
        }

        self.restore_search_cursor(0)
    }

    fn open_switcher(&mut self) -> Result<()> {
        self.switcher_input.clear();
        self.switcher_cursor = 0;
        self.refresh_switcher_results()?;
        self.mode = Mode::Switcher;
        self.status = "Switcher: type to jump to a note".to_string();
        Ok(())
    }

    fn refresh_switcher_results(&mut self) -> Result<()> {
        let query = if self.switcher_input.trim().is_empty() {
            String::new()
        } else {
            self.switcher_input.trim().to_string()
        };
        self.switcher_results = self.store.list_notes_for_switcher(&query)?;
        self.rank_switcher_results();
        if self.switcher_cursor >= self.switcher_results.len() {
            self.switcher_cursor = self.switcher_results.len().saturating_sub(1);
        }
        Ok(())
    }

    fn rank_switcher_results(&mut self) {
        let query = self.switcher_input.trim().to_ascii_lowercase();
        if query.is_empty() {
            self.switcher_results.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            return;
        }

        self.switcher_results.sort_by(|a, b| {
            let rank = |note: &NoteSummary| -> u8 {
                let title = note.title.to_ascii_lowercase();
                let snippet = note.snippet.to_ascii_lowercase();
                if title == query {
                    0
                } else if title.starts_with(&query) {
                    1
                } else if title.contains(&query) {
                    2
                } else if snippet.contains(&query) {
                    3
                } else {
                    4
                }
            };
            rank(a).cmp(&rank(b)).then_with(|| b.updated_at.cmp(&a.updated_at))
        });
    }

    fn open_tag_browser(&mut self) -> Result<()> {
        self.reload_tag_metadata()?;
        self.tag_browser_entries = self.store.list_tags()?;
        self.tag_browser_cursor = 0;
        self.tag_browser_mode = TagBrowserMode::Browse;
        self.tag_browser_input.clear();
        self.tag_color_cursor = 0;
        self.mode = Mode::Tags;
        self.status = "Tag browser: Enter filter, n create, c color, D delete".to_string();
        Ok(())
    }

    fn open_archive_browser(&mut self) -> Result<()> {
        self.note_browser_input = self.query.clone();
        self.note_browser_selected_ids.clear();
        self.refresh_note_browser_results(true)?;
        self.note_browser_cursor = 0;
        self.mode = Mode::ArchiveBrowser;
        self.sync_preview_from_note_browser_cursor()?;
        self.status = "Archive browser: filter, mark, U unarchive, D trash".to_string();
        Ok(())
    }

    fn open_trash_browser(&mut self) -> Result<()> {
        self.note_browser_input = self.query.clone();
        self.note_browser_selected_ids.clear();
        self.refresh_note_browser_results(true)?;
        self.note_browser_cursor = 0;
        self.mode = Mode::TrashBrowser;
        self.sync_preview_from_note_browser_cursor()?;
        self.status = "Trash browser: filter, mark, R restore, P purge".to_string();
        Ok(())
    }

    fn refresh_note_browser_results(&mut self, reset_cursor: bool) -> Result<()> {
        let archived = self.mode == Mode::ArchiveBrowser;
        let trash = self.mode == Mode::TrashBrowser;
        self.note_browser_results = self
            .store
            .list_notes_scoped(&self.note_browser_input, None, archived, trash)?;
        self.note_browser_selected_ids
            .retain(|id| self.note_browser_results.iter().any(|note| note.id == *id));
        if reset_cursor || self.note_browser_cursor >= self.note_browser_results.len() {
            self.note_browser_cursor = self.note_browser_results.len().saturating_sub(1);
        }
        Ok(())
    }

    fn sync_preview_from_note_browser_cursor(&mut self) -> Result<()> {
        if let Some(summary) = self.note_browser_results.get(self.note_browser_cursor) {
            self.active_note_id = Some(summary.id);
            if let Some(note) = self.store.get_note(summary.id)? {
                self.load_note_into_editor(&note.body);
            }
        } else {
            self.active_note_id = None;
            self.load_note_into_editor("");
        }
        Ok(())
    }

    fn close_note_browser(&mut self, status: &str) -> Result<()> {
        self.mode = Mode::Normal;
        self.note_browser_input.clear();
        self.note_browser_results.clear();
        self.note_browser_cursor = 0;
        self.note_browser_selected_ids.clear();
        self.sync_active_note_from_cursor()?;
        self.status = status.to_string();
        Ok(())
    }

    fn reload_tag_metadata(&mut self) -> Result<()> {
        self.tag_color_overrides = self.store.list_tag_colors()?;
        Ok(())
    }

    fn refresh_tag_browser_entries(&mut self) -> Result<()> {
        self.tag_browser_entries = self.store.list_tags()?;
        if self.tag_browser_cursor >= self.tag_browser_entries.len() {
            self.tag_browser_cursor = self.tag_browser_entries.len().saturating_sub(1);
        }
        Ok(())
    }

    fn selected_tag_entry(&self) -> Option<&TagEntry> {
        self.tag_browser_entries.get(self.tag_browser_cursor)
    }

    fn selected_tag_name(&self) -> Option<&str> {
        self.selected_tag_entry().map(|entry| entry.tag.as_str())
    }

    fn tag_color_key_for<'a>(&'a self, tag: &str) -> Option<&'a str> {
        self.tag_color_overrides.get(tag).map(String::as_str)
    }

    fn strip_query_scope_tokens(query: &str) -> String {
        let mut kept = Vec::new();
        for token in query.split_whitespace() {
            if !token.eq_ignore_ascii_case(":archived") && !token.eq_ignore_ascii_case(":trash") {
                kept.push(token);
            }
        }
        kept.join(" ")
    }

    fn accept_switcher_selection(&mut self) -> Result<()> {
        let Some(selected) = self.switcher_results.get(self.switcher_cursor).cloned() else {
            self.status = "No matching note".to_string();
            self.mode = Mode::Normal;
            return Ok(());
        };

        if self.dirty {
            self.save_active_note()?;
        }
        self.select_by_id(selected.id);
        self.sync_active_note_from_cursor()?;
        self.mode = Mode::Normal;
        self.status = format!("Jumped to {}", selected.title);
        Ok(())
    }

    fn selected_note_ids_in_view(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .tree
            .iter()
            .filter_map(|item| item.note())
            .filter(|note| self.selected_note_ids.contains(&note.id))
            .map(|note| note.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn current_note_id(&self) -> Option<i64> {
        self.tree.get(self.tree_cursor).and_then(|item| item.note()).map(|note| note.id)
    }

    fn in_trash_view(&self) -> bool {
        self.mode == Mode::TrashBrowser
    }

    fn note_browser_selected_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .note_browser_results
            .iter()
            .filter(|note| self.note_browser_selected_ids.contains(&note.id))
            .map(|note| note.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn note_browser_target_ids(&self) -> Vec<i64> {
        let selected = self.note_browser_selected_ids();
        if !selected.is_empty() {
            selected
        } else {
            self.note_browser_results
                .get(self.note_browser_cursor)
                .map(|note| vec![note.id])
                .unwrap_or_default()
        }
    }

    fn apply_note_browser_action(&mut self, action: &str) -> Result<()> {
        let ids = self.note_browser_target_ids();
        if ids.is_empty() {
            self.status = "No notes selected".to_string();
            return Ok(());
        }

        match action {
            "unarchive" => {
                for id in &ids {
                    self.store.set_archived(*id, false)?;
                }
                self.status = format!("Unarchived {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "trash" => {
                for id in &ids {
                    self.store.delete_note(*id)?;
                }
                self.status = format!("Moved {} note{} to trash", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "restore" => {
                for id in &ids {
                    self.store.restore_note(*id)?;
                }
                self.status = format!("Restored {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "purge" => {
                for id in &ids {
                    self.store.purge_note(*id)?;
                }
                self.status = format!("Permanently deleted {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            _ => {}
        }

        for id in &ids {
            self.note_browser_selected_ids.remove(id);
        }
        self.refresh_note_browser_results(false)?;
        self.sync_preview_from_note_browser_cursor()?;
        Ok(())
    }

    fn toggle_current_selection(&mut self) {
        if let Some(id) = self.current_note_id() {
            if !self.selected_note_ids.insert(id) {
                self.selected_note_ids.remove(&id);
            }
            let count = self.selected_note_ids_in_view().len();
            self.status = if count == 0 {
                "Selection cleared".to_string()
            } else {
                format!("Selected {} note{}", count, if count == 1 { "" } else { "s" })
            };
        }
    }

    fn select_all_visible_notes(&mut self) {
        for id in self.tree.iter().filter_map(|item| item.note()).map(|note| note.id) {
            self.selected_note_ids.insert(id);
        }
        let count = self.selected_note_ids_in_view().len();
        self.status = format!("Selected {} visible note{}", count, if count == 1 { "" } else { "s" });
    }

    fn clear_selection(&mut self) {
        self.selected_note_ids.clear();
        self.status = "Selection cleared".to_string();
    }

    fn target_note_ids_for_action(&self) -> Vec<i64> {
        let selected = self.selected_note_ids_in_view();
        if !selected.is_empty() {
            selected
        } else {
            self.current_note_id().into_iter().collect()
        }
    }

    fn apply_bulk_action(&mut self, action: &str) -> Result<()> {
        let ids = self.target_note_ids_for_action();
        if ids.is_empty() {
            self.status = "No notes selected".to_string();
            return Ok(());
        }

        match action {
            "archive" => {
                for id in &ids {
                    self.store.set_archived(*id, true)?;
                }
                self.status = format!("Archived {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "unarchive" => {
                for id in &ids {
                    self.store.set_archived(*id, false)?;
                }
                self.status = format!("Unarchived {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "trash" => {
                for id in &ids {
                    self.store.delete_note(*id)?;
                }
                self.status = format!("Moved {} note{} to trash", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "restore" => {
                for id in &ids {
                    self.store.restore_note(*id)?;
                }
                self.status = format!("Restored {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            "purge" => {
                for id in &ids {
                    self.store.purge_note(*id)?;
                }
                self.status = format!("Permanently deleted {} note{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
            }
            _ => {}
        }

        for id in &ids {
            self.selected_note_ids.remove(id);
        }
        self.refresh_notes()?;
        self.sync_active_note_from_cursor()?;
        Ok(())
    }

    fn arm_archive_confirmation(&mut self) {
        let count = self.target_note_ids_for_action().len();
        self.archive_pending = true;
        self.status = format!(
            "Archive {} note{}? press 'a' again to confirm, any other key cancels",
            count,
            if count == 1 { "" } else { "s" }
        );
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Handle inline tree input (folder create/rename/note rename)
        if self.tree_inline_mode != TreeInlineMode::None {
            match key.code {
                KeyCode::Esc => {
                    self.tree_inline_mode = TreeInlineMode::None;
                    self.tree_inline_input.clear();
                }
                KeyCode::Enter => {
                    self.commit_tree_inline()?;
                }
                KeyCode::Backspace => { self.tree_inline_input.pop(); }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    self.tree_inline_input.push(c);
                }
                _ => {}
            }
            return Ok(false);
        }

        if key.code == KeyCode::Char('?') {
            self.mode = Mode::Help;
            return Ok(false);
        }

        if key.code == KeyCode::Char('g') {
            self.open_tag_browser()?;
            return Ok(false);
        }

        if key.code == KeyCode::Char('u') {
            self.clear_selection();
            return Ok(false);
        }

        if is_ctrl_char(&key, 'p') {
            self.quit_pending = false;
            self.open_switcher()?;
            return Ok(false);
        }

        if self.quit_pending && key.code != KeyCode::Char('q') {
            self.quit_pending = false;
            self.status = "Quit canceled".to_string();
        }

        if self.archive_pending && key.code != KeyCode::Char('a') {
            self.archive_pending = false;
            self.status = "Archive canceled".to_string();
        }

        if key.code == KeyCode::Char('q') {
            return self.request_quit();
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
            if self.normal_is_down(&key) {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
                return Ok(false);
            }
            if self.normal_is_up(&key) {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
                return Ok(false);
            }
            if key.code == KeyCode::PageDown {
                self.preview_scroll = self.preview_scroll.saturating_add(20);
                return Ok(false);
            }
            if key.code == KeyCode::PageUp {
                self.preview_scroll = self.preview_scroll.saturating_sub(20);
                return Ok(false);
            }
        }

        // Shift+Up/Down: move item in tree
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.tree_shift_move(-1)?;
                    return Ok(false);
                }
                KeyCode::Down => {
                    self.tree_shift_move(1)?;
                    return Ok(false);
                }
                _ => {}
            }
        }

        // Left/Right: expand/collapse folders
        match key.code {
            KeyCode::Right => {
                match self.tree.get(self.tree_cursor).cloned() {
                    Some(TreeItem::Folder { name, expanded, .. }) => {
                        if !expanded {
                            // Expand the folder
                            self.toggle_folder(&name, false)?;
                        } else {
                            // Already expanded: move into first child note
                            let next = self.tree_cursor + 1;
                            if next < self.tree.len() {
                                if let Some(TreeItem::Note(_)) = self.tree.get(next) {
                                    self.tree_cursor = next;
                                    self.sync_active_note_from_cursor()?;
                                }
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            KeyCode::Left => {
                match self.tree.get(self.tree_cursor).cloned() {
                    Some(TreeItem::Folder { name, expanded, .. }) => {
                        if expanded {
                            self.toggle_folder(&name, true)?;
                        }
                    }
                    Some(TreeItem::Note(note)) if !note.folder.is_empty() => {
                        // Collapse parent folder and land on it
                        self.toggle_folder(&note.folder, true)?;
                    }
                    _ => {}
                }
                return Ok(false);
            }
            _ => {}
        }

        if self.normal_is_down(&key) {
            if self.tree_cursor + 1 < self.tree.len() {
                self.tree_cursor += 1;
                self.sync_active_note_from_cursor()?;
            }
            return Ok(false);
        }

        if self.normal_is_up(&key) {
            if self.tree_cursor > 0 {
                self.tree_cursor -= 1;
                self.sync_active_note_from_cursor()?;
            }
            return Ok(false);
        }

        match key.code {
            KeyCode::Char('x') => {
                self.toggle_current_selection();
            }
            KeyCode::Char('*') => {
                self.select_all_visible_notes();
            }
            KeyCode::Char(' ') => {
                if let Some(TreeItem::Folder { name, expanded, .. }) = self.tree.get(self.tree_cursor).cloned() {
                    self.toggle_folder(&name, expanded)?;
                }
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.command_input.clear();
                self.status = "Command mode".to_string();
            }
            KeyCode::Char('n') => {
                let target_folder = match self.tree.get(self.tree_cursor) {
                    Some(TreeItem::Folder { name, .. }) => {
                        self.tree_expanded.insert(name.clone());
                        name.clone()
                    }
                    Some(TreeItem::Note(n)) => n.folder.clone(),
                    None => String::new(),
                };
                let id = self.store.create_note("Untitled", "")?;
                if !target_folder.is_empty() {
                    self.store.set_folder(id, &target_folder)?;
                }
                self.refresh_notes()?;
                self.select_by_id(id);
                self.sync_active_note_from_cursor()?;
                self.enter_edit_mode();
                self.status = "Created note".to_string();
            }
            KeyCode::Char('p') => {
                if !self.selected_note_ids_in_view().is_empty() {
                    for id in self.selected_note_ids_in_view() {
                        self.store.set_pinned(id, true)?;
                    }
                    self.refresh_notes()?;
                    self.status = "Pinned selected notes".to_string();
                } else if let Some(id) = self.active_note_id {
                    let pinned = self.active_summary().map(|note| note.pinned).unwrap_or(false);
                    self.store.set_pinned(id, !pinned)?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.sync_active_note_from_cursor()?;
                    self.status = if pinned { "Note unpinned".to_string() } else { "Note pinned".to_string() };
                }
            }
            KeyCode::Char('a') => {
                if !self.selected_note_ids_in_view().is_empty() {
                    if self.archive_pending {
                        self.archive_pending = false;
                        self.apply_bulk_action("archive")?;
                    } else {
                        self.arm_archive_confirmation();
                    }
                } else if let Some(id) = self.active_note_id {
                    let archived = self.active_summary().map(|note| note.archived).unwrap_or(false);
                    if archived {
                        self.store.set_archived(id, false)?;
                        self.refresh_notes()?;
                        self.select_by_id(id);
                        self.status = "Note unarchived".to_string();
                    } else {
                        if self.archive_pending {
                            self.archive_pending = false;
                            self.store.set_archived(id, true)?;
                            self.refresh_notes()?;
                            self.status = "Note archived".to_string();
                        } else {
                            self.arm_archive_confirmation();
                            return Ok(false);
                        }
                    }
                    self.sync_active_note_from_cursor()?;
                }
            }
            KeyCode::Char('U') => {
                self.status = "Use the archive browser (A) to unarchive notes".to_string();
            }
            KeyCode::Char('A') => {
                self.open_archive_browser()?;
            }
            KeyCode::Char('T') => {
                self.open_trash_browser()?;
            }
            KeyCode::Char('D') => {
                self.apply_bulk_action("trash")?;
            }
            KeyCode::Char('R') => {
                self.status = "Use the trash browser (T) to restore notes".to_string();
            }
            KeyCode::Char('P') => {
                self.status = "Use the trash browser (T) to purge notes".to_string();
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                match self.tree.get(self.tree_cursor).cloned() {
                    Some(TreeItem::Folder { name, expanded, .. }) => {
                        self.toggle_folder(&name, expanded)?;
                    }
                    Some(TreeItem::Note(_)) => {
                        if self.active_note_id.is_some() {
                            self.enter_edit_mode();
                        }
                    }
                    None => {}
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.pre_search_query = self.query.clone();
                self.pre_search_cursor = self.tree_cursor;
                self.search_input = self.query.clone();
                self.status = "Search mode".to_string();
            }
            KeyCode::Char('r') => {
                match self.tree.get(self.tree_cursor).cloned() {
                    Some(TreeItem::Folder { name, .. }) => {
                        self.tree_inline_input = name.clone();
                        self.tree_inline_mode = TreeInlineMode::RenameFolder(name);
                    }
                    Some(TreeItem::Note(n)) => {
                        self.tree_inline_input = n.title.clone();
                        self.tree_inline_mode = TreeInlineMode::RenameNote(n.id);
                    }
                    None => {
                        self.refresh_notes()?;
                        self.status = "Reloaded".to_string();
                    }
                }
            }
            KeyCode::Char('f') => {
                self.tree_inline_input.clear();
                self.tree_inline_mode = TreeInlineMode::CreateFolder;
            }
            KeyCode::Char('d') if !self.delete_pending => {
                if self.tree.get(self.tree_cursor).is_some() {
                    self.delete_pending = true;
                    let what = match self.tree.get(self.tree_cursor) {
                        Some(TreeItem::Folder { name, note_count, .. }) =>
                            format!("Delete folder '{}' ({} notes lose folder)? d=confirm  any other key=cancel", name, note_count),
                        Some(TreeItem::Note(n)) =>
                            format!("Delete '{}'? d=confirm  any other key=cancel", n.title),
                        _ => "Delete? d=confirm  any other key=cancel".to_string(),
                    };
                    self.status = what;
                }
            }
            KeyCode::Char('d') if self.delete_pending => {
                self.delete_pending = false;
                match self.tree.get(self.tree_cursor).cloned() {
                    Some(TreeItem::Folder { name, .. }) => {
                        self.store.delete_folder(&name)?;
                        self.tree_expanded.remove(&name);
                        if self.tree_cursor > 0 { self.tree_cursor -= 1; }
                        self.rebuild_tree()?;
                        self.sync_active_note_from_cursor()?;
                        self.status = format!("Deleted folder '{}'", name);
                    }
                    Some(TreeItem::Note(n)) => {
                        if self.in_trash_view() {
                            self.store.purge_note(n.id)?;
                        } else {
                            self.store.delete_note(n.id)?;
                        }
                        if self.active_note_id == Some(n.id) {
                            self.active_note_id = None;
                            self.load_note_into_editor("");
                        }
                        if self.tree_cursor > 0 { self.tree_cursor -= 1; }
                        self.rebuild_tree()?;
                        self.sync_active_note_from_cursor()?;
                        self.status = if self.in_trash_view() {
                            "Note permanently deleted".to_string()
                        } else {
                            "Moved to trash".to_string()
                        };
                    }
                    None => {}
                }
            }
            _ => {
                self.delete_pending = false;
                self.archive_pending = false;
                self.quit_pending = false;
            }
        }
        Ok(false)
    }

    fn handle_edit_key(&mut self, key: KeyEvent) -> Result<bool> {
        if is_ctrl_char(&key, 'p') {
            self.open_switcher()?;
            return Ok(false);
        }

        // Ctrl+S: save and stay in edit mode
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.save_active_note()?;
            self.status = "Saved".to_string();
            return Ok(false);
        }

        // Ctrl+F: find within note (both keymaps)
        if is_ctrl_char(&key, 'f') {
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

        // Lint jumps: default keymap always; vim keymap only in Normal mode (] and [ are motion keys in Insert)
        let vim_normal = self.keymap == KeymapPreset::Vim
            && self.editor_state.mode == EditorMode::Normal;
        if self.keymap == KeymapPreset::Default || vim_normal {
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
        }

        match self.keymap {
            KeymapPreset::Default => self.handle_edit_key_default(key),
            KeymapPreset::Vim => self.handle_edit_key_vim_edtui(key),
        }
    }

    fn handle_edit_key_default(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc {
            if self.dirty {
                self.save_active_note()?;
                self.status = "Saved and returned to preview".to_string();
            } else {
                self.status = "Normal mode".to_string();
            }
            self.mode = Mode::Normal;
            self.selection_anchor = None;
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
            if self.dirty {
                self.save_active_note()?;
                self.status = "Saved and returned to preview".to_string();
            } else {
                self.status = "Normal mode".to_string();
            }
            self.mode = Mode::Normal;
            self.selection_anchor = None;
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
                self.query = self.pre_search_query.clone();
                self.refresh_notes()?;
                self.restore_search_cursor(self.pre_search_cursor)?;
                self.mode = Mode::Normal;
                self.status = "Search canceled".to_string();
            }
            KeyCode::Enter => {
                let raw = self.search_input.trim().to_string();
                self.query = Self::strip_query_scope_tokens(&raw);
                if raw.split_whitespace().any(|token| token.eq_ignore_ascii_case(":archived")) {
                    self.open_archive_browser()?;
                } else if raw.split_whitespace().any(|token| token.eq_ignore_ascii_case(":trash")) {
                    self.open_trash_browser()?;
                } else {
                    self.refresh_notes()?;
                    self.mode = Mode::Normal;
                    let result_count = self.tree.iter().filter(|item| item.is_note()).count();
                    self.status = if self.query.is_empty() {
                        "Search cleared  (#tag /folder text)".to_string()
                    } else {
                        format!("Search: {}  [{} result{}]", self.query, result_count, if result_count == 1 { "" } else { "s" })
                    };
                }
            }
            KeyCode::Backspace => {
                self.search_input.pop();
                self.query = Self::strip_query_scope_tokens(self.search_input.trim());
                self.refresh_search_results_preserving_selection()?;
            }
            KeyCode::Char(c) => {
                self.search_input.push(c);
                self.query = Self::strip_query_scope_tokens(self.search_input.trim());
                self.refresh_search_results_preserving_selection()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_switcher_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "Switcher canceled".to_string();
            }
            KeyCode::Enter => {
                self.accept_switcher_selection()?;
            }
            _ if self.normal_is_up(&key) => {
                self.switcher_cursor = self.switcher_cursor.saturating_sub(1);
            }
            _ if self.normal_is_down(&key) => {
                if self.switcher_cursor + 1 < self.switcher_results.len() {
                    self.switcher_cursor += 1;
                }
            }
            KeyCode::Backspace => {
                self.switcher_input.pop();
                self.refresh_switcher_results()?;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.switcher_input.push(c);
                self.refresh_switcher_results()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_archive_browser_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('A') => {
                self.close_note_browser("Archive browser closed")?;
            }
            _ if self.normal_is_up(&key) => {
                self.note_browser_cursor = self.note_browser_cursor.saturating_sub(1);
                self.sync_preview_from_note_browser_cursor()?;
            }
            _ if self.normal_is_down(&key) => {
                if self.note_browser_cursor + 1 < self.note_browser_results.len() {
                    self.note_browser_cursor += 1;
                    self.sync_preview_from_note_browser_cursor()?;
                }
            }
            KeyCode::Backspace => {
                self.note_browser_input.pop();
                self.refresh_note_browser_results(true)?;
                self.sync_preview_from_note_browser_cursor()?;
            }
            KeyCode::Char('x') => {
                if let Some(note) = self.note_browser_results.get(self.note_browser_cursor) {
                    if !self.note_browser_selected_ids.insert(note.id) {
                        self.note_browser_selected_ids.remove(&note.id);
                    }
                    let count = self.note_browser_selected_ids().len();
                    self.status = if count == 0 {
                        "Archive selection cleared".to_string()
                    } else {
                        format!("Selected {} archived note{}", count, if count == 1 { "" } else { "s" })
                    };
                }
            }
            KeyCode::Char('*') => {
                for note in &self.note_browser_results {
                    self.note_browser_selected_ids.insert(note.id);
                }
                let count = self.note_browser_selected_ids().len();
                self.status = format!("Selected {} archived note{}", count, if count == 1 { "" } else { "s" });
            }
            KeyCode::Char('u') => {
                self.note_browser_selected_ids.clear();
                self.status = "Archive selection cleared".to_string();
            }
            KeyCode::Char('U') => {
                self.apply_note_browser_action("unarchive")?;
            }
            KeyCode::Char('D') => {
                self.apply_note_browser_action("trash")?;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.note_browser_input.push(c);
                self.refresh_note_browser_results(true)?;
                self.sync_preview_from_note_browser_cursor()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_trash_browser_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('T') => {
                self.close_note_browser("Trash browser closed")?;
            }
            _ if self.normal_is_up(&key) => {
                self.note_browser_cursor = self.note_browser_cursor.saturating_sub(1);
                self.sync_preview_from_note_browser_cursor()?;
            }
            _ if self.normal_is_down(&key) => {
                if self.note_browser_cursor + 1 < self.note_browser_results.len() {
                    self.note_browser_cursor += 1;
                    self.sync_preview_from_note_browser_cursor()?;
                }
            }
            KeyCode::Backspace => {
                self.note_browser_input.pop();
                self.refresh_note_browser_results(true)?;
                self.sync_preview_from_note_browser_cursor()?;
            }
            KeyCode::Char('x') => {
                if let Some(note) = self.note_browser_results.get(self.note_browser_cursor) {
                    if !self.note_browser_selected_ids.insert(note.id) {
                        self.note_browser_selected_ids.remove(&note.id);
                    }
                    let count = self.note_browser_selected_ids().len();
                    self.status = if count == 0 {
                        "Trash selection cleared".to_string()
                    } else {
                        format!("Selected {} trashed note{}", count, if count == 1 { "" } else { "s" })
                    };
                }
            }
            KeyCode::Char('*') => {
                for note in &self.note_browser_results {
                    self.note_browser_selected_ids.insert(note.id);
                }
                let count = self.note_browser_selected_ids().len();
                self.status = format!("Selected {} trashed note{}", count, if count == 1 { "" } else { "s" });
            }
            KeyCode::Char('u') => {
                self.note_browser_selected_ids.clear();
                self.status = "Trash selection cleared".to_string();
            }
            KeyCode::Char('R') | KeyCode::Char('r') => {
                self.apply_note_browser_action("restore")?;
            }
            KeyCode::Char('P') | KeyCode::Char('p') => {
                self.apply_note_browser_action("purge")?;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.note_browser_input.push(c);
                self.refresh_note_browser_results(true)?;
                self.sync_preview_from_note_browser_cursor()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_tags_key(&mut self, key: KeyEvent) -> Result<bool> {
        match self.tag_browser_mode {
            TagBrowserMode::Browse => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.status = "Tag browser closed".to_string();
                }
                _ if self.normal_is_up(&key) => {
                    self.tag_browser_cursor = self.tag_browser_cursor.saturating_sub(1);
                }
                _ if self.normal_is_down(&key) => {
                    if self.tag_browser_cursor + 1 < self.tag_browser_entries.len() {
                        self.tag_browser_cursor += 1;
                    }
                }
                KeyCode::Char('n') => {
                    self.tag_browser_mode = TagBrowserMode::Create;
                    self.tag_browser_input.clear();
                    self.status = "New tag: type a name, Enter to create".to_string();
                }
                KeyCode::Char('c') | KeyCode::Char('e') => {
                    if let Some(tag) = self.selected_tag_name().map(str::to_string) {
                        self.tag_browser_mode = TagBrowserMode::Color;
                        let current = self.tag_color_overrides.get(&tag).cloned();
                        self.tag_color_cursor = tag_color_choice_index(current.as_deref());
                        self.status = format!("Color for #{}: Enter save, Esc cancel", tag);
                    } else {
                        self.status = "No tag selected".to_string();
                    }
                }
                KeyCode::Char('D') => {
                    if let Some((tag, count)) = self
                        .selected_tag_entry()
                        .map(|entry| (entry.tag.clone(), entry.count))
                    {
                        self.tag_browser_mode = TagBrowserMode::DeleteConfirm;
                        self.status = format!(
                            "Delete #{} from {} note{}? Enter confirm, Esc cancel",
                            tag,
                            count,
                            if count == 1 { "" } else { "s" }
                        );
                    } else {
                        self.status = "No tag selected".to_string();
                    }
                }
                KeyCode::Enter => {
                    if let Some(tag) = self.selected_tag_name() {
                        let tag = tag.to_string();
                        self.query = format!("#{}", tag);
                        self.refresh_notes()?;
                        self.restore_search_cursor(0)?;
                        self.mode = Mode::Normal;
                        self.status = format!("Showing tag #{}", tag);
                    }
                }
                _ => {}
            },
            TagBrowserMode::Create => match key.code {
                KeyCode::Esc => {
                    self.tag_browser_mode = TagBrowserMode::Browse;
                    self.tag_browser_input.clear();
                    self.status = "Tag creation canceled".to_string();
                }
                KeyCode::Backspace => {
                    self.tag_browser_input.pop();
                }
                KeyCode::Enter => {
                    match self.store.create_tag(&self.tag_browser_input) {
                        Ok(tag) => {
                            self.reload_tag_metadata()?;
                            self.refresh_tag_browser_entries()?;
                            if let Some(pos) = self.tag_browser_entries.iter().position(|entry| entry.tag == tag) {
                                self.tag_browser_cursor = pos;
                            }
                            self.tag_browser_mode = TagBrowserMode::Color;
                            self.tag_color_cursor = 0;
                            self.status = format!("Created #{}. Choose a color", tag);
                        }
                        Err(err) => {
                            self.status = format!("Tag error: {}", err);
                        }
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.tag_browser_input.push(c);
                }
                _ => {}
            },
            TagBrowserMode::Color => match key.code {
                KeyCode::Esc => {
                    self.tag_browser_mode = TagBrowserMode::Browse;
                    self.status = "Tag color canceled".to_string();
                }
                KeyCode::Left | KeyCode::Up => {
                    self.tag_color_cursor = self.tag_color_cursor.saturating_sub(1);
                }
                KeyCode::Right | KeyCode::Down => {
                    let max = self.theme.tag_color_choices().len();
                    if self.tag_color_cursor < max {
                        self.tag_color_cursor += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(tag) = self.selected_tag_name() {
                        let tag = tag.to_string();
                        let color = if self.tag_color_cursor == 0 {
                            None
                        } else {
                            Some(self.theme.tag_color_choices()[self.tag_color_cursor - 1].key)
                        };
                        match self.store.set_tag_color(&tag, color) {
                            Ok(_) => {
                                self.reload_tag_metadata()?;
                                self.refresh_tag_browser_entries()?;
                                self.tag_browser_mode = TagBrowserMode::Browse;
                                self.status = match color {
                                    Some(color) => format!("Tag #{} color -> {}", tag, color),
                                    None => format!("Tag #{} color -> auto", tag),
                                };
                            }
                            Err(err) => {
                                self.status = format!("Tag color error: {}", err);
                            }
                        }
                    }
                }
                _ => {}
            },
            TagBrowserMode::DeleteConfirm => match key.code {
                KeyCode::Esc => {
                    self.tag_browser_mode = TagBrowserMode::Browse;
                    self.status = "Tag deletion canceled".to_string();
                }
                KeyCode::Enter | KeyCode::Char('D') => {
                    if let Some(tag) = self.selected_tag_name().map(str::to_string) {
                        match self.store.delete_tag_everywhere(&tag) {
                            Ok(updated) => {
                                self.reload_tag_metadata()?;
                                self.refresh_tag_browser_entries()?;
                                self.refresh_notes()?;
                                self.sync_active_note_from_cursor()?;
                                self.tag_browser_mode = TagBrowserMode::Browse;
                                self.status = format!(
                                    "Removed #{} from {} note{}",
                                    tag,
                                    updated,
                                    if updated == 1 { "" } else { "s" }
                                );
                            }
                            Err(err) => {
                                self.status = format!("Tag delete error: {}", err);
                            }
                        }
                    }
                }
                _ => {}
            },
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
                "NAVIGATE  {}/{}  n/↓ next  N/↑ prev  Enter=edit  Bksp=edit query  type=new search  Esc=close  [{}]",
                self.find_cursor + 1,
                self.find_matches.len(),
                self.find_query,
            );
        } else {
            self.status = format!(
                "FIND  {}  [{} match{}]  Enter/Tab=navigate  Esc=cancel",
                self.find_query,
                self.find_matches.len(),
                if self.find_matches.len() == 1 { "" } else { "es" },
            );
        }
    }

    fn run_find(&mut self) {
        self.find_matches.clear();
        self.find_cursor = 0;
        if self.find_query.is_empty() {
            self.status = "FIND  type to search  Esc=cancel".to_string();
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

        let parts = parse_command_parts(command);
        let mut parts = parts.iter().map(String::as_str);
        let name = parts.next().unwrap_or_default().to_ascii_lowercase();

        match name.as_str() {
            "q" | "quit" => return self.request_quit(),
            "w" => {
                self.save_active_note()?;
                self.status = "Saved".to_string();
            }
            "wq" | "x" => {
                self.status = "Saved".to_string();
                return self.save_and_quit();
            }
            "new" => {
                let id = self.store.create_note("Untitled", "")?;
                self.refresh_notes()?;
                self.select_by_id(id);
                self.sync_active_note_from_cursor()?;
                self.enter_edit_mode();
                self.status = "Created note".to_string();
            }
            "import" => {
                let files: Vec<PathBuf> = parts.map(PathBuf::from).collect();
                if files.is_empty() {
                    self.status = "Usage: :import <path...>".to_string();
                } else {
                    let mut imported = 0usize;
                    let mut last_id = None;
                    for path in &files {
                        match std::fs::read_to_string(path) {
                            Ok(body) => {
                                let note_title = path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
                                    .unwrap_or_else(|| "Untitled".to_string());
                                let id = self.store.create_note_with_title_lock(&note_title, &body, true)?;
                                imported += 1;
                                last_id = Some(id);
                            }
                            Err(err) => {
                                self.status = format!("Import failed for {}: {}", path.display(), err);
                                return Ok(false);
                            }
                        }
                    }
                    self.refresh_notes()?;
                    if let Some(id) = last_id {
                        self.select_by_id(id);
                        self.sync_active_note_from_cursor()?;
                    }
                    self.status = format!("Imported {} file{}", imported, if imported == 1 { "" } else { "s" });
                }
            }
            "export" => {
                let Some(id) = self.active_note_id else {
                    self.status = "No active note".to_string();
                    return Ok(false);
                };
                let Some(path) = parts.next() else {
                    self.status = "Usage: :export <path>".to_string();
                    return Ok(false);
                };
                let Some(note) = self.store.get_note(id)? else {
                    self.status = format!("Note {} not found", id);
                    return Ok(false);
                };
                let output = PathBuf::from(path);
                match std::fs::write(&output, note.body) {
                    Ok(_) => {
                        self.status = format!("Exported note {} to {}", id, output.display());
                    }
                    Err(err) => {
                        self.status = format!("Export failed for {}: {}", output.display(), err);
                    }
                }
            }
            "edit" => {
                if self.active_note_id.is_some() {
                    self.enter_edit_mode();
                }
            }
            "reload" => {
                self.refresh_notes()?;
                self.sync_active_note_from_cursor()?;
                self.status = "Reloaded".to_string();
            }
            "search" => {
                let raw_query = parts.collect::<Vec<_>>().join(" ");
                self.query = Self::strip_query_scope_tokens(&raw_query);
                self.refresh_notes()?;
                self.tree_cursor = 0;
                self.sync_active_note_from_cursor()?;
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
            "sort" => {
                let arg = parts.next().unwrap_or("");
                if let Some(sort_mode) = SortMode::from_label(arg) {
                    self.set_sort_mode(sort_mode)?;
                } else {
                    self.status = "Usage: :sort manual|updated|title".to_string();
                }
            }
            "tags" => {
                self.open_tag_browser()?;
            }
            "archived" => {
                self.open_archive_browser()?;
            }
            "trash" => {
                self.open_trash_browser()?;
            }
            "restore" => {
                if let Some(id) = self.active_note_id {
                    self.store.restore_note(id)?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.sync_active_note_from_cursor()?;
                    self.status = "Note restored".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "purge" => {
                if let Some(id) = self.active_note_id {
                    self.store.purge_note(id)?;
                    self.refresh_notes()?;
                    self.sync_active_note_from_cursor()?;
                    self.status = "Note permanently deleted".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "empty-trash" => {
                let count = self.store.purge_deleted_notes()?;
                self.refresh_notes()?;
                self.sync_active_note_from_cursor()?;
                self.status = format!("Emptied trash: {} note{}", count, if count == 1 { "" } else { "s" });
            }
            "folder" => {
                let name = parts.collect::<Vec<_>>().join(" ");
                if let Some(id) = self.active_note_id {
                    self.store.set_folder(id, &name)?;
                    // Ensure folder entry exists in folders table
                    if !name.trim().is_empty() {
                        let _ = self.store.create_folder(name.trim());
                        self.tree_expanded.insert(name.trim().to_string());
                    }
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
            "archive" | "archive!" => {
                if let Some(id) = self.active_note_id {
                    let archived = self.active_summary().map(|note| note.archived).unwrap_or(false);
                    if archived {
                        self.status = "Note is already archived".to_string();
                    } else if name == "archive!" {
                        self.archive_pending = false;
                        self.store.set_archived(id, true)?;
                        self.refresh_notes()?;
                        if self.tree_cursor >= self.tree.len() && !self.tree.is_empty() {
                            self.tree_cursor = self.tree.len() - 1;
                        }
                        self.sync_active_note_from_cursor()?;
                        self.status = "Note archived".to_string();
                    } else {
                        self.archive_pending = true;
                        self.status = "Confirm archive with :archive! or press 'a' again".to_string();
                    }
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
            "rename" => {
                let new_title = parts.collect::<Vec<_>>().join(" ");
                if new_title.is_empty() {
                    self.status = "Usage: :rename <new title>".to_string();
                } else if self.active_note_id.is_some() {
                    // Replace or prepend first line with the new title
                    if self.editor_buffer.lines.is_empty() || self.editor_buffer.lines[0].is_empty() {
                        if self.editor_buffer.lines.is_empty() {
                            self.editor_buffer.lines.push(new_title.clone());
                        } else {
                            self.editor_buffer.lines[0] = new_title.clone();
                        }
                    } else {
                        self.editor_buffer.lines[0] = new_title.clone();
                    }
                    self.sync_state_from_editor_buffer();
                    self.dirty = true;
                    self.save_active_note()?;
                    self.status = format!("Renamed to: {}", new_title);
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "tag" => {
                let tag = parts.next().unwrap_or("").trim().to_ascii_lowercase();
                if tag.is_empty() {
                    self.status = "Usage: :tag <name>".to_string();
                } else if let Some(id) = self.active_note_id {
                    if let Some(note) = self.store.get_note(id)? {
                        if body_has_tag(&note.body, &tag) {
                            self.status = format!("Already tagged: #{}", tag);
                        } else {
                            let new_body = append_tag_to_body(&note.body, &tag);
                            self.store.update_note(id, &new_body)?;
                            self.load_note_into_editor(&new_body);
                            self.refresh_notes()?;
                            self.select_by_id(id);
                            self.status = format!("Tagged: #{}", tag);
                        }
                    }
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "untag" => {
                let tag = parts.next().unwrap_or("").trim().to_ascii_lowercase();
                if tag.is_empty() {
                    self.status = "Usage: :untag <name>".to_string();
                } else if let Some(id) = self.active_note_id {
                    if let Some(note) = self.store.get_note(id)? {
                        let new_body = remove_tag_from_body(&note.body, &tag);
                        if new_body == note.body {
                            self.status = format!("Tag #{} not found", tag);
                        } else {
                            self.store.update_note(id, &new_body)?;
                            self.load_note_into_editor(&new_body);
                            self.refresh_notes()?;
                            self.select_by_id(id);
                            self.status = format!("Removed tag: #{}", tag);
                        }
                    }
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "discard" => {
                if let Some(id) = self.active_note_id {
                    if let Some(note) = self.store.get_note(id)? {
                        self.load_note_into_editor(&note.body);
                        self.dirty = false;
                        self.lints.clear();
                        self.lints_active = false;
                        self.status = "Changes discarded".to_string();
                    }
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "unfolder" => {
                if let Some(id) = self.active_note_id {
                    self.store.set_folder(id, "")?;
                    self.refresh_notes()?;
                    self.select_by_id(id);
                    self.status = "Moved to root (no folder)".to_string();
                } else {
                    self.status = "No active note".to_string();
                }
            }
            "help" => {
                self.mode = Mode::Help;
            }
            _ => {
                self.status = format!("Unknown command: {}", command);
            }
        }

        Ok(false)
    }

    fn rebuild_tree(&mut self) -> Result<()> {
        self.tree.clear();
        let folders = self.store.list_folders()?;
        let query = self.query.clone();
        let mut root_notes = self.store.list_notes_in_folder("", &query)?;
        self.sort_note_summaries(&mut root_notes);
        let max_folder_order = folders.iter().map(|folder| folder.sort_order).max().unwrap_or(0);

        #[derive(Clone)]
        enum TopLevelItem {
            Folder(crate::storage::FolderEntry),
            RootNote(NoteSummary),
        }

        let mut top_level: Vec<(i64, i64, TopLevelItem)> = Vec::new();
        for folder in folders {
            top_level.push((folder.sort_order, 0, TopLevelItem::Folder(folder)));
        }
        for (idx, note) in root_notes.into_iter().enumerate() {
            let effective_order = if self.sort_mode == SortMode::Manual {
                if note.note_order == 0 {
                    max_folder_order + 1
                } else {
                    note.note_order
                }
            } else {
                max_folder_order + ((idx as i64) + 1) * 10
            };
            let pin_rank = if note.pinned { 0 } else { 1 };
            top_level.push((effective_order, pin_rank, TopLevelItem::RootNote(note)));
        }
        top_level.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        for (_, _, item) in top_level {
            match item {
                TopLevelItem::Folder(folder) => {
                    let expanded = self.tree_expanded.contains(&folder.name);
                    let mut notes_in_folder = self.store.list_notes_in_folder(&folder.name, &query)?;
                    self.sort_note_summaries(&mut notes_in_folder);
                    let note_count = notes_in_folder.len();
                    self.tree.push(TreeItem::Folder {
                        name: folder.name,
                        expanded,
                        note_count,
                    });
                    if expanded {
                        for note in notes_in_folder {
                            self.tree.push(TreeItem::Note(note));
                        }
                    }
                }
                TopLevelItem::RootNote(note) => self.tree.push(TreeItem::Note(note)),
            }
        }

        // Clamp cursor
        if !self.tree.is_empty() && self.tree_cursor >= self.tree.len() {
            self.tree_cursor = self.tree.len() - 1;
        }

        Ok(())
    }

    fn refresh_notes(&mut self) -> Result<()> {
        self.reload_tag_metadata()?;
        self.rebuild_tree()?;
        Ok(())
    }

    fn sync_active_note_from_cursor(&mut self) -> Result<()> {
        let note_summary = self.tree.get(self.tree_cursor).and_then(|item| item.note()).cloned();
        if let Some(summary) = note_summary {
            if self.active_note_id != Some(summary.id) {
                self.active_note_id = Some(summary.id);
                if let Some(note) = self.store.get_note(summary.id)? {
                    self.load_note_into_editor(&note.body);
                }
                self.persist_preferences();
            }
        } else if self.tree.is_empty() {
            self.active_note_id = None;
            self.load_note_into_editor("");
            self.persist_preferences();
        }
        Ok(())
    }

    fn sort_note_summaries(&self, notes: &mut [NoteSummary]) {
        match self.sort_mode {
            SortMode::Manual => {}
            SortMode::Updated => notes.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then(b.updated_at.cmp(&a.updated_at))
                    .then_with(|| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()))
            }),
            SortMode::Title => notes.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then_with(|| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()))
                    .then(b.updated_at.cmp(&a.updated_at))
            }),
        }
    }

    fn load_note_into_editor(&mut self, body: &str) {
        self.editor_buffer = EditorBuffer::from_text(body.to_string());
        self.sync_state_from_editor_buffer();
        self.dirty = false;
        self.lints.clear();
        self.lints_active = false;
        self.selection_anchor = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.preview_scroll = 0;
        self.editor_scroll = 0;
    }

    fn save_active_note(&mut self) -> Result<()> {
        if let Some(id) = self.active_note_id {
            self.store.update_note(id, &self.editor_buffer.to_text())?;
            self.refresh_notes()?;
            self.select_by_id(id);
            if self.mode != Mode::Edit {
                self.sync_active_note_from_cursor()?;
            } else {
                self.dirty = false;
            }
        }
        Ok(())
    }

    fn select_by_id(&mut self, id: i64) {
        if let Some(pos) = self.tree.iter().position(|item| {
            item.note().map(|n| n.id == id).unwrap_or(false)
        }) {
            self.tree_cursor = pos;
        }
    }

    fn commit_tree_inline(&mut self) -> Result<()> {
        let input = self.tree_inline_input.trim().to_string();
        if input.is_empty() {
            self.tree_inline_mode = TreeInlineMode::None;
            return Ok(());
        }
        match self.tree_inline_mode.clone() {
            TreeInlineMode::CreateFolder => {
                self.store.create_folder(&input)?;
                self.tree_expanded.insert(input.clone());
                self.status = format!("Created folder: {}", input);
            }
            TreeInlineMode::RenameFolder(old_name) => {
                self.store.rename_folder(&old_name, &input)?;
                let was_expanded = self.tree_expanded.remove(&old_name);
                if was_expanded { self.tree_expanded.insert(input.clone()); }
                self.status = format!("Renamed folder to: {}", input);
            }
            TreeInlineMode::RenameNote(id) => {
                if let Some(note) = self.store.get_note(id)? {
                    let mut lines: Vec<String> = note.body.lines().map(|l| l.to_string()).collect();
                    let new_body = if lines.is_empty() {
                        input.clone()
                    } else {
                        lines[0] = input.clone();
                        lines.join("\n")
                    };
                    self.store.update_note_with_title(id, &new_body, &input, true)?;
                }
                self.status = format!("Renamed to: {}", input);
            }
            TreeInlineMode::None => {}
        }
        self.tree_inline_mode = TreeInlineMode::None;
        self.tree_inline_input.clear();
        self.rebuild_tree()?;
        Ok(())
    }

    fn selected_note_ids_in_tree_order(&self) -> Vec<i64> {
        self.tree
            .iter()
            .filter_map(|item| item.note())
            .filter(|note| self.selected_note_ids.contains(&note.id))
            .map(|note| note.id)
            .collect()
    }

    fn move_selected_notes(&mut self, direction: i32) -> Result<bool> {
        let mut ids = self.selected_note_ids_in_tree_order();
        if ids.is_empty() {
            return Ok(false);
        }

        if direction > 0 {
            ids.reverse();
        }

        let anchor_id = if direction < 0 {
            *ids.first().unwrap()
        } else {
            *ids.last().unwrap()
        };

        let mut moved_any = false;
        for id in &ids {
            if let Some(cur) = self.tree.iter().position(|item| item.note().map(|n| n.id == *id).unwrap_or(false)) {
                let Some(note) = self.tree.get(cur).and_then(|item| item.note()).cloned() else {
                    continue;
                };
                if self.tree_shift_move_note(cur, note, direction)? {
                    moved_any = true;
                }
            }
        }

        if moved_any {
            if let Some(pos) = self.tree.iter().position(|item| item.note().map(|n| n.id == anchor_id).unwrap_or(false)) {
                self.tree_cursor = pos;
            }
            self.sync_active_note_from_cursor()?;
            let mut status = format!(
                "Moved {} selected note{}",
                ids.len(),
                if ids.len() == 1 { "" } else { "s" }
            );
            if self.sort_mode == SortMode::Manual {
                status.push_str(" (manual sort)");
            }
            self.status = status;
        } else {
            self.status = "Selected notes can't move further".to_string();
        }

        Ok(true)
    }

    fn tree_shift_move_note(&mut self, cur: usize, note: NoteSummary, direction: i32) -> Result<bool> {
        let note_folder = note.folder.clone();
        let note_id = note.id;

        let target_idx = if direction < 0 {
            if cur == 0 { return Ok(false); }
            cur - 1
        } else {
            if cur + 1 >= self.tree.len() { return Ok(false); }
            cur + 1
        };

        match self.tree.get(target_idx).cloned() {
            Some(TreeItem::Note(other_note)) if other_note.folder == note_folder => {
                if note.note_order == other_note.note_order {
                    self.normalize_note_orders_in_group(&note_folder)?;
                }
                self.store.swap_note_order(note_id, other_note.id)?;
                self.rebuild_tree()?;
                self.tree_cursor = target_idx;
                Ok(true)
            }
            Some(TreeItem::Note(other_note)) => {
                let dest_folder = other_note.folder.clone();
                if note.note_order == other_note.note_order {
                    self.normalize_note_orders_in_group(&dest_folder)?;
                }
                self.store.set_folder(note_id, &dest_folder)?;
                self.store.swap_note_order(note_id, other_note.id)?;
                self.rebuild_tree()?;
                self.tree_cursor = self.tree.iter().position(|i| i.note().map(|n| n.id == note_id).unwrap_or(false)).unwrap_or(0);
                Ok(true)
            }
            Some(TreeItem::Folder { name: ref folder_name, .. }) => {
                let folder_name = folder_name.clone();
                let folder_sort_order = self
                    .store
                    .list_folders()?
                    .into_iter()
                    .find(|folder| folder.name == folder_name)
                    .map(|folder| folder.sort_order)
                    .unwrap_or(0);
                if direction < 0 {
                    if target_idx == 0 {
                        self.store.set_folder(note_id, "")?;
                        self.store.set_note_order(note_id, folder_sort_order - 1)?;
                        self.rebuild_tree()?;
                        self.tree_cursor = self.tree.iter().position(|i| i.note().map(|n| n.id == note_id).unwrap_or(false)).unwrap_or(0);
                    } else {
                        match self.tree.get(target_idx - 1).cloned() {
                            Some(TreeItem::Note(prev_note)) => {
                                let prev_folder = prev_note.folder.clone();
                                if note.note_order == prev_note.note_order {
                                    self.normalize_note_orders_in_group(&prev_folder)?;
                                }
                                self.store.set_folder(note_id, &prev_folder)?;
                                self.store.swap_note_order(note_id, prev_note.id)?;
                                self.rebuild_tree()?;
                                self.tree_cursor = self.tree.iter().position(|i| i.note().map(|n| n.id == note_id).unwrap_or(false)).unwrap_or(0);
                            }
                            _ => {
                                self.store.set_folder(note_id, "")?;
                                self.store.set_note_order(note_id, folder_sort_order - 1)?;
                                self.rebuild_tree()?;
                                self.tree_cursor = self.tree.iter().position(|i| i.note().map(|n| n.id == note_id).unwrap_or(false)).unwrap_or(0);
                            }
                        }
                    }
                } else {
                    self.tree_expanded.insert(folder_name.clone());
                    self.store.set_folder(note_id, &folder_name)?;
                    self.rebuild_tree()?;
                    self.tree_cursor = self.tree.iter().position(|i| i.note().map(|n| n.id == note_id).unwrap_or(false)).unwrap_or(0);
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn tree_shift_move(&mut self, direction: i32) -> Result<()> {
        let cur = self.tree_cursor;
        if self.tree.is_empty() { return Ok(()); }

        let switched_to_manual = if self.sort_mode != SortMode::Manual {
            self.sort_mode = SortMode::Manual;
            self.persist_preferences();
            self.refresh_notes()?;
            if let Some(id) = self.active_note_id {
                self.select_by_id(id);
            }
            self.sync_active_note_from_cursor()?;
            true
        } else {
            false
        };

        if !self.selected_note_ids_in_view().is_empty() {
            self.move_selected_notes(direction)?;
            if switched_to_manual && !self.status.contains("manual sort") {
                self.status.push_str(" (manual sort)");
            }
            return Ok(());
        }

        match self.tree.get(cur).cloned() {
            Some(TreeItem::Folder { name: folder_name, .. }) => {
                let folders = self.store.list_folders()?;
                let pos = folders.iter().position(|f| f.name == folder_name);
                if let Some(idx) = pos {
                    let swap_idx = if direction < 0 {
                        if idx == 0 { return Ok(()); }
                        idx - 1
                    } else {
                        if idx + 1 >= folders.len() { return Ok(()); }
                        idx + 1
                    };
                    self.store.swap_folder_order(&folder_name, &folders[swap_idx].name)?;
                    self.rebuild_tree()?;
                    if let Some(new_pos) = self.tree.iter().position(|item| item.folder_name() == Some(&folder_name)) {
                        self.tree_cursor = new_pos;
                    }
                }
            }
            Some(TreeItem::Note(note)) => {
                self.tree_shift_move_note(cur, note, direction)?;
                if switched_to_manual {
                    self.status = "Moved note (manual sort)".to_string();
                }
            }
            None => {}
        }
        Ok(())
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
                    let lang = trimmed.trim_start_matches(fence_char).trim().trim_end_matches('`').trim();
                    let syntax = if lang.is_empty() {
                        self.syntax_set.find_syntax_plain_text()
                    } else {
                        let lower = lang.to_lowercase();
                        self.syntax_set.find_syntax_by_token(lang)
                            .or_else(|| self.syntax_set.syntaxes().iter().find(|s| s.name.to_lowercase() == lower))
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

    fn editor_view(&mut self, area: Rect, palette: Palette) -> (Text<'static>, u16, u16, u16) {
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

        // Sticky scroll: only move the viewport when the cursor leaves it.
        let visual_row_offset = if cursor_visual_row < self.editor_scroll {
            // Cursor moved above the top of the viewport — scroll up to cursor.
            cursor_visual_row
        } else if height > 0 && cursor_visual_row >= self.editor_scroll + height {
            // Cursor moved below the bottom of the viewport — scroll down minimally.
            cursor_visual_row.saturating_sub(height.saturating_sub(1))
        } else {
            // Cursor is still within the viewport — don't scroll.
            self.editor_scroll
        };
        self.editor_scroll = visual_row_offset;

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

        let hint = if lint.suggestions.is_empty() {
            "  (no fix available)"
        } else {
            "  Tab: apply"
        };

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

        let status_height = 1u16;
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
            (String::new(), palette.muted)
        };

        let note_count = self.tree.iter().filter(|i| i.is_note()).count();
        let selected_count = self.selected_note_ids_in_view().len();
        let top_text = Text::from(Line::from(vec![
            TSpan::styled(
                format!(
                    " anote  theme:{}  keymap:{}  sort:{}  notes:{}  sel:{}  {}",
                    self.theme.label(),
                    self.keymap.label(),
                    self.sort_mode.label(),
                    note_count,
                    selected_count,
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

        let notes_area = main[0];

        // Build tree list items
        let list_items: Vec<ListItem> = {
            let mut items: Vec<ListItem> = Vec::new();
            let tree_len = self.tree.len();

            if tree_len == 0 && self.tree_inline_mode != TreeInlineMode::CreateFolder {
                let empty_message = if self.query.is_empty() {
                    "No notes yet. Press 'n' to create or 'f' for a folder.".to_string()
                } else {
                    format!("No notes match '{}'. Press '/' to refine or clear the search.", self.query)
                };
                items.push(ListItem::new(Line::styled(
                    empty_message,
                    Style::default().fg(palette.muted),
                )));
            }

            for idx in 0..tree_len {
                let item = &self.tree[idx];
                let is_selected = idx == self.tree_cursor;
                let base_style = if is_selected {
                    Style::default().bg(palette.accent).fg(palette.bg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.text)
                };

                let list_item = match item {
                    TreeItem::Folder { name, expanded, note_count } => {
                        // If renaming this folder, show inline input
                        if matches!(&self.tree_inline_mode, TreeInlineMode::RenameFolder(n) if n == name) {
                            let input_line = format!("\u{f0153} {}█", self.tree_inline_input);
                            ListItem::new(TSpan::styled(input_line, Style::default().fg(palette.accent)))
                        } else {
                            let icon = if *expanded { "\u{f0176} " } else { "\u{f0153} " };
                            let count_text = format!("  ({})", note_count);
                            let count_style = if is_selected {
                                Style::default().fg(palette.bg)
                            } else {
                                Style::default().fg(palette.muted)
                            };
                            let spans = vec![
                                TSpan::styled(icon.to_string(), if is_selected { base_style } else { Style::default().fg(palette.accent) }),
                                TSpan::styled(name.clone(), base_style),
                                TSpan::styled(count_text, count_style),
                            ];
                            ListItem::new(Line::from(spans))
                        }
                    }
                    TreeItem::Note(note) => {
                        let is_marked = self.selected_note_ids.contains(&note.id);
                        let in_folder = !note.folder.is_empty();
                        let is_last_in_group = {
                            let next = self.tree.get(idx + 1);
                            match next {
                                None => true,
                                Some(TreeItem::Note(n)) => n.folder != note.folder,
                                Some(TreeItem::Folder { .. }) => true,
                            }
                        };
                        let tree_prefix = if in_folder {
                            if is_last_in_group { "\u{2514} " } else { "\u{251c} " }
                        } else {
                            "  "
                        };
                        let note_icon = if note.pinned { "\u{f0403} " } else { "\u{f0219} " };
                        let selection_mark = if is_marked { "[x] " } else { "[ ] " };

                        // If renaming this note, show inline input
                        if matches!(&self.tree_inline_mode, TreeInlineMode::RenameNote(id) if *id == note.id) {
                            let input_line = format!("{}{} {}█", tree_prefix, note_icon, self.tree_inline_input);
                            ListItem::new(TSpan::styled(input_line, Style::default().fg(palette.accent)))
                        } else {
                            let mut spans = vec![
                                TSpan::styled(tree_prefix.to_string(), Style::default().fg(palette.muted)),
                                TSpan::styled(selection_mark.to_string(), if is_selected { base_style } else { Style::default().fg(palette.muted) }),
                                TSpan::styled(note_icon.to_string(), if is_selected { base_style } else { Style::default().fg(palette.muted) }),
                                TSpan::styled(note.title.clone(), base_style),
                            ];
                            for tag in note.tags.split_whitespace() {
                                spans.push(TSpan::raw(" "));
                                spans.push(TSpan::styled(
                                    "●",
                                    tag_dot_style(self.theme, tag, self.tag_color_key_for(tag)),
                                ));
                            }
                            if !note.snippet.is_empty() && !self.query.is_empty() {
                                ListItem::new(Text::from(vec![
                                    Line::from(spans),
                                    Line::from(vec![
                                        TSpan::styled(
                                            format!("    {}", note.snippet.replace(['[', ']'], "")),
                                            if is_selected {
                                                Style::default().bg(palette.accent).fg(palette.bg)
                                            } else {
                                                Style::default().fg(palette.muted)
                                            },
                                        ),
                                    ]),
                                ]))
                            } else {
                                ListItem::new(Line::from(spans))
                            }
                        }
                    }
                };
                items.push(list_item);

                // After current item, insert create-folder input if in CreateFolder mode
                if self.tree_inline_mode == TreeInlineMode::CreateFolder && idx == self.tree_cursor {
                    let input_line = format!("\u{f0153} {}█", self.tree_inline_input);
                    items.push(ListItem::new(TSpan::styled(input_line, Style::default().fg(palette.accent))));
                }
            }

            // If tree is empty and creating folder
            if tree_len == 0 && self.tree_inline_mode == TreeInlineMode::CreateFolder {
                let input_line = format!("\u{f0153} {}█", self.tree_inline_input);
                items.push(ListItem::new(TSpan::styled(input_line, Style::default().fg(palette.accent))));
            }

            items
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
        frame.render_widget(Clear, notes_area);
        if self.mode != Mode::Edit {
            let mut list_state = ratatui::widgets::ListState::default();
            list_state.select(Some(self.tree_cursor));
            frame.render_stateful_widget(
                List::new(list_items).block(list_block).highlight_style(Style::default()),
                notes_area,
                &mut list_state,
            );
        }

        let meta_base = Style::default().bg(palette.panel).fg(palette.muted);
        let title_style = Style::default()
            .bg(palette.panel)
            .fg(palette.text)
            .add_modifier(Modifier::BOLD);
        let header_lines: Vec<Line<'static>> = if let Some(summary) = self.active_summary() {
            let mut lines = vec![Line::from(vec![TSpan::styled(summary.title.clone(), title_style)])];

            let mut meta_spans: Vec<TSpan<'static>> = vec![
                TSpan::styled(format!("updated {}", short_timestamp(&summary.updated_at)), meta_base),
                TSpan::styled(format!("  id:{}", summary.id), meta_base),
            ];
            if !summary.folder.is_empty() {
                meta_spans.push(TSpan::styled(
                    format!("  folder:{}", summary.folder),
                    meta_base,
                ));
            }
            lines.push(Line::from(meta_spans));

            if !summary.tags.is_empty() {
                let mut tag_spans: Vec<TSpan<'static>> = Vec::new();
                for (idx, tag) in summary.tags.split_whitespace().enumerate() {
                    if idx > 0 {
                        tag_spans.push(TSpan::raw(" "));
                    }
                    for span in tag_pill_spans(self.theme, tag, self.tag_color_key_for(tag), palette.panel) {
                        tag_spans.push(span);
                    }
                }
                lines.push(Line::from(tag_spans));
            }

            lines
        } else {
            vec![Line::styled("no note selected", meta_base)]
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
            Mode::Switcher => " Preview ",
            Mode::ArchiveBrowser => " Preview ",
            Mode::TrashBrowser => " Preview ",
            Mode::Tags => " Preview ",
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
            .constraints([Constraint::Length(header_lines.len() as u16), Constraint::Min(1)])
            .split(editor_inner);

        self.editor_col_width = editor_layout[1].width as usize;
        self.editor_row_height = editor_layout[1].height as usize;

        let meta = Paragraph::new(Text::from(header_lines));
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
                &self.syntax_set,
                &self.theme_set,
                &preview_highlight_terms(&self.query),
            );
            let preview = Paragraph::new(md_text)
                .style(Style::default().fg(palette.text).bg(palette.panel))
                .wrap(Wrap { trim: false })
                .scroll((self.preview_scroll, 0));
            frame.render_widget(preview, editor_layout[1]);
        }

        if self.mode == Mode::Edit {
            frame.set_cursor_position((cursor_x, cursor_y));
        }

        if self.mode == Mode::Edit {
            if let Some(idx) = self.lint_index_at_cursor() {
                if let Some(lint) = self.lints.get(idx) {
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

        let mode_text = match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Edit => "EDIT",
            Mode::Search => "SEARCH",
            Mode::Command => "COMMAND",
            Mode::Find => "FIND",
            Mode::Switcher => "SWITCH",
            Mode::ArchiveBrowser => "ARCHIVE",
            Mode::TrashBrowser => "TRASH",
            Mode::Tags => "TAGS",
            Mode::Help => "HELP",
        };
        let dirty_text = if self.dirty { "*" } else { "" };

        let inline_hint = if self.tree_inline_mode != TreeInlineMode::None {
            match &self.tree_inline_mode {
                TreeInlineMode::CreateFolder => format!("  New folder: {}█  Enter confirm  Esc cancel", self.tree_inline_input),
                TreeInlineMode::RenameFolder(_) => format!("  Rename: {}█  Enter confirm  Esc cancel", self.tree_inline_input),
                TreeInlineMode::RenameNote(_) => format!("  Rename: {}█  Enter confirm  Esc cancel", self.tree_inline_input),
                TreeInlineMode::None => String::new(),
            }
        } else {
            String::new()
        };

        let footer_width = layout[2].width as usize;
        let status_line = match self.mode {
            Mode::Search => fit_footer_left(&format!("/{}", self.search_input), footer_width),
            Mode::Command => fit_footer_left(&format!(":{}", self.command_input), footer_width),
            Mode::Switcher => fit_footer_segments(
                &format!("jump:{}", self.switcher_input),
                &["j/k move", "Enter open", "Bksp edit", "Esc close"],
                footer_width,
            ),
            Mode::ArchiveBrowser => fit_footer_segments(
                &format!("[ARCHIVE] {}", self.note_browser_input),
                &["j/k move", "Bksp filter", "x mark", "* all", "U unarchive", "D trash", "Esc close"],
                footer_width,
            ),
            Mode::TrashBrowser => fit_footer_segments(
                &format!("[TRASH] {}", self.note_browser_input),
                &["j/k move", "Bksp filter", "x mark", "* all", "R restore", "P purge", "Esc close"],
                footer_width,
            ),
            Mode::Tags => {
                let (left, hints): (String, Vec<&str>) = match self.tag_browser_mode {
                    TagBrowserMode::Browse => (
                        "[TAGS] browse tags".to_string(),
                        vec!["j/k move", "Enter filter", "n new", "c color", "D delete", "Esc close"],
                    ),
                    TagBrowserMode::Create => (
                        format!("[TAGS] new #{}", self.tag_browser_input),
                        vec!["Enter create", "Bksp edit", "Esc cancel"],
                    ),
                    TagBrowserMode::Color => (
                        "[TAGS] choose color".to_string(),
                        vec!["←→ pick", "Enter save", "Esc cancel"],
                    ),
                    TagBrowserMode::DeleteConfirm => (
                        "[TAGS] delete tag".to_string(),
                        vec!["Enter confirm", "D confirm", "Esc cancel"],
                    ),
                };
                fit_footer_segments(&left, &hints, footer_width)
            }
            Mode::Help => fit_footer_segments(
                &format!("[{}] {}", mode_text, self.status),
                &["j/k scroll", "PgUp/PgDn", "Esc close"],
                footer_width,
            ),
            Mode::Edit | Mode::Find => {
                let left = format!("[{}] {} {}", mode_text, self.status, dirty_text);
                let hints: Vec<&str> = if self.mode == Mode::Find {
                    vec!["Esc close", "Enter edit", "Bksp query"]
                } else if self.lints_active {
                    vec!["Esc preview", "Ctrl+S save", "Ctrl+L lint", "Tab fix", "]/[ jump"]
                } else {
                    vec!["Esc preview", "Ctrl+S save", "Ctrl+F find", "Ctrl+L lint"]
                };
                fit_footer_segments(&left, &hints, footer_width)
            }
            _ => {
                if !inline_hint.is_empty() {
                    fit_footer_left(&inline_hint, footer_width)
                } else {
                    let left = format!("[{}] {} {}", mode_text, self.status, dirty_text);
                    let hints: Vec<&str> = if self.quit_pending {
                        vec!["q force quit", ":wq save+quit", "any key cancel"]
                    } else if self.archive_pending {
                        vec!["a confirm", "any key cancel"]
                    } else if self.delete_pending {
                        vec!["d confirm", "any key cancel"]
                    } else if selected_count > 0 {
                        vec!["a archive", "D trash", "p pin", "u clear"]
                    } else if self.notes_pane_collapsed {
                        vec!["j/k scroll", "PgUp/PgDn", "\\ notes", "F9 sort", "? help", "q quit"]
                    } else {
                        vec![": command", "x mark", "* all", "A archive", "T trash", "g tags", "/ search", "F9 sort"]
                    };
                    fit_footer_segments(&left, &hints, footer_width)
                }
            }
        };

        let status_style = if self.delete_pending || self.archive_pending {
            Style::default()
                .bg(palette.danger)
                .fg(palette.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(palette.panel)
                .fg(if self.mode == Mode::Command {
                    palette.accent
                } else if self.dirty {
                    palette.danger
                } else {
                    palette.ok
                })
                .add_modifier(Modifier::BOLD)
        };
        let status = Paragraph::new(status_line).style(status_style);
        frame.render_widget(status, layout[2]);

        if self.mode == Mode::Switcher {
            self.render_switcher_overlay(frame, palette);
        }

        if matches!(self.mode, Mode::ArchiveBrowser | Mode::TrashBrowser) {
            self.render_note_browser_overlay(frame, palette);
        }

        if self.mode == Mode::Tags {
            self.render_tag_browser_overlay(frame, palette);
        }

        if self.mode == Mode::Help {
            self.render_help_overlay(frame, palette);
        }
    }

    fn render_switcher_overlay(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        let w = area.width.min(72);
        let h = area.height.min(16);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect { x, y, width: w, height: h };
        let inner = Block::default()
            .title(" Jump To Note ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.accent))
            .style(Style::default().bg(palette.bg).fg(palette.text))
            .inner(popup);

        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title(" Jump To Note ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.accent))
                .style(Style::default().bg(palette.bg).fg(palette.text)),
            popup,
        );

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        frame.render_widget(
            Paragraph::new(format!("> {}", self.switcher_input))
                .style(Style::default().fg(palette.accent).bg(palette.bg)),
            chunks[0],
        );

        let items: Vec<ListItem> = if self.switcher_results.is_empty() {
            vec![ListItem::new(Line::styled(
                "No matching notes",
                Style::default().fg(palette.muted),
            ))]
        } else {
            self.switcher_results
                .iter()
                .enumerate()
                .map(|(idx, note)| {
                    let selected = idx == self.switcher_cursor;
                    let rail_style = if selected {
                        Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.bg)
                    };
                    let title_style = if selected {
                        Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.text)
                    };
                    let meta = format!(
                        "{}{}",
                        if note.archived { "archived" } else { "active" },
                        if note.folder.is_empty() { "".to_string() } else { format!("  /{}", note.folder) },
                    );
                    let snippet = if note.snippet.is_empty() {
                        short_timestamp(&note.updated_at)
                    } else {
                        note.snippet.replace(['[', ']'], "")
                    };
                    let meta_style = if selected {
                        Style::default().fg(palette.text)
                    } else {
                        Style::default().fg(palette.muted)
                    };
                    let mut lines = vec![
                        Line::from(vec![
                            TSpan::styled(if selected { "▍ " } else { "  " }, rail_style),
                            TSpan::styled(note.title.clone(), title_style),
                        ]),
                        Line::from(vec![TSpan::styled(
                            format!("  {}  {}", meta, snippet),
                            meta_style,
                        )]),
                    ];

                    if !note.tags.is_empty() {
                        let row_bg = palette.bg;
                        let mut tag_spans = vec![TSpan::styled("  ".to_string(), Style::default().bg(row_bg))];
                        for (tag_idx, tag) in note.tags.split_whitespace().enumerate() {
                            if tag_idx > 0 {
                                tag_spans.push(TSpan::raw(" "));
                            }
                            tag_spans.extend(tag_pill_spans(
                                self.theme,
                                tag,
                                self.tag_color_key_for(tag),
                                row_bg,
                            ));
                        }
                        lines.push(Line::from(tag_spans));
                    }

                    ListItem::new(Text::from(lines))
                })
                .collect()
        };
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(self.switcher_cursor.min(items.len().saturating_sub(1))));
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default()),
            chunks[1],
            &mut list_state,
        );
    }

    fn render_note_browser_overlay(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        let w = area.width.min(72);
        let h = area.height.min(20);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect { x, y, width: w, height: h };
        let title = if self.mode == Mode::ArchiveBrowser {
            " Archive Browser "
        } else {
            " Trash Browser "
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.accent))
            .style(Style::default().bg(palette.bg).fg(palette.text));
        let inner = block.inner(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(6), Constraint::Length(3)])
            .split(inner);

        frame.render_widget(
            Paragraph::new(format!("> {}", self.note_browser_input))
                .style(Style::default().fg(palette.accent).bg(palette.bg)),
            chunks[0],
        );

        let items: Vec<ListItem> = if self.note_browser_results.is_empty() {
            vec![ListItem::new(Line::styled(
                if self.mode == Mode::ArchiveBrowser {
                    "No archived notes"
                } else {
                    "Trash is empty"
                },
                Style::default().fg(palette.muted),
            ))]
        } else {
            self.note_browser_results
                .iter()
                .enumerate()
                .map(|(idx, note)| {
                    let selected = idx == self.note_browser_cursor;
                    let marked = self.note_browser_selected_ids.contains(&note.id);
                    let rail_style = if selected {
                        Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.bg)
                    };
                    let title_style = if selected {
                        Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.text)
                    };
                    let meta_style = if selected {
                        Style::default().fg(palette.text)
                    } else {
                        Style::default().fg(palette.muted)
                    };
                    let meta = if note.folder.is_empty() {
                        short_timestamp(&note.updated_at)
                    } else {
                        format!("/{}  {}", note.folder, short_timestamp(&note.updated_at))
                    };
                    let mut lines = vec![
                        Line::from(vec![
                            TSpan::styled(if selected { "▍ " } else { "  " }, rail_style),
                            TSpan::styled(if marked { "[x] " } else { "[ ] " }, meta_style),
                            TSpan::styled(note.title.clone(), title_style),
                        ]),
                        Line::from(vec![TSpan::styled(format!("  {}", meta), meta_style)]),
                    ];
                    if !note.tags.is_empty() {
                        let mut tag_spans = vec![TSpan::styled("  ".to_string(), Style::default().bg(palette.bg))];
                        for (tag_idx, tag) in note.tags.split_whitespace().enumerate() {
                            if tag_idx > 0 {
                                tag_spans.push(TSpan::raw(" "));
                            }
                            tag_spans.extend(tag_pill_spans(self.theme, tag, self.tag_color_key_for(tag), palette.bg));
                        }
                        lines.push(Line::from(tag_spans));
                    }
                    ListItem::new(Text::from(lines))
                })
                .collect()
        };
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(self.note_browser_cursor.min(items.len().saturating_sub(1))));
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default()),
            chunks[1],
            &mut list_state,
        );

        let detail_lines = if let Some(note) = self.note_browser_results.get(self.note_browser_cursor) {
            let action = if self.mode == Mode::ArchiveBrowser {
                "U unarchive  D trash"
            } else {
                "R restore  P purge"
            };
            vec![
                Line::from(vec![
                    TSpan::styled(note.title.clone(), Style::default().fg(palette.text).add_modifier(Modifier::BOLD)),
                    TSpan::styled(format!("  {}", action), Style::default().fg(palette.muted)),
                ]),
                Line::styled(
                    if note.snippet.is_empty() { short_timestamp(&note.updated_at) } else { note.snippet.replace(['[', ']'], "") },
                    Style::default().fg(palette.muted),
                ),
            ]
        } else {
            vec![Line::styled("Esc closes the browser", Style::default().fg(palette.muted))]
        };
        frame.render_widget(Paragraph::new(detail_lines), chunks[2]);
    }

    fn render_tag_browser_overlay(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        let w = area.width.min(60);
        let h = area.height.min(22);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect { x, y, width: w, height: h };
        let title = match self.tag_browser_mode {
            TagBrowserMode::Browse => " Tags ",
            TagBrowserMode::Create => " Tags • New ",
            TagBrowserMode::Color => " Tags • Color ",
            TagBrowserMode::DeleteConfirm => " Tags • Delete ",
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.accent))
            .style(Style::default().bg(palette.bg).fg(palette.text));
        let inner = block.inner(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);

        let detail_height = match self.tag_browser_mode {
            TagBrowserMode::Color => 10,
            TagBrowserMode::DeleteConfirm => 5,
            _ => 4,
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(detail_height)])
            .split(inner);

        let items: Vec<ListItem> = if self.tag_browser_entries.is_empty() {
            vec![ListItem::new(Line::styled("No tags yet", Style::default().fg(palette.muted)))]
        } else {
            self.tag_browser_entries
                .iter()
                .map(|entry| {
                    let mut spans = vec![
                        TSpan::styled(
                            format!("#{}", entry.tag),
                            Style::default()
                                .fg(palette.text)
                                .add_modifier(Modifier::BOLD),
                        ),
                        TSpan::styled(
                            format!(" ({})", entry.count),
                            Style::default().fg(palette.muted),
                        ),
                    ];

                    let color_label = entry.color.as_deref().unwrap_or("auto");
                    spans.push(TSpan::styled("  ", Style::default()));
                    spans.extend(tag_pill_spans(
                        self.theme,
                        &entry.tag,
                        entry.color.as_deref(),
                        palette.bg,
                    ));
                    spans.push(TSpan::styled(
                        format!("  {}", color_label),
                        Style::default().fg(palette.muted),
                    ));

                    ListItem::new(Line::from(spans))
                })
                .collect()
        };
        let mut list_state = ratatui::widgets::ListState::default();
        list_state.select(Some(self.tag_browser_cursor.min(items.len().saturating_sub(1))));
        frame.render_stateful_widget(
            List::new(items).highlight_style(
                Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
            ),
            chunks[0],
            &mut list_state,
        );

        let detail_lines: Vec<Line> = match self.tag_browser_mode {
            TagBrowserMode::Browse => {
                if let Some(entry) = self.selected_tag_entry() {
                    vec![
                        Line::from(vec![
                            TSpan::styled("Selected ", Style::default().fg(palette.muted)),
                            TSpan::styled(format!("#{}", entry.tag), Style::default().fg(palette.text).add_modifier(Modifier::BOLD)),
                            TSpan::styled(
                                format!("  {} note{}", entry.count, if entry.count == 1 { "" } else { "s" }),
                                Style::default().fg(palette.muted),
                            ),
                        ]),
                        Line::from(vec![
                            TSpan::styled("Color ", Style::default().fg(palette.muted)),
                            TSpan::styled(
                                entry.color.as_deref().unwrap_or("auto"),
                                Style::default().fg(palette.accent),
                            ),
                            TSpan::styled("  n new  c color  Enter filter", Style::default().fg(palette.muted)),
                        ]),
                    ]
                } else {
                    vec![
                        Line::styled("Create your first tag with n", Style::default().fg(palette.muted)),
                        Line::styled("Enter filters notes by the selected tag", Style::default().fg(palette.muted)),
                    ]
                }
            }
            TagBrowserMode::Create => vec![
                Line::from(vec![
                    TSpan::styled("New tag ", Style::default().fg(palette.muted)),
                    TSpan::styled(format!("#{}█", self.tag_browser_input), Style::default().fg(palette.text)),
                ]),
                Line::styled(
                    "Letters, numbers, '-' and '_' only. Enter creates, then opens color picker.",
                    Style::default().fg(palette.muted),
                ),
            ],
            TagBrowserMode::Color => {
                let choices = self.theme.tag_color_choices();
                let current_name = self.selected_tag_name().unwrap_or("");
                let label = if self.tag_color_cursor == 0 {
                    "auto".to_string()
                } else {
                    choices[self.tag_color_cursor - 1].label.to_string()
                };
                let preview_color = if self.tag_color_cursor == 0 {
                    None
                } else {
                    Some(choices[self.tag_color_cursor - 1].key)
                };

                let mut lines = vec![
                    Line::from(vec![
                        TSpan::styled(format!("Color for #{} ", current_name), Style::default().fg(palette.muted)),
                        TSpan::styled(label, Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)),
                    ]),
                    Line::from(vec![
                        TSpan::styled("Preview ", Style::default().fg(palette.muted)),
                        TSpan::raw(" "),
                        TSpan::styled("← →", Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)),
                        TSpan::styled(" choose  Enter save  Esc cancel", Style::default().fg(palette.muted)),
                    ]),
                    Line::from(tag_pill_spans(
                        self.theme,
                        current_name,
                        preview_color,
                        palette.bg,
                    )),
                    Line::raw(""),
                ];

                let mut option_rows = vec![
                    vec![Some(0usize), Some(1usize)],
                    vec![Some(2usize), Some(3usize)],
                    vec![Some(4usize), Some(5usize)],
                    vec![Some(6usize), Some(7usize)],
                    vec![Some(8usize), None],
                ];

                for row in option_rows.drain(..) {
                    let mut spans = Vec::new();
                    for (col_idx, opt_idx) in row.into_iter().enumerate() {
                        if col_idx > 0 {
                            spans.push(TSpan::raw("   "));
                        }
                        spans.extend(color_choice_entry_spans(
                            self.theme,
                            palette,
                            opt_idx,
                            self.tag_color_cursor,
                        ));
                    }
                    lines.push(Line::from(spans));
                }

                lines
            }
            TagBrowserMode::DeleteConfirm => {
                if let Some(entry) = self.selected_tag_entry() {
                    vec![
                        Line::from(vec![
                            TSpan::styled("Delete ", Style::default().fg(palette.muted)),
                            TSpan::styled(
                                format!("#{}", entry.tag),
                                Style::default().fg(palette.danger).add_modifier(Modifier::BOLD),
                            ),
                            TSpan::styled(
                                format!(" from {} note{}?", entry.count, if entry.count == 1 { "" } else { "s" }),
                                Style::default().fg(palette.muted),
                            ),
                        ]),
                        Line::styled(
                            "This removes the tag from the first line of every matching note and deletes its saved color.",
                            Style::default().fg(palette.muted),
                        ),
                        Line::styled(
                            "Press Enter or D to confirm. Esc cancels.",
                            Style::default().fg(palette.danger),
                        ),
                    ]
                } else {
                    vec![Line::styled("No tag selected", Style::default().fg(palette.muted))]
                }
            }
        };

        let detail = Paragraph::new(detail_lines)
            .style(Style::default().bg(palette.bg).fg(palette.text))
            .wrap(Wrap { trim: false });
        frame.render_widget(detail, chunks[1]);
    }


    fn render_help_overlay(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        let w = area.width.min(70);
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
            row("j/k  ↑↓",        "navigate tree"),
            row("→ / Space/Enter", "expand folder or enter"),
            row("←",              "collapse folder (or go to parent)"),
            row("Enter / e",       "open note for edit"),
            row("n",               "new note in current folder"),
            row("f",               "new folder"),
            row("r",               "rename note or folder"),
            row("d d",             "delete  (any other key cancels)"),
            row("Shift+↑↓",        "move note/folder"),
            row("/",               "search notes"),
            row(":",               "command palette"),
            row("\\",              "toggle notes pane"),
            row("Ctrl+P",          "quick switcher"),
            row("g",               "browse and manage tags"),
            row("x / * / u",       "mark / all / clear"),
            row("p",               "pin selected notes or toggle current note"),
            row("A / T",           "open archive / trash browser"),
            row("F9",              "cycle sort"),
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
            row("Tab",             "apply lint fix (when lint active)"),
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
            row("] / [",           "next/prev lint (Normal mode only)"),
            pad(),
            heading("  SEARCH  (/)"),
            row("#tag",            "filter by tag (first line of note)"),
            row("/folder",         "filter by folder"),
            row(":archived",       "open archive browser on Enter"),
            row(":trash",          "open trash browser on Enter"),
            pad(),
            heading("  TAG BROWSER"),
            row("g / :tags",       "open tag browser"),
            row("j/k  ↑↓",         "move through tags"),
            row("Enter",           "filter notes by selected tag"),
            row("n",               "create a new tag"),
            row("c / e",           "choose a color for selected tag"),
            row("D",               "delete tag from all notes (confirm)"),
            row("Esc",             "close browser / cancel tag edit"),
            pad(),
            heading("  TRASH / ARCHIVE"),
            row("a",               "archive selected/current note"),
            row("A",               "open archive browser"),
            row("T",               "open trash browser"),
            row("D",               "move selected note(s) to trash"),
            row("j/k  ↑↓",         "move in archive / trash browser"),
            row("U / D",           "unarchive / trash in archive browser"),
            row("R / P",           "restore / purge in trash browser"),
            row("x / * / u",       "mark / all / clear in popup browser"),
            row("type / Bksp",     "filter popup browser notes"),
            row(":archive / :archive!", "archive current note / force confirm"),
            row(":unarchive",      "unarchive current note"),
            row(":archived",       "open archive browser"),
            row(":trash",          "open trash browser"),
            row(":restore",        "restore current trashed note"),
            row(":purge",          "permanently delete current trashed note"),
            row(":empty-trash",    "permanently delete all trashed notes"),
            pad(),
            heading("  COMMANDS  (:)"),
            row(":new",            "create note"),
            row(":import <path>",  "import file(s) as notes"),
            row(":export <path>",  "export current note"),
            row(":edit",           "enter edit mode"),
            row(":rename <title>", "rename note"),
            row(":folder <name>",  "move to folder (empty = root)"),
            row(":unfolder",       "remove from folder (move to root)"),
            row(":tag / :untag",   "add/remove #tag on first line"),
            row(":pin / :unpin",   "pin to top"),
            row(":discard",        "discard unsaved changes"),
            row(":search <q>",     "search"),
            row(":reload",         "refresh list from disk"),
            row(":theme <name>",   "neo-noir|paper|matrix"),
            row(":keymap <name>",  "default|vim"),
            row(":sort <mode>",    "manual|updated|title"),
            row(":tags",           "browse and manage tags"),
            row(":archived",       "open archive browser"),
            pad(),
            Line::from(vec![dim("  F6 theme  F7 keymap  F8 density  F9 sort  "), key("?/Esc"), dim(" close")]),
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

fn fit_footer_left(text: &str, width: usize) -> String {
    truncate_with_ellipsis(text, width)
}

fn fit_footer_segments(left: &str, hints: &[&str], width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let left = truncate_with_ellipsis(left.trim(), width);
    let left_len = left.chars().count();
    if left_len >= width || hints.is_empty() {
        return left;
    }

    let mut line = left;
    for hint in hints {
        let segment = format!(" | {}", hint);
        let seg_len = segment.chars().count();
        if line.chars().count() + seg_len > width {
            break;
        }
        line.push_str(&segment);
    }
    line
}

fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let trimmed = text.trim();
    let len = trimmed.chars().count();
    if len <= width {
        return trimmed.to_string();
    }

    if width == 1 {
        return "…".to_string();
    }

    let mut out: String = trimmed.chars().take(width - 1).collect();
    out.push('…');
    out
}

fn parse_command_parts(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '"' | '\'' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                } else {
                    current.push(ch);
                }
            }
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn tag_color_idx(tag: &str, len: usize) -> usize {
    tag.bytes()
        .fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize))
        % len
}

fn tag_color_choice_index(key: Option<&str>) -> usize {
    match key {
        None | Some("") => 0,
        Some(key) => TAG_COLOR_CHOICES
            .iter()
            .position(|choice| choice.key == key)
            .map(|idx| idx + 1)
            .unwrap_or(0),
    }
}

fn resolve_tag_colors(theme: ThemeName, tag: &str, color_key: Option<&str>) -> (Color, Color) {
    if let Some(key) = color_key {
        if let Some(choice) = theme.tag_color_choices().iter().find(|choice| choice.key == key) {
            return choice.colors(theme);
        }
    }

    let choices = theme.tag_color_choices();
    choices[tag_color_idx(tag, choices.len())].colors(theme)
}

fn tag_dot_style(theme: ThemeName, tag: &str, color_key: Option<&str>) -> Style {
    let (bg, _) = resolve_tag_colors(theme, tag, color_key);
    Style::default().fg(bg)
}

///// Returns spans for a rounded pill using Nerd Font powerline glyphs (requires Nerd Font).
/// `row_bg` should be the background color of the containing row so the caps blend in.
fn tag_pill_spans(theme: ThemeName, tag: &str, color_key: Option<&str>, row_bg: Color) -> Vec<TSpan<'static>> {
    let (bg, fg) = resolve_tag_colors(theme, tag, color_key);
    let cap = Style::default().fg(bg).bg(row_bg);
    let body = Style::default().bg(bg).fg(fg);
    vec![
        TSpan::styled("\u{E0B6}", cap),
        TSpan::styled(format!("#{} ", tag), body),
        TSpan::styled("\u{E0B4}", cap),
    ]
}

fn color_choice_entry_spans(
    theme: ThemeName,
    palette: Palette,
    choice_idx: Option<usize>,
    selected_idx: usize,
) -> Vec<TSpan<'static>> {
    match choice_idx {
        None => vec![TSpan::raw("")],
        Some(0) => {
            let selected = selected_idx == 0;
            vec![
                TSpan::styled(
                    if selected { "› " } else { "  " },
                    if selected {
                        Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.bg)
                    },
                ),
                TSpan::styled(
                    "auto",
                    if selected {
                        Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.muted)
                    },
                ),
            ]
        }
        Some(idx) => {
            let choice = TAG_COLOR_CHOICES[idx - 1];
            let selected = selected_idx == idx;
            let mut spans = vec![TSpan::styled(
                if selected { "› " } else { "  " },
                if selected {
                    Style::default().fg(choice.colors(theme).0).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.bg)
                },
            )];
            spans.extend(tag_pill_spans(theme, choice.key, Some(choice.key), palette.bg));
            spans.push(TSpan::styled(
                format!(" {}", choice.label),
                if selected {
                    Style::default().fg(choice.colors(theme).0).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.text)
                },
            ));
            spans
        }
    }
}

fn short_timestamp(ts: &str) -> String {
    ts.get(0..16).unwrap_or(ts).to_string()
}

fn is_tag_boundary(c: char) -> bool {
    !c.is_ascii_alphanumeric() && c != '_' && c != '-'
}

/// Returns true if the first line of `body` contains `#tag` as a whole tag token.
fn body_has_tag(body: &str, tag: &str) -> bool {
    let first_line = body.lines().next().unwrap_or("").to_ascii_lowercase();
    let needle = format!("#{}", tag);
    let mut pos = 0;
    while pos < first_line.len() {
        if let Some(found) = first_line[pos..].find(&needle) {
            let abs = pos + found;
            let after = abs + needle.len();
            let next_is_continuation = first_line[after..].chars().next()
                .map(|c| !is_tag_boundary(c))
                .unwrap_or(false);
            if !next_is_continuation {
                return true;
            }
            pos = abs + 1;
        } else {
            break;
        }
    }
    false
}

/// Appends ` #tag` to the end of the first line of `body`.
fn append_tag_to_body(body: &str, tag: &str) -> String {
    let token = format!(" #{}", tag);
    match body.find('\n') {
        Some(nl) => format!("{}{}{}", &body[..nl], token, &body[nl..]),
        None => format!("{}{}", body, token),
    }
}

/// Removes all whole-token occurrences of `#tag` from the first line of `body`.
fn remove_tag_from_body(body: &str, tag: &str) -> String {
    let nl = body.find('\n');
    let first_line = match nl {
        Some(pos) => &body[..pos],
        None => body,
    };
    let rest = match nl {
        Some(pos) => &body[pos..],
        None => "",
    };

    let needle = format!("#{}", tag);
    let mut line = first_line.to_string();
    let mut search_from = 0;
    loop {
        let lower = line[search_from..].to_ascii_lowercase();
        if let Some(found) = lower.find(&needle) {
            let abs = search_from + found;
            let after = abs + needle.len();
            let next_is_continuation = line[after..].chars().next()
                .map(|c| !is_tag_boundary(c))
                .unwrap_or(false);
            if next_is_continuation {
                search_from = abs + 1;
                continue;
            }
            // Eat a leading space before the token to avoid leaving double spaces
            let remove_start = if abs > 0 && line.as_bytes()[abs - 1] == b' ' {
                abs - 1
            } else {
                abs
            };
            // Or eat a trailing space after the token
            let remove_end = if line[after..].starts_with(' ') && remove_start == abs {
                after + 1
            } else {
                after
            };
            line = format!("{}{}", &line[..remove_start], &line[remove_end..]);
            search_from = remove_start;
        } else {
            break;
        }
    }

    format!("{}{}", line, rest)
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

fn fix_fences(text: &str) -> String {
    // CommonMark disallows backticks in a backtick-fence info string.
    // When the user writes ```lang``` (open+close on one line), strip the
    // trailing fence so pulldown-cmark sees a valid opening fence.
    text.lines()
        .map(|line| {
            let trimmed = line.trim_end();
            for fence in &["```", "~~~"] {
                if trimmed.starts_with(fence) && trimmed.ends_with(fence)
                    && trimmed.len() > fence.len() * 2
                {
                    let stripped = &trimmed[..trimmed.len() - fence.len()];
                    return format!("{}\n", stripped);
                }
            }
            format!("{}\n", line)
        })
        .collect()
}

fn render_markdown_preview(text: &str, palette: Palette, _width: usize, syntax_set: &SyntaxSet, theme_set: &ThemeSet, highlight_terms: &[String]) -> Text<'static> {
    let fixed = fix_fences(text);
    let opts = MdOptions::ENABLE_STRIKETHROUGH | MdOptions::ENABLE_TABLES;
    let parser = MdParser::new_ext(&fixed, opts);

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
    let preview_theme = theme_set.themes.get("base16-ocean.dark")
        .or_else(|| theme_set.themes.values().next());

    let mut in_heading: Option<HeadingLevel> = None;
    let mut in_bold = false;
    let mut in_italic = false;
    let in_code = false;
    let mut in_code_block = false;
    let mut code_highlighter: Option<HighlightLines> = None;
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
            MdEvent::Start(MdTag::CodeBlock(kind)) => {
                in_code_block = true;
                lines.push(Line::from(vec![]));
                if let (CodeBlockKind::Fenced(lang_cow), Some(theme)) = (&kind, preview_theme) {
                    let lang = lang_cow.trim().trim_end_matches('`').trim();
                    if !lang.is_empty() {
                        let lower = lang.to_lowercase();
                        let syntax = syntax_set.find_syntax_by_token(lang)
                            .or_else(|| syntax_set.syntaxes().iter().find(|s| s.name.to_lowercase() == lower))
                            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
                        code_highlighter = Some(HighlightLines::new(syntax, theme));
                    }
                }
            }
            MdEvent::End(MdTagEnd::CodeBlock) => {
                in_code_block = false;
                code_highlighter = None;
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
                    // Emit each line of the code block separately, with syntect highlighting if available
                    let lines_vec: Vec<&str> = s.lines().collect();
                    for (i, line) in lines_vec.iter().enumerate() {
                        let indent = "  ".repeat(list_depth.max(1).saturating_sub(1) + 1);
                        if let Some(hl) = code_highlighter.as_mut() {
                            let line_with_newline = format!("{}\n", line);
                            if let Ok(tokens) = hl.highlight_line(&line_with_newline, syntax_set) {
                                current_spans.push(TSpan::raw(indent));
                                for (syntect_style, token_str) in &tokens {
                                    let text = token_str.trim_end_matches('\n');
                                    if !text.is_empty() {
                                        let fg = syntect_style.foreground;
                                        let span_style = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
                                        current_spans.push(TSpan::styled(text.to_string(), span_style));
                                    }
                                }
                            } else {
                                current_spans.push(TSpan::styled(format!("{}{}", indent, line), code_style));
                            }
                        } else {
                            current_spans.push(TSpan::styled(format!("{}{}", indent, line), code_style));
                        }
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

    if highlight_terms.is_empty() {
        Text::from(lines)
    } else {
        Text::from(highlight_preview_lines(lines, highlight_terms, palette))
    }
}

fn preview_highlight_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|token| {
            !token.starts_with('#')
                && !token.starts_with('/')
                && !token.starts_with(':')
                && !token.is_empty()
        })
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn highlight_preview_lines(lines: Vec<Line<'static>>, terms: &[String], palette: Palette) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let text: String = line.spans.iter().map(|span| span.content.to_string()).collect();
            if text.is_empty() {
                return line;
            }
            let lower = text.to_ascii_lowercase();
            let mut marks = vec![false; lower.chars().count()];
            let chars: Vec<char> = lower.chars().collect();
            for term in terms {
                let tchars: Vec<char> = term.chars().collect();
                if tchars.is_empty() || tchars.len() > chars.len() {
                    continue;
                }
                for i in 0..=chars.len().saturating_sub(tchars.len()) {
                    if chars[i..i + tchars.len()] == tchars[..] {
                        for mark in marks.iter_mut().skip(i).take(tchars.len()) {
                            *mark = true;
                        }
                    }
                }
            }

            if !marks.iter().any(|marked| *marked) {
                return line;
            }

            let mut spans = Vec::new();
            let source_chars: Vec<char> = text.chars().collect();
            let mut current = String::new();
            let mut current_mark = marks[0];
            for (idx, ch) in source_chars.iter().enumerate() {
                if marks[idx] != current_mark {
                    let style = if current_mark {
                        Style::default().bg(palette.ok).fg(palette.bg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.text)
                    };
                    spans.push(TSpan::styled(std::mem::take(&mut current), style));
                    current_mark = marks[idx];
                }
                current.push(*ch);
            }
            if !current.is_empty() {
                let style = if current_mark {
                    Style::default().bg(palette.ok).fg(palette.bg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.text)
                };
                spans.push(TSpan::styled(current, style));
            }
            Line::from(spans)
        })
        .collect()
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
    fn tag_helpers_add_and_remove() {
        use super::{body_has_tag, append_tag_to_body, remove_tag_from_body};

        assert!(!body_has_tag("hello world", "rust"));
        assert!(body_has_tag("hello #rust world", "rust"));
        assert!(!body_has_tag("hello #rustacean world", "rust")); // prefix, not whole tag
        assert!(body_has_tag("hello #Rust world", "rust")); // case-insensitive

        // append_tag_to_body appends to the first line
        let body = append_tag_to_body("hello", "rust");
        assert_eq!(body, "hello #rust");
        assert!(body_has_tag(&body, "rust"));

        // body on multiple lines: tag goes on first line only
        let body_ml = append_tag_to_body("hello\nbody text", "rust");
        assert_eq!(body_ml, "hello #rust\nbody text");
        assert!(body_has_tag(&body_ml, "rust"));
        assert!(!body_has_tag("first line\nbody text #rust", "rust")); // only first line counts

        // remove from first line
        let removed = remove_tag_from_body("hello #rust\nbody", "rust");
        assert_eq!(removed, "hello\nbody");
        assert!(!body_has_tag(&removed, "rust"));

        // Should not touch a tag with same prefix
        let body2 = "notes #rustacean #rust end";
        let removed2 = remove_tag_from_body(body2, "rust");
        assert!(removed2.contains("#rustacean"));
        assert!(!body_has_tag(&removed2, "rust"));
    }

    #[test]
    fn normalize_paste_keeps_tab_stops_consistent() {
        assert_eq!(normalize_pasted_text("\tX", 0, 4), "    X");
        assert_eq!(normalize_pasted_text("\tX", 3, 4), " X");
    }

    #[test]
    fn folder_expand_collapse_roundtrip() {
        use crate::storage::Store;
        use super::{App, TreeItem};
        let store = Store::open_for_test().unwrap();
        store.create_folder("Work").unwrap();
        let id = store.create_note("Note A", "").unwrap();
        store.set_folder(id, "Work").unwrap();
        let id2 = store.create_note("Note B", "").unwrap();
        store.set_folder(id2, "Work").unwrap();

        let mut app = App::new(store).unwrap();
        // Starts collapsed: only the folder row, no notes
        assert_eq!(app.tree.len(), 1, "collapsed: only folder header");
        assert!(matches!(app.tree[0], TreeItem::Folder { expanded: false, .. }));

        // Expand
        app.tree_expanded.insert("Work".to_string());
        app.rebuild_tree().unwrap();
        assert_eq!(app.tree.len(), 3, "expanded: folder + 2 notes");
        assert!(matches!(app.tree[0], TreeItem::Folder { expanded: true, .. }));
        assert!(matches!(app.tree[1], TreeItem::Note(_)));
        assert!(matches!(app.tree[2], TreeItem::Note(_)));

        // Collapse
        app.tree_expanded.remove("Work");
        app.rebuild_tree().unwrap();
        assert_eq!(app.tree.len(), 1, "collapsed again: only folder header");
        assert!(matches!(app.tree[0], TreeItem::Folder { expanded: false, .. }));
    }

    #[test]
    fn selected_notes_move_together_down() {
        use crate::storage::Store;
        use super::{App, TreeItem};

        let store = Store::open_for_test().unwrap();
        let a = store.create_note("A", "").unwrap();
        let b = store.create_note("B", "").unwrap();
        let c = store.create_note("C", "").unwrap();
        store.set_note_order(a, 10).unwrap();
        store.set_note_order(b, 20).unwrap();
        store.set_note_order(c, 30).unwrap();

        let mut app = App::new(store).unwrap();
        app.selected_note_ids.insert(a);
        app.selected_note_ids.insert(b);

        app.move_selected_notes(1).unwrap();

        let titles: Vec<String> = app
            .tree
            .iter()
            .filter_map(|item| match item {
                TreeItem::Note(note) => Some(note.title.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, vec!["C", "A", "B"]);
    }
}
