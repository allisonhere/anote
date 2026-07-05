use edtui::clipboard::ClipboardTrait;

/// A clipboard provider that writes to the system clipboard via shell tools
/// (wl-copy/xclip) with arboard as fallback. Shell tools are tried first
/// because arboard on Wayland can return Ok without actually committing
/// to the compositor.
pub struct SystemClipboard;

impl ClipboardTrait for SystemClipboard {
    fn set_text(&mut self, text: String) {
        // Primary: shell tools (reliable on both Wayland and X11)
        let mut ok = false;
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            ok = child.wait().is_ok_and(|s| s.success());
        }
        if !ok
            && let Ok(mut child) = std::process::Command::new("xclip")
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
        // Fallback: arboard (reliable on X11, may not commit on Wayland)
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(&text);
        }
    }

    fn get_text(&mut self) -> String {
        // Try arboard first for reads (reading is synchronous unlike writing)
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                return text;
            }
        }
        // Fallback: wl-paste
        if let Ok(out) = std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
            && out.status.success()
            && let Ok(s) = String::from_utf8(out.stdout)
        {
            return s;
        }
        // Fallback: xclip
        if let Ok(out) = std::process::Command::new("xclip")
            .args(["-sel", "clip", "-o"])
            .output()
            && out.status.success()
            && let Ok(s) = String::from_utf8(out.stdout)
        {
            return s;
        }
        // Fallback: xsel
        if let Ok(out) = std::process::Command::new("xsel")
            .arg("--clipboard")
            .arg("--output")
            .output()
            && out.status.success()
            && let Ok(s) = String::from_utf8(out.stdout)
        {
            return s;
        }
        String::new()
    }
}
