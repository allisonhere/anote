# anote

Keyboard-first TUI note-taking app in Rust.

## Current features

- SQLite-backed notes
- Full-text search with FTS5
- TUI list + editor with vim/default presets
- Command palette (`:`) with app actions
- Theme/keymap/density persisted to config
- Quick CLI capture
- CLI search

## Usage

Build:

```bash
cargo build
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

Default behavior:

- Uses local data directory when writable (`~/.local/share/anote` on Linux)
- Falls back to `./.anote/` when that location is unavailable

Override location:

```bash
ANOTE_DATA_DIR=/path/to/data cargo run
```

## Config file

Default behavior:

- Uses config directory when writable (`~/.config/anote/config.toml` on Linux)
- Falls back to `./.anote/config.toml` if needed

Override location:

```bash
ANOTE_CONFIG_PATH=/path/to/config.toml cargo run
```

## TUI keybindings

- `q`: quit
- `j`/`k` or arrows: move selection
- `n`: new note
- `e` or `Enter`: edit selected note
- `Arrow keys`: move cursor while editing
- `Home`/`End`: jump to line start/end
- `Backspace`/`Delete`: delete before/at cursor
- `Ctrl+s`: save while editing
- `Esc`: exit edit/search mode
- `/`: search mode
- `r`: reload notes
- `:`: command palette
- `F6`: cycle theme (`neo-noir`, `paper`, `matrix`)
- `F7`: cycle keymap (`default`, `vim`)
- `F8`: toggle density (`cozy`, `compact`)

### Command palette

- `:new`
- `:edit`
- `:search <query>`
- `:theme neo-noir|paper|matrix`
- `:keymap default|vim`
- `:density cozy|compact|toggle`
- `:reload`
- `:help`
- `:quit`

### Vim preset extras

- Edit opens in vim normal mode
- `l`: open the selected note from the notes pane
- `h`: return to the notes pane from vim normal mode at column 0
- `i`, `a`, `I`, `A`: enter insert mode variants
- `h`, `j`, `k`, `l`: move cursor in vim normal mode
- `0`, `$`: line start/end
- `o`, `O`: open line below/above
- `x`: delete at cursor
- `Esc`: insert->normal, or normal->app normal mode

## Next targets

- Tags and backlinks
- Daily notes and templates
- Sync engine
