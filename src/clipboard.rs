use edtui::clipboard::ClipboardTrait;

/// A clipboard provider that writes to the system clipboard via shell tools.
/// Uses wl-copy/xclip on Linux and pbcopy/pbpaste on macOS.
/// arboard is used as fallback on Linux.
pub struct SystemClipboard;

impl ClipboardTrait for SystemClipboard {
    fn set_text(&mut self, text: String) {
        // macOS: pbcopy
        #[cfg(target_os = "macos")]
        {
            if let Ok(mut child) = std::process::Command::new("pbcopy")
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
        }

        // Linux: shell tools
        #[cfg(not(target_os = "macos"))]
        {
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
    }

    fn get_text(&mut self) -> String {
        // macOS: pbpaste
        #[cfg(target_os = "macos")]
        {
            if let Ok(out) = std::process::Command::new("pbpaste").output()
                && out.status.success()
                && let Ok(s) = String::from_utf8(out.stdout)
            {
                return s;
            }
        }

        // Linux: arboard first, then shell tools
        #[cfg(not(target_os = "macos"))]
        {
            // Try arboard first for reads (reading is synchronous unlike writing)
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Ok(text) = cb.get_text() {
                    return text;
                }
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
