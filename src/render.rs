use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TSpan, Text};

use pulldown_cmark::{
    CodeBlockKind, Event as MdEvent, HeadingLevel, Options as MdOptions, Parser as MdParser,
    Tag as MdTag, TagEnd as MdTagEnd,
};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use super::types::{Palette, ThemeName, TAG_COLOR_CHOICES};

// ── Tag color computation ──────────────────────────────────────────────────────

pub fn tag_color_idx(tag: &str, len: usize) -> usize {
    tag.bytes()
        .fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize))
        % len
}

pub fn tag_color_choice_index(key: Option<&str>) -> usize {
    match key {
        None | Some("") => 0,
        Some(key) => TAG_COLOR_CHOICES
            .iter()
            .position(|choice| choice.key == key)
            .map(|idx| idx + 1)
            .unwrap_or(0),
    }
}

pub fn resolve_tag_colors(theme: ThemeName, tag: &str, color_key: Option<&str>) -> (Color, Color) {
    if let Some(key) = color_key
        && let Some(choice) = theme.tag_color_choices().iter().find(|choice| choice.key == key)
    {
        return choice.colors(theme);
    }

    let choices = theme.tag_color_choices();
    choices[tag_color_idx(tag, choices.len())].colors(theme)
}

pub fn tag_dot_style(theme: ThemeName, tag: &str, color_key: Option<&str>) -> Style {
    let (bg, _) = resolve_tag_colors(theme, tag, color_key);
    Style::default().fg(bg)
}

/// Returns spans for a rounded pill using Nerd Font powerline glyphs (requires Nerd Font).
/// `row_bg` should be the background color of the containing row so the caps blend in.
pub fn tag_pill_spans(
    theme: ThemeName,
    tag: &str,
    color_key: Option<&str>,
    row_bg: Color,
) -> Vec<TSpan<'static>> {
    let (bg, fg) = resolve_tag_colors(theme, tag, color_key);
    let cap = Style::default().fg(bg).bg(row_bg);
    let body = Style::default().bg(bg).fg(fg);
    vec![
        TSpan::styled("\u{E0B6}", cap),
        TSpan::styled(format!("#{} ", tag), body),
        TSpan::styled("\u{E0B4}", cap),
    ]
}

pub fn color_choice_entry_spans(
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
                    if selected { "\u{203A} " } else { "  " },
                    if selected {
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.bg)
                    },
                ),
                TSpan::styled(
                    "auto",
                    if selected {
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD)
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
                if selected { "\u{203A} " } else { "  " },
                if selected {
                    Style::default()
                        .fg(choice.colors(theme).0)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.bg)
                },
            )];
            spans.extend(tag_pill_spans(theme, choice.key, Some(choice.key), palette.bg));
            spans.push(TSpan::styled(
                format!(" {}", choice.label),
                if selected {
                    Style::default()
                        .fg(choice.colors(theme).0)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.text)
                },
            ));
            spans
        }
    }
}

// ── Tag body manipulation ──────────────────────────────────────────────────────

fn is_tag_boundary(c: char) -> bool {
    !c.is_ascii_alphanumeric() && c != '_' && c != '-'
}

/// Returns true if the first line of `body` contains `#tag` as a whole tag token.
pub fn body_has_tag(body: &str, tag: &str) -> bool {
    let first_line = body.lines().next().unwrap_or("").to_ascii_lowercase();
    let needle = format!("#{}", tag);
    let mut pos = 0;
    while pos < first_line.len() {
        if let Some(found) = first_line[pos..].find(&needle) {
            let abs = pos + found;
            let after = abs + needle.len();
            let next_is_continuation = first_line[after..]
                .chars()
                .next()
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
pub fn append_tag_to_body(body: &str, tag: &str) -> String {
    let token = format!(" #{}", tag);
    match body.find('\n') {
        Some(nl) => format!("{}{}{}", &body[..nl], token, &body[nl..]),
        None => format!("{}{}", body, token),
    }
}

/// Removes all whole-token occurrences of `#tag` from the first line of `body`.
pub fn remove_tag_from_body(body: &str, tag: &str) -> String {
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
            let next_is_continuation = line[after..]
                .chars()
                .next()
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

pub fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|&(s, _)| s);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = last.1.max(e);
            continue;
        }
        merged.push((s, e));
    }
    merged
}

// ── Markdown preview highlighting ──────────────────────────────────────────────

pub fn markdown_highlight_line(line: &str, palette: Palette) -> Vec<(usize, usize, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if len == 0 {
        return vec![];
    }

    let muted_style = Style::default().fg(palette.muted);
    let accent_style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let ok_style = Style::default().fg(palette.ok);

    // Headings: starts with one or more '#' followed by a space
    let heading_level: usize = {
        let mut level = 0;
        for &c in &chars {
            if c == '#' {
                level += 1;
            } else {
                break;
            }
        }
        level
    };
    if heading_level > 0 && heading_level < len && chars[heading_level] == ' ' {
        let mut ranges = vec![
            (0, heading_level + 1, muted_style), // "## " prefix
            (heading_level + 1, len, accent_style), // heading text
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
            if (first == '-' || first == '*' || first == '_')
                && trimmed.iter().all(|&c| c == first)
            {
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
            let start = i;
            i += 1;
            let content_start = i;
            while i < len && chars[i] != '`' {
                i += 1;
            }
            if i < len {
                let content_end = i;
                i += 1; // consume closing backtick
                ranges.push((start, start + 1, muted_style));
                if content_start < content_end {
                    ranges.push((content_start, content_end, ok_style));
                }
                ranges.push((content_end, content_end + 1, muted_style));
            }
        } else if chars[i] == '*' {
            if i + 1 < len && chars[i + 1] == '*' {
                let start = i;
                i += 2;
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
                    i += 2;
                    ranges.push((start, start + 2, muted_style));
                    if content_start < content_end {
                        ranges.push((
                            content_start,
                            content_end,
                            Style::default().add_modifier(Modifier::BOLD),
                        ));
                    }
                    ranges.push((content_end, content_end + 2, muted_style));
                }
            } else {
                let start = i;
                i += 1;
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
                    i += 1;
                    ranges.push((start, start + 1, muted_style));
                    if content_start < content_end {
                        ranges.push((
                            content_start,
                            content_end,
                            Style::default().add_modifier(Modifier::ITALIC),
                        ));
                    }
                    ranges.push((content_end, content_end + 1, muted_style));
                }
            }
        } else {
            i += 1;
        }
    }

    ranges
}

