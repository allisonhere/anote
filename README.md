# anote

Keyboard-first TUI note-taking app in Rust.

## Features

- SQLite-backed notes with FTS5 full-text search
- Markdown preview with syntax highlighting in code blocks
- Inline spell/grammar linting (harper-core) with one-key fixes
- In-note find with live match highlighting and two-phase navigation
- Folders, tags, pin, archive
- Vim and default keymaps
- Three themes: neo-noir, paper, matrix
- Collapsible notes pane with scrollable preview
- Command palette (`:`), quick CLI capture, CLI search

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/allisonhere/anote/master/install.sh | bash
```

Installs to `~/.local/bin` (or `/usr/local/bin`). Override the location:

```bash
ANOTE_INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/allisonhere/anote/master/install.sh | bash
```

Supported platforms: Linux and macOS, x86_64 and aarch64.

## Build from source

```bash
cargo build --release
```

Run TUI:

```bash
cargo run
# or
cargo run -- tui
```

Quick capture:

```bash
cargo run -- capture "Ship the parser"
cargo run -- capture -t "Idea" "Use WAL + background indexing"
```

Search:

```bash
cargo run -- search "Ship"
```

## Data directory

- Linux default: `~/.local/share/anote/`
- Falls back to `./.anote/`

Override:

```bash
ANOTE_DATA_DIR=/path/to/data cargo run
```

## Config file

- Linux default: `~/.config/anote/config.toml`
- Falls back to `./.anote/config.toml`

Override:

```bash
ANOTE_CONFIG_PATH=/path/to/config.toml cargo run
```

## TUI keybindings

### Normal mode

| Key | Action |
|-----|--------|
| `j` / `k` or `↑` / `↓` | navigate notes |
| `n` | new note |
| `e` or `Enter` | open note in editor |
| `d d` | delete note |
| `/` | search / filter notes |
| `:` | command palette |
| `\` | toggle notes pane |
| `r` | reload notes |
| `?` | help overlay |
| `q` | quit |
| `F6` | cycle theme |
| `F7` | cycle keymap |

### Collapsed pane (preview only)

| Key | Action |
|-----|--------|
| `j` / `k` or `↑` / `↓` | scroll preview one line |
| `PgDn` / `PgUp` | scroll preview fast |

### Edit mode

| Key | Action |
|-----|--------|
| `Esc` | exit to preview |
| `Ctrl+S` | save |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo |
| `Ctrl+C` / `Ctrl+X` | copy / cut |
| `Ctrl+V` | paste from clipboard |
| `Ctrl+F` | find in note |
| `Ctrl+L` | run spell/grammar lint |
| `Tab` | apply first lint suggestion at cursor |
| `]` / `[` | jump to next / prev lint |

### Find mode (Ctrl+F, default keymap)

Type freely to build the query — all characters including `n`/`N` go into the search term. Matches highlight live.

| Key | Action |
|-----|--------|
| `↓` / `↑` | next / prev match while typing |
| `Enter` or `Tab` | commit query → navigation phase |
| `Esc` | cancel find |

Navigation phase (after Enter):

| Key | Action |
|-----|--------|
| `n` / `↓` | next match |
| `N` / `↑` | prev match |
| `Enter` | enter edit mode at current match |
| `Backspace` | return to typing phase |
| any char | restart typing with that character |
| `Esc` | close find |

### Search (/ to enter)

| Token | Effect |
|-------|--------|
| `#tag` | filter by tag |
| `/folder` | filter by folder |
| `:archived` | show archived notes |
| plain text | full-text search |

### Vim keymap extras

| Key | Action |
|-----|--------|
| `h` `j` `k` `l` | move cursor |
| `i` / `a` | enter insert mode |
| `v` | visual select |
| `y` / `d` | yank / delete |
| `p` / `P` | paste from system clipboard |
| `u` / `Ctrl+R` | undo / redo |
| `l` (normal mode) | open selected note from notes pane |

## Command palette

| Command | Description |
|---------|-------------|
| `:new` | create a new note |
| `:edit` | open note in editor |
| `:folder <name>` | move note to folder (blank = remove) |
| `:pin` / `:unpin` | pin note to top of list |
| `:archive` | hide note from main list |
| `:unarchive` | restore an archived note |
| `:search <query>` | run a search programmatically |
| `:theme <name>` | `neo-noir` \| `paper` \| `matrix` |
| `:keymap <name>` | `default` \| `vim` |
| `:reload` | refresh note list |
| `:w` | save |
| `:q` / `:quit` | quit |

## Tags

Write `#tagname` anywhere in a note body — tags are extracted automatically and searchable with `#tag` in the search bar.
