# System Clipboard Integration for Vim Yank/Delete

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Make vim-mode yank (`yy`, `yw`, etc.) and delete (`dd`, `dw`, etc.) operations write to the system clipboard, so text yanked in anote can be pasted elsewhere and vice versa.

**Architecture:** edtui 0.11.2 has a `ClipboardTrait` abstraction — `EditorState::set_clipboard()` accepts any `ClipboardTrait` impl. Currently anote never calls `set_clipboard()`, so edtui defaults to its `InternalClipboard` (in-process only). We'll implement `ClipboardTrait` for a `SystemClipboard` struct that wraps `arboard::Clipboard` with shell fallbacks, then inject it into every `EditorState`. This makes all vim yank/delete ops automatically write to the system clipboard, with zero changes to keybinding logic.

**Tech Stack:** Rust, edtui 0.11.2, arboard 3.x

---

## Current State

### What already works:
- **Default keymap Ctrl+C**: calls `copy_selection()` → saves to `yank_buffer` AND `clipboard_set()` (system clipboard) ✓
- **Default keymap Ctrl+X**: calls `copy_selection()` → then `delete_selection()` (cut) ✓
- **Default keymap Ctrl+V**: calls `clipboard_get()` (system clipboard preferred), falls back to `yank_buffer` ✓
- **Vim visual select yank**: uses `copy_selection()` → system clipboard ✓ (line 3286)

### What DOESN'T work:
- **Vim yy (YankLine)**: edtui's internal `InternalClipboard` only — never reaches system clipboard ✗
- **Vim yw (YankWord)**: same ✗
- **Vim dd (DeleteLine)**: same ✗
- **Vim dw (DeleteWord)**: same ✗
- **Vim cw/cc (Change)**: deleted text goes to edtui internal only ✗

### Current clipboard plumbing:
- `App.clipboard: Option<arboard::Clipboard>` — created once at startup, used by `clipboard_set()`/`clipboard_get()`
- `clipboard_set()`: tries arboard first, then `wl-copy` and `xclip` shell fallbacks
- `clipboard_get()`: tries arboard first, then `wl-paste`, `xclip`, `xsel` shell fallbacks
- `App.yank_buffer: String` — internal buffer, used as fallback when system clipboard is empty

### Key insight: edtui's `ClipboardTrait`
From edtui docs (`target/doc/edtui/clipboard/`):

```rust
pub trait ClipboardTrait {
    fn set_text(&mut self, text: String);
    fn get_text(&mut self) -> String;
}
```

`EditorState::set_clipboard(clipboard: impl ClipboardTrait)` — injects a clipboard provider. All yank/delete/paste operations in vim mode delegate to this. Currently never called in anote, so edtui defaults to `InternalClipboard` (a `String` buffer).

---

## Step-by-step Plan

### Task 1: Add `SystemClipboard` struct implementing `ClipboardTrait`

**Objective:** Create a `SystemClipboard` type that edtui can use for all vim-mode clipboard operations.

**Files:**
- Create: `src/clipboard.rs`
- Modify: `src/main.rs` (add `mod clipboard;`)

**Step 1: Create the module**

Create `src/clipboard.rs` with:

```rust
use edtui::clipboard::ClipboardTrait;

/// A clipboard provider that writes to the system clipboard via arboard
/// with shell-command fallbacks for headless/Wayland/X11 environments.
pub struct SystemClipboard;

impl ClipboardTrait for SystemClipboard {
    fn set_text(&mut self, text: String) {
        // Try arboard (primary path for X11/Wayland with display server)
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if cb.set_text(&text).is_ok() {
                return;
            }
        }
        // Fallback: wl-copy (wlroots-based Wayland compositors)
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
        // Fallback: xclip (X11)
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-sel", "clip"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }

    fn get_text(&mut self) -> String {
        // Try arboard
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                return text;
            }
        }
        // Fallback: wl-paste
        if let Ok(out) = std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    return s;
                }
            }
        }
        // Fallback: xclip
        if let Ok(out) = std::process::Command::new("xclip")
            .args(["-sel", "clip", "-o"])
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    return s;
                }
            }
        }
        // Fallback: xsel
        if let Ok(out) = std::process::Command::new("xsel")
            .arg("--clipboard")
            .arg("--output")
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    return s;
                }
            }
        }
        String::new()
    }
}
```

**Step 2: Register the module**

In `src/main.rs`, add `mod clipboard;` after the existing module declarations:
```rust
mod clipboard;
mod config;
// ...
```

**Step 3: Build check**

```bash
cd /home/allie/Projects/anote && cargo check 2>&1
```

Expected: compiles (unused import warnings for now).

---

### Task 2: Inject `SystemClipboard` into edtui's `EditorState`

**Objective:** Every time we create or recreate the `EditorState` for vim mode, inject the system clipboard so edtui uses it.

**Files:**
- Modify: `src/tui.rs`

**Changes:**

#### 2a: Add import
Add at the top of `tui.rs` near the edtui imports:
```rust
use edtui::clipboard::Clipboard as EdtuiClipboard;
use crate::clipboard::SystemClipboard;
```

#### 2b: Inject clipboard after `sync_state_from_editor_buffer`

Replace the `sync_state_from_editor_buffer` method (line ~456-469) to inject clipboard after creating state:

```rust
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
    state.set_clipboard(EdtuiClipboard::new(SystemClipboard));
    self.editor_state = state;
}
```

