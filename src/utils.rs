// ── Footer helpers ─────────────────────────────────────────────────────────────

pub fn fit_footer_left(text: &str, width: usize) -> String {
    truncate_with_ellipsis(text, width)
}

pub fn fit_footer_segments(left: &str, hints: &[&str], width: usize) -> String {
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
        return "\u{2026}".to_string();
    }

    let mut out: String = trimmed.chars().take(width - 1).collect();
    out.push('\u{2026}');
    out
}

// ── Command parsing ────────────────────────────────────────────────────────────

/// Tokenize a command string respecting single/double quotes and backslash escapes.
pub fn parse_command_parts(command: &str) -> Vec<String> {
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

// ── Time ────────────────────────────────────────────────────────────────────────

pub fn short_timestamp(ts: &str) -> String {
    ts.get(0..16).unwrap_or(ts).to_string()
}
