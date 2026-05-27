use ratatui::style::Color;

use crate::storage::NoteSummary;

// ── Mode ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Edit,
    Search,
    Command,
    Find,
    Switcher,
    CommandPalette,
    ArchiveBrowser,
    TrashBrowser,
    Tags,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagBrowserMode {
    Browse,
    Create,
    Color,
    DeleteConfirm,
}

// ── Tree ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TreeItem {
    Folder {
        name: String,
        expanded: bool,
        note_count: usize,
    },
    Note(NoteSummary),
}

impl TreeItem {
    pub fn is_note(&self) -> bool {
        matches!(self, TreeItem::Note(_))
    }
    pub fn note(&self) -> Option<&NoteSummary> {
        match self {
            TreeItem::Note(n) => Some(n),
            _ => None,
        }
    }
    pub fn folder_name(&self) -> Option<&str> {
        match self {
            TreeItem::Folder { name, .. } => Some(name),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeInlineMode {
    None,
    CreateFolder,
    RenameFolder(String),
    RenameNote(i64),
}

// ── Theme ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    NeoNoir,
    Paper,
    Matrix,
}

impl ThemeName {
    pub fn next(self) -> Self {
        match self {
            Self::NeoNoir => Self::Paper,
            Self::Paper => Self::Matrix,
            Self::Matrix => Self::NeoNoir,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NeoNoir => "neo-noir",
            Self::Paper => "paper",
            Self::Matrix => "matrix",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "neo-noir" | "neonoir" | "neo" => Some(Self::NeoNoir),
            "paper" => Some(Self::Paper),
            "matrix" => Some(Self::Matrix),
            _ => None,
        }
    }

    pub fn palette(self) -> Palette {
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

    pub fn tag_color_choices(self) -> &'static [TagColorChoice] {
        &TAG_COLOR_CHOICES
    }
}

// ── Keymap ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapPreset {
    Default,
    Vim,
}

impl KeymapPreset {
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Vim,
            Self::Vim => Self::Default,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Vim => "vim",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "vim" => Some(Self::Vim),
            _ => None,
        }
    }
}

// ── Density ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Cozy,
    Compact,
}

impl Density {
    pub fn toggle(self) -> Self {
        match self {
            Self::Cozy => Self::Compact,
            Self::Compact => Self::Cozy,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cozy => "cozy",
            Self::Compact => "compact",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "cozy" => Some(Self::Cozy),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }
}

// ── Sort ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Manual,
    Updated,
    Title,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            Self::Manual => Self::Updated,
            Self::Updated => Self::Title,
            Self::Title => Self::Manual,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Updated => "updated",
            Self::Title => "title",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "updated" | "recent" => Some(Self::Updated),
            "title" | "alpha" | "alphabetical" => Some(Self::Title),
            _ => None,
        }
    }
}

// ── Palette ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub panel: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub danger: Color,
    pub ok: Color,
}

// ── Tag Colors ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct TagColorChoice {
    pub key: &'static str,
    pub label: &'static str,
    pub neo: (Color, Color),
    pub paper: (Color, Color),
    pub matrix: (Color, Color),
}

impl TagColorChoice {
    pub fn colors(self, theme: ThemeName) -> (Color, Color) {
        match theme {
            ThemeName::NeoNoir => self.neo,
            ThemeName::Paper => self.paper,
            ThemeName::Matrix => self.matrix,
        }
    }
}

pub const TAG_COLOR_CHOICES: [TagColorChoice; 8] = [
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

// ── Command Palette ────────────────────────────────────────────────────────────

pub struct CommandPaletteEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub command: &'static str,
}

pub fn command_palette_entries() -> &'static [CommandPaletteEntry] {
    &[
        CommandPaletteEntry { name: "New Note",          description: "Create a new note",                    command: "new" },
        CommandPaletteEntry { name: "Create Folder",     description: "Create a new folder",                  command: "create-folder" },
        CommandPaletteEntry { name: "Save",              description: "Save current note",                    command: "w" },
        CommandPaletteEntry { name: "Save & Quit",       description: "Save and exit",                        command: "wq" },
        CommandPaletteEntry { name: "Quit",              description: "Exit anote",                           command: "q" },
        CommandPaletteEntry { name: "Edit Note",         description: "Enter edit mode",                      command: "edit" },
        CommandPaletteEntry { name: "Discard Changes",   description: "Discard unsaved edits",                command: "discard" },
        CommandPaletteEntry { name: "Sticky",            description: "Sticky the active note",                command: "pin" },
        CommandPaletteEntry { name: "Unsticky",          description: "Unsticky the active note",              command: "unpin" },
        CommandPaletteEntry { name: "Archive Note",      description: "Archive the active note",              command: "archive!" },
        CommandPaletteEntry { name: "Unarchive Note",    description: "Unarchive the active note",            command: "unarchive" },
        CommandPaletteEntry { name: "Delete Note",       description: "Delete the active note",               command: "delete" },
        CommandPaletteEntry { name: "Remove from Folder", description: "Remove note from its folder",         command: "unfolder" },
        CommandPaletteEntry { name: "Toggle Theme",      description: "Cycle theme (neo-noir / paper / matrix)", command: "toggle-theme" },
        CommandPaletteEntry { name: "Toggle Keymap",     description: "Switch keymap (default / vim)",        command: "toggle-keymap" },
        CommandPaletteEntry { name: "Change Sort",       description: "Cycle sort mode",                      command: "toggle-sort" },
        CommandPaletteEntry { name: "Daily Note",        description: "Open today's daily note",              command: "daily" },
        CommandPaletteEntry { name: "Tag Browser",       description: "Open tag browser",                     command: "tags" },
        CommandPaletteEntry { name: "Archive Browser",   description: "Open archive browser",                 command: "archived" },
        CommandPaletteEntry { name: "Trash Browser",     description: "Open trash browser",                   command: "trash" },
        CommandPaletteEntry { name: "Empty Trash",       description: "Permanently delete all trashed notes", command: "empty-trash" },
        CommandPaletteEntry { name: "Help",              description: "Open help screen",                     command: "help" },
    ]
}