#### 2c: Inject clipboard in `App::new()` for initial state

In `App::new()` (line ~147), after creating `editor_state`, add clipboard injection. Change:
```rust
editor_state: EdtuiState::new(Lines::from("")),
```
to:
```rust
editor_state: {
    let mut state = EdtuiState::new(Lines::from(""));
    state.set_clipboard(EdtuiClipboard::new(SystemClipboard));
    state
},
```

**Step 4: Build check**

```bash
cargo check 2>&1
```

Expected: compiles with no errors.

---

### Task 3: Remove redundant vim paste interception

**Objective:** With system clipboard injected into edtui, the custom `p`/`P` interception is no longer needed — edtui will read from our `SystemClipboard` automatically. Remove the interception and let edtui handle paste natively.

**Files:**
- Modify: `src/tui.rs` — `handle_edit_key()` for vim keymap

**Change:** Remove lines 1589–1608 (the `p`/`P` system clipboard interception block).

The vim key handler currently has:
```rust
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
```

Replace with the simple delegation:
```rust
        let before = self.editor_state.lines.to_string();
        self.editor_events.on_key_event(key, &mut self.editor_state);
        self.sync_after_editor_event(before);
```

This also removes the need for `InsertChar` in the vim key handler (it was only used for the paste interception). Check if `InsertChar` is used elsewhere in the file — if not, remove the unused import.

**Step 5: Build check**

```bash
cargo check 2>&1
```

Expected: compiles. Fix any unused import warnings.

---

### Task 4: Verify `sync_state_from_editor_buffer` call sites all inject clipboard

**Objective:** Audit all call sites of `sync_state_from_editor_buffer` to ensure clipboard is re-injected after every state recreation.

**Files:**
- Inspect: `src/tui.rs`

**Current call sites:**

1. `apply_editor_keymap()` (line ~451) — calls `sync_state_from_editor_buffer()` when switching to vim keymap. ✓ (already handles via Task 2b)
2. `delete_selection()` (line ~3268) — calls `sync_state_from_editor_buffer()` after replacing editor_buffer. Only used in default keymap mode (the `App.editor_state` isn't the active editor in default mode — `EditorBuffer` is the source of truth). So this is irrelevant for clipboard injection.

No additional changes needed.

---

### Task 5: Remove now-unused `App` clipboard fields

**Objective:** Clean up dead code. With `SystemClipboard` handling all vim clipboard operations, the `App.clipboard` field and `clipboard_set()`/`clipboard_get()` methods still serve the default keymap (Ctrl+C/V/X). Keep them but simplify.

**Files:**
- Modify: `src/tui.rs`

**Decision:** Keep `App.clipboard`, `clipboard_set()`, and `clipboard_get()` for now — they're still needed for:
- Default keymap Ctrl+C/V/X (which doesn't go through edtui)
- `copy_selection()` at line 3286

These can be consolidated in a follow-up but aren't part of this change.

---

### Task 6: Update README

**Objective:** Update the keybinding docs to reflect that vim yank/delete now uses system clipboard.

**Files:**
- Modify: `README.md`

**Change:** Update line 225:
```
| `y` / `d` | yank / delete |
```
to:
```
| `y` / `d` | yank / delete (to system clipboard) |
```

---

### Task 7: Build and verify

**Objective:** Full build and basic smoke test.

**Step: Build release**

```bash
cd /home/allie/Projects/anote && cargo build --release 2>&1
```

Expected: compiles with zero warnings.

**Step: Run basic smoke test**

```bash
cargo build 2>&1 && echo "BUILD OK"
```

Expected: `BUILD OK`

---

## Files Changed Summary

| File | Action |
|------|--------|
| `src/clipboard.rs` | **Create** — `SystemClipboard` struct with `ClipboardTrait` impl |
| `src/main.rs` | Modify — add `mod clipboard;` |
| `src/tui.rs` | Modify — inject clipboard into `EditorState`, remove p/P interception |
| `README.md` | Modify — update vim keybinding docs |

---

## Risks and Tradeoffs

1. **Performance**: `SystemClipboard` creates a new `arboard::Clipboard` on every call. This is lightweight (just connects to the display server's clipboard protocol). Acceptable for interactive use.

2. **Wayland without wl-copy**: If arboard fails AND wl-copy isn't installed, clipboard is silently no-op. Users on exotic compositors without wlroots protocols need to install wl-clipboard. Acceptable tradeoff — same behavior as current fallbacks.

3. **Text truncation on shell fallback**: `wl-paste --no-newline` strips trailing newlines. This can lose intentional trailing newlines in yanked text. Pre-existing issue (current `clipboard_get` has the same behavior). Not addressed in this plan.

4. **Thread safety**: `ClipboardTrait::set_text/get_text` take `&mut self`. edtui's event loop is single-threaded, and our clipboard is created inline (not shared). No race conditions.

5. **Nerd Font requirement**: No change — tag pills already require Nerd Font.

---

## Verification

```bash
# Build
cd /home/allie/Projects/anote && cargo build --release

# Run and manually test:
# 1. Open anote TUI, switch to vim keymap (F7)
# 2. Enter edit mode on a note, yank a line (yy)
# 3. Close anote, paste into another app — text should appear
# 4. Copy text from external app, open anote, paste with p in vim normal mode
```