// ── Span builder for editor rows ───────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn build_spans_for_row(
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

    let mut spans: Vec<TSpan<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_cat: u8 = 0;
    let mut current_syn_style: Style = normal;

    for (i, &c) in visible_chars.iter().enumerate() {
        let abs_col = col_offset + i;
        let in_sel = sel_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_find_active = find_active_ranges
            .iter()
            .any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_find = find_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let in_lint = lint_ranges.iter().any(|&(s, e)| abs_col >= s && abs_col < e);
        let cat: u8 = if in_sel {
            4
        } else if in_find_active {
            3
        } else if in_find {
            2
        } else if in_lint {
            1
        } else {
            0
        };

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

// ── Code fence fixer ───────────────────────────────────────────────────────────

/// CommonMark disallows backticks in a backtick-fence info string.
/// When the user writes ```lang``` (open+close on one line), strip the
/// trailing fence so pulldown-cmark sees a valid opening fence.
pub fn fix_fences(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_end();
            for fence in &["```", "~~~"] {
                if trimmed.starts_with(fence)
                    && trimmed.ends_with(fence)
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

// ── Markdown preview renderer ──────────────────────────────────────────────────

pub fn render_markdown_preview(
    text: &str,
    palette: Palette,
    syntax_set: &SyntaxSet,
    theme_set: &ThemeSet,
    highlight_terms: &[String],
) -> Text<'static> {
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
    let preview_theme = theme_set
        .themes
        .get("base16-ocean.dark")
        .or_else(|| theme_set.themes.values().next());

    let mut in_heading: Option<HeadingLevel> = None;
    let mut in_bold = false;
    let mut in_italic = false;
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
                        let syntax = syntax_set
                            .find_syntax_by_token(lang)
                            .or_else(|| {
                                syntax_set
                                    .syntaxes()
                                    .iter()
                                    .find(|s| s.name.to_lowercase() == lower)
                            })
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
                    "\u{2500}".repeat(40),
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
                } else {
                    normal_style
                };

                if in_code_block {
                    let lines_vec: Vec<&str> = s.lines().collect();
                    for (i, line_str) in lines_vec.iter().enumerate() {
                        let indent = "  ".repeat(list_depth.max(1).saturating_sub(1) + 1);
                        if let Some(hl) = code_highlighter.as_mut() {
                            let line_with_newline = format!("{}\n", line_str);
                            if let Ok(tokens) = hl.highlight_line(&line_with_newline, syntax_set) {
                                current_spans.push(TSpan::raw(indent));
                                for (syntect_style, token_str) in &tokens {
                                    let text_part = token_str.trim_end_matches('\n');
                                    if !text_part.is_empty() {
                                        let fg = syntect_style.foreground;
                                        let span_style =
                                            Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
                                        current_spans.push(TSpan::styled(
                                            text_part.to_string(),
                                            span_style,
                                        ));
                                    }
                                }
                            } else {
                                current_spans.push(TSpan::styled(
                                    format!("{}{}", indent, line_str),
                                    code_style,
                                ));
                            }
                        } else {
                            current_spans.push(TSpan::styled(
                                format!("{}{}", indent, line_str),
                                code_style,
                            ));
                        }
                        if i + 1 < lines_vec.len() {
                            flush_line(&mut current_spans, &mut lines);
                        }
                    }
                } else {
                    let prefix = if is_list_item && list_item_first {
                        list_item_first = false;
                        let indent = "  ".repeat(list_depth.saturating_sub(1));
                        format!("{indent}\u{2022} ")
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
                current_spans.push(TSpan::styled("\u{2502} ", rule_style));
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
    while lines.last().is_some_and(|l: &Line<'_>| l.spans.is_empty()) {
        lines.pop();
    }

    if highlight_terms.is_empty() {
        Text::from(lines)
    } else {
        Text::from(highlight_preview_lines(lines, highlight_terms, palette))
    }
}

// ── Preview search highlighting ────────────────────────────────────────────────

pub fn preview_highlight_terms(query: &str) -> Vec<String> {
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

fn highlight_preview_lines(
    lines: Vec<Line<'static>>,
    terms: &[String],
    palette: Palette,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.to_string())
                .collect();
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
                        Style::default()
                            .bg(palette.ok)
                            .fg(palette.bg)
                            .add_modifier(Modifier::BOLD)
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
                    Style::default()
                        .bg(palette.ok)
                        .fg(palette.bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.text)
                };
                spans.push(TSpan::styled(current, style));
            }
            Line::from(spans)
        })
        .collect()
}
