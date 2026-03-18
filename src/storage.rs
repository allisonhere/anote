use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone)]
pub struct NoteSummary {
    pub id: i64,
    pub title: String,
    pub updated_at: String,
    pub folder: String,
    pub tags: String,
    pub snippet: String,
    pub pinned: bool,
    pub archived: bool,
    pub note_order: i64,
}

#[derive(Debug, Clone)]
pub struct FolderEntry {
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone)]
pub struct TagEntry {
    pub tag: String,
    pub count: i64,
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub body: String,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    #[cfg(test)]
    pub fn open_for_test() -> Result<Self> {
        let conn = Connection::open_in_memory().context("failed opening in-memory db")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self> {
        let data_dir = resolve_data_dir()?;
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed creating data dir {}", data_dir.display()))?;

        let db_path = data_dir.join("anote.db");
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed opening db at {}", db_path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
                title,
                body,
                content='notes',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS notes_ai AFTER INSERT ON notes BEGIN
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
            END;

            CREATE TRIGGER IF NOT EXISTS notes_ad AFTER DELETE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.id, old.title, old.body);
            END;

            CREATE TRIGGER IF NOT EXISTS notes_au AFTER UPDATE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.id, old.title, old.body);
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
            END;
            "#,
        )?;

        // Migrations: add folder and tags columns to existing DBs.
        for sql in &[
            "ALTER TABLE notes ADD COLUMN folder TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE notes ADD COLUMN tags   TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE notes ADD COLUMN note_order INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE notes ADD COLUMN title_locked INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE notes ADD COLUMN deleted_at TEXT",
        ] {
            match self.conn.execute_batch(sql) {
                Ok(_) => {}
                Err(e) if e.to_string().contains("duplicate column name") => {}
                Err(e) => return Err(e.into()),
            }
        }

        // Create folders table
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                sort_order INTEGER NOT NULL DEFAULT 0
            );"
        )?;

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tag_meta (
                name TEXT PRIMARY KEY,
                color TEXT NOT NULL DEFAULT ''
            );"
        )?;

        // Seed folders table from existing notes
        self.conn.execute_batch(
            "INSERT OR IGNORE INTO folders (name, sort_order)
             SELECT folder, ROW_NUMBER() OVER (ORDER BY folder) * 10
             FROM notes
             WHERE folder != ''
             GROUP BY folder;"
        )?;

        Ok(())
    }

    pub fn list_notes(&self, query: &str) -> Result<Vec<NoteSummary>> {
        self.list_notes_scoped(query, None, false, false)
    }

    pub fn list_notes_for_switcher(&self, query: &str) -> Result<Vec<NoteSummary>> {
        self.list_notes_scoped(query, None, false, false)
    }

    pub fn list_notes_in_folder(&self, folder: &str, query: &str) -> Result<Vec<NoteSummary>> {
        self.list_notes_scoped(query, Some(folder), false, false)
    }

    pub fn list_notes_scoped(
        &self,
        query: &str,
        folder_scope: Option<&str>,
        show_archived: bool,
        show_trash: bool,
    ) -> Result<Vec<NoteSummary>> {
        self.list_notes_internal(query, folder_scope, show_archived, show_trash)
    }

    pub fn list_tags(&self) -> Result<Vec<TagEntry>> {
        let mut stmt = self.conn.prepare(
            "WITH tag_counts AS (
                SELECT tag, COUNT(*) AS count FROM (
                    SELECT TRIM(value) AS tag
                    FROM notes, json_each('[\"' || REPLACE(tags, ' ', '\",\"') || '\"]')
                    WHERE tags != '' AND deleted_at IS NULL AND archived = 0
                )
                WHERE tag != ''
                GROUP BY tag
            ),
            all_tags AS (
                SELECT tag FROM tag_counts
                UNION
                SELECT name AS tag FROM tag_meta
            )
            SELECT all_tags.tag,
                   COALESCE(tag_counts.count, 0) AS count,
                   NULLIF(TRIM(tag_meta.color), '') AS color
            FROM all_tags
            LEFT JOIN tag_counts ON tag_counts.tag = all_tags.tag
            LEFT JOIN tag_meta ON tag_meta.name = all_tags.tag
            ORDER BY
                CASE WHEN COALESCE(tag_counts.count, 0) = 0 THEN 1 ELSE 0 END,
                COALESCE(tag_counts.count, 0) DESC,
                all_tags.tag ASC"
        )?;
        let tags = stmt.query_map([], |row| {
            Ok(TagEntry { tag: row.get(0)?, count: row.get(1)?, color: row.get(2)? })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(tags)
    }

    pub fn list_tag_colors(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, color FROM tag_meta WHERE TRIM(color) != ''"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut colors = HashMap::new();
        for row in rows {
            let (tag, color) = row?;
            colors.insert(tag, color);
        }
        Ok(colors)
    }

    pub fn list_folders(&self) -> Result<Vec<FolderEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, sort_order FROM folders ORDER BY sort_order, name"
        )?;
        let folders = stmt.query_map([], |row| {
            Ok(FolderEntry { name: row.get(0)?, sort_order: row.get(1)? })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(folders)
    }

    pub fn create_folder(&self, name: &str) -> Result<i64> {
        let max_order: i64 = self.conn
            .query_row("SELECT COALESCE(MAX(sort_order), 0) FROM folders", [], |r| r.get(0))
            .unwrap_or(0);
        self.conn.execute(
            "INSERT OR IGNORE INTO folders (name, sort_order) VALUES (?1, ?2)",
            params![name.trim(), max_order + 10],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_folder(&self, old_name: &str, new_name: &str) -> Result<()> {
        self.conn.execute("UPDATE folders SET name = ?1 WHERE name = ?2", params![new_name.trim(), old_name])?;
        self.conn.execute("UPDATE notes SET folder = ?1 WHERE folder = ?2", params![new_name.trim(), old_name])?;
        Ok(())
    }

    pub fn delete_folder(&self, name: &str) -> Result<()> {
        self.conn.execute("UPDATE notes SET folder = '' WHERE folder = ?1", params![name])?;
        self.conn.execute("DELETE FROM folders WHERE name = ?1", params![name])?;
        Ok(())
    }

    pub fn swap_folder_order(&self, name_a: &str, name_b: &str) -> Result<()> {
        let order_a: i64 = self.conn.query_row(
            "SELECT sort_order FROM folders WHERE name = ?1", params![name_a], |r| r.get(0)
        )?;
        let order_b: i64 = self.conn.query_row(
            "SELECT sort_order FROM folders WHERE name = ?1", params![name_b], |r| r.get(0)
        )?;
        self.conn.execute("UPDATE folders SET sort_order = ?1 WHERE name = ?2", params![order_b, name_a])?;
        self.conn.execute("UPDATE folders SET sort_order = ?1 WHERE name = ?2", params![order_a, name_b])?;
        Ok(())
    }

    pub fn swap_note_order(&self, id_a: i64, id_b: i64) -> Result<()> {
        let order_a: i64 = self.conn.query_row(
            "SELECT note_order FROM notes WHERE id = ?1", params![id_a], |r| r.get(0)
        )?;
        let order_b: i64 = self.conn.query_row(
            "SELECT note_order FROM notes WHERE id = ?1", params![id_b], |r| r.get(0)
        )?;
        self.conn.execute("UPDATE notes SET note_order = ?1 WHERE id = ?2", params![order_b, id_a])?;
        self.conn.execute("UPDATE notes SET note_order = ?1 WHERE id = ?2", params![order_a, id_b])?;
        Ok(())
    }

    pub fn set_note_order(&self, id: i64, order: i64) -> Result<()> {
        self.conn.execute("UPDATE notes SET note_order = ?1 WHERE id = ?2", params![order, id])?;
        Ok(())
    }

    pub fn get_note(&self, id: i64) -> Result<Option<Note>> {
        let mut stmt = self.conn.prepare("SELECT body FROM notes WHERE id = ?1")?;
        let note = stmt
            .query_row([id], |row| Ok(Note { body: row.get(0)? }))
            .optional()?;
        Ok(note)
    }

    pub fn create_note(&self, title: &str, body: &str) -> Result<i64> {
        self.create_note_with_title_lock(title, body, false)
    }

    pub fn create_note_with_title_lock(
        &self,
        title: &str,
        body: &str,
        title_locked: bool,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let note_title = if title_locked {
            title.trim().to_string()
        } else if body.trim().is_empty() {
            title.trim().to_string()
        } else {
            derive_title(body)
        };
        let tags = extract_tags(body);
        self.conn.execute(
            "INSERT INTO notes (title, body, created_at, updated_at, tags, title_locked) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![note_title, body, now, now, tags, title_locked as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn capture(&self, title: Option<&str>, body: &str) -> Result<i64> {
        match title {
            Some(t) if !t.trim().is_empty() => self.create_note_with_title_lock(t.trim(), body, true),
            _ => self.create_note("Untitled", body),
        }
    }

    pub fn update_note(&self, id: i64, body: &str) -> Result<()> {
        let title_locked: bool = self.conn.query_row(
            "SELECT title_locked FROM notes WHERE id = ?1",
            params![id],
            |row| Ok(row.get::<_, i64>(0)? != 0),
        )?;
        let title = if title_locked {
            self.conn
                .query_row("SELECT title FROM notes WHERE id = ?1", params![id], |row| row.get(0))?
        } else {
            derive_title(body)
        };
        let now = Utc::now().to_rfc3339();
        let tags = extract_tags(body);
        self.conn.execute(
            "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3, tags = ?4 WHERE id = ?5",
            params![title, body, now, tags, id],
        )?;
        Ok(())
    }

    pub fn update_note_with_title(&self, id: i64, body: &str, title: &str, title_locked: bool) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let tags = extract_tags(body);
        self.conn.execute(
            "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3, tags = ?4, title_locked = ?5 WHERE id = ?6",
            params![title.trim(), body, now, tags, title_locked as i64, id],
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn is_title_locked(&self, id: i64) -> Result<bool> {
        self.conn.query_row(
            "SELECT title_locked FROM notes WHERE id = ?1",
            params![id],
            |row| Ok(row.get::<_, i64>(0)? != 0),
        )
        .map_err(Into::into)
    }

    pub fn set_folder(&self, id: i64, folder: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE notes SET folder = ?1 WHERE id = ?2",
            params![folder.trim(), id],
        )?;
        Ok(())
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE notes SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, id],
        )?;
        Ok(())
    }

    pub fn set_archived(&self, id: i64, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE notes SET archived = ?1 WHERE id = ?2",
            params![archived as i64, id],
        )?;
        Ok(())
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute("UPDATE notes SET deleted_at = ?1 WHERE id = ?2", params![now, id])?;
        Ok(())
    }

    pub fn restore_note(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE notes SET deleted_at = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn purge_note(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn purge_deleted_notes(&self) -> Result<usize> {
        let deleted = self.conn.execute("DELETE FROM notes WHERE deleted_at IS NOT NULL", [])?;
        Ok(deleted)
    }

    pub fn create_tag(&self, tag: &str) -> Result<String> {
        let normalized = normalize_tag_name(tag)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO tag_meta (name, color) VALUES (?1, '')",
            params![normalized],
        )?;
        Ok(normalized)
    }

    pub fn set_tag_color(&self, tag: &str, color: Option<&str>) -> Result<String> {
        let normalized = normalize_tag_name(tag)?;
        let color = color.unwrap_or("").trim();
        self.conn.execute(
            "INSERT INTO tag_meta (name, color) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET color = excluded.color",
            params![normalized, color],
        )?;
        Ok(normalized)
    }

    pub fn delete_tag_everywhere(&self, tag: &str) -> Result<usize> {
        let normalized = normalize_tag_name(tag)?;
        let rows: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, body
                 FROM notes
                 WHERE (' ' || tags || ' ') LIKE ?1"
            )?;
            stmt.query_map([format!("% {} %", normalized)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?.collect::<Result<Vec<_>, _>>()?
        };

        let mut updated = 0usize;
        for (id, body) in rows {
            let new_body = remove_tag_from_body(&body, &normalized);
            if new_body != body {
                self.update_note(id, &new_body)?;
                updated += 1;
            }
        }

        self.conn.execute("DELETE FROM tag_meta WHERE name = ?1", params![normalized])?;
        Ok(updated)
    }
}

impl Store {
    fn list_notes_internal(
        &self,
        query: &str,
        folder_scope: Option<&str>,
        forced_archived: bool,
        forced_trash: bool,
    ) -> Result<Vec<NoteSummary>> {
        let (tags, folder_filter, query_archived, query_trash, fts) = parse_query(query);
        let show_archived = forced_archived || query_archived;
        let show_trash = forced_trash || query_trash;
        if let Some(scope) = folder_scope {
            if let Some(filter) = folder_filter.as_deref() {
                if !scope.eq_ignore_ascii_case(filter) {
                    return Ok(Vec::new());
                }
            }
        }

        let mut where_clauses: Vec<String> = Vec::new();
        let mut bind_vals: Vec<String> = Vec::new();

        let has_fts = !fts.is_empty();
        if has_fts {
            where_clauses.push("notes_fts MATCH ?".to_string());
            bind_vals.push(fts);
        }

        where_clauses.push(if show_trash {
            "n.deleted_at IS NOT NULL".to_string()
        } else {
            "n.deleted_at IS NULL".to_string()
        });

        where_clauses.push(if show_archived {
            "n.archived = 1".to_string()
        } else {
            "n.archived = 0".to_string()
        });

        for tag in &tags {
            where_clauses.push("(' ' || n.tags || ' ') LIKE ?".to_string());
            bind_vals.push(format!("% {} %", tag));
        }

        match folder_scope {
            Some(scope) => {
                where_clauses.push("n.folder = ?".to_string());
                bind_vals.push(scope.to_string());
            }
            None => {
                if let Some(folder) = folder_filter {
                    where_clauses.push("LOWER(n.folder) = ?".to_string());
                    bind_vals.push(folder);
                }
            }
        }

        let order_by = if folder_scope.is_some() {
            "n.pinned DESC, n.note_order ASC, n.updated_at DESC"
        } else {
            "n.pinned DESC, n.updated_at DESC"
        };
        let limit = if has_fts { 200 } else { 500 };
        let from_clause = if has_fts {
            "FROM notes_fts JOIN notes n ON n.id = notes_fts.rowid"
        } else {
            "FROM notes n"
        };
        let sql = format!(
            "SELECT n.id, n.title, n.updated_at, n.folder, n.tags, \
             {} AS snippet, n.pinned, n.archived, n.note_order \
             {from_clause} \
             WHERE {} \
             ORDER BY {order_by} LIMIT {limit}",
            if has_fts {
                "snippet(notes_fts, 1, '[', ']', ' … ', 12)"
            } else {
                "''"
            },
            where_clauses.join(" AND ")
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bind_vals.iter()), |row| {
            Ok(NoteSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get(2)?,
                folder: row.get(3)?,
                tags: row.get(4)?,
                snippet: row.get(5)?,
                pinned: row.get::<_, i64>(6)? != 0,
                archived: row.get::<_, i64>(7)? != 0,
                note_order: row.get(8)?,
            })
        })?;

        let mut notes = Vec::new();
        for row in rows {
            notes.push(row?);
        }
        Ok(notes)
    }
}

fn resolve_data_dir() -> Result<std::path::PathBuf> {
    if let Ok(path) = std::env::var("ANOTE_DATA_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(std::path::PathBuf::from(trimmed));
        }
    }

    if let Some(base) = dirs::data_local_dir() {
        let path = base.join("anote");
        if std::fs::create_dir_all(&path).is_ok() {
            return Ok(path);
        }
    }

    let cwd = std::env::current_dir().context("could not resolve current directory")?;
    Ok(cwd.join(".anote"))
}

fn derive_title(body: &str) -> String {
    let first = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Untitled")
        .trim();
    if first.is_empty() {
        "Untitled".to_string()
    } else {
        first.chars().take(80).collect()
    }
}

fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn normalize_tag_name(tag: &str) -> Result<String> {
    let trimmed = tag.trim().trim_start_matches('#').to_ascii_lowercase();
    if trimmed.len() < 2 {
        bail!("tag must be at least 2 characters");
    }
    if !trimmed.chars().all(is_tag_char) {
        bail!("tag may only use letters, numbers, '_' and '-'");
    }
    Ok(trimmed)
}

fn extract_tags(body: &str) -> String {
    let first_line = body.lines().next().unwrap_or("");
    let mut tags: Vec<String> = Vec::new();
    let chars: Vec<char> = first_line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' {
            i += 1;
            let start = i;
            while i < chars.len() && is_tag_char(chars[i]) {
                i += 1;
            }
            let tag: String = chars[start..i].iter().collect();
            if tag.len() >= 2 {
                let lower = tag.to_ascii_lowercase();
                if !tags.contains(&lower) {
                    tags.push(lower);
                }
            }
        } else {
            i += 1;
        }
    }
    tags.join(" ")
}

fn is_tag_boundary(c: char) -> bool {
    !c.is_ascii_alphanumeric() && c != '_' && c != '-'
}

fn remove_tag_from_body(body: &str, tag: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(|line| line.to_string()).collect();
    if lines.is_empty() {
        return String::new();
    }

    let first_line = lines[0].clone();
    let lower = first_line.to_ascii_lowercase();
    let needle = format!("#{}", tag);
    let mut remove_ranges: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0;

    while pos < lower.len() {
        if let Some(found) = lower[pos..].find(&needle) {
            let abs = pos + found;
            let after = abs + needle.len();
            let prev_ok = if abs == 0 {
                true
            } else {
                lower[..abs]
                    .chars()
                    .next_back()
                    .map(is_tag_boundary)
                    .unwrap_or(true)
            };
            let next_ok = lower[after..]
                .chars()
                .next()
                .map(is_tag_boundary)
                .unwrap_or(true);
            if prev_ok && next_ok {
                let start = first_line[..abs]
                    .char_indices()
                    .rev()
                    .find(|(_, c)| !c.is_whitespace())
                    .map(|(idx, _)| idx + first_line[idx..].chars().next().unwrap().len_utf8())
                    .unwrap_or(abs);
                let trim_start = first_line[..start]
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_whitespace())
                    .map(|(idx, _)| idx)
                    .unwrap_or(abs);
                let remove_start = if trim_start < abs { trim_start } else { abs };

                let mut remove_end = after;
                while let Some(ch) = first_line[remove_end..].chars().next() {
                    if ch.is_whitespace() {
                        remove_end += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                remove_ranges.push((remove_start, remove_end));
            }
            pos = after;
        } else {
            break;
        }
    }

    if remove_ranges.is_empty() {
        return body.to_string();
    }

    let mut rebuilt = String::new();
    let mut cursor = 0;
    for (start, end) in remove_ranges {
        if start > cursor {
            rebuilt.push_str(&first_line[cursor..start]);
        }
        cursor = end;
    }
    if cursor < first_line.len() {
        rebuilt.push_str(&first_line[cursor..]);
    }
    lines[0] = rebuilt.split_whitespace().collect::<Vec<_>>().join(" ");
    lines.join("\n")
}

/// Parse a search query into (tags, folder, show_archived, show_trash, fts_text).
fn parse_query(query: &str) -> (Vec<String>, Option<String>, bool, bool, String) {
    let mut tags: Vec<String> = Vec::new();
    let mut folder: Option<String> = None;
    let mut show_archived = false;
    let mut show_trash = false;
    let mut fts_tokens: Vec<String> = Vec::new();

    for token in query.split_whitespace() {
        if token.eq_ignore_ascii_case(":archived") {
            show_archived = true;
        } else if token.eq_ignore_ascii_case(":trash") {
            show_trash = true;
        } else if let Some(rest) = token.strip_prefix('#') {
            if !rest.is_empty() {
                tags.push(rest.to_ascii_lowercase());
            }
        } else if let Some(rest) = token.strip_prefix('/') {
            if !rest.is_empty() {
                folder = Some(rest.to_ascii_lowercase());
            }
        } else {
            fts_tokens.push(fts_literal_token(token));
        }
    }

    let fts = fts_tokens.join(" ");
    (tags, folder, show_archived, show_trash, fts)
}

fn fts_literal_token(token: &str) -> String {
    let escaped = token.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

#[cfg(test)]
mod tests {
    use super::Store;

    #[test]
    fn create_note_extracts_tags_immediately() {
        let store = Store::open_for_test().unwrap();
        let id = store.create_note("Untitled", "Ship #Rust #fts today").unwrap();

        let notes = store.list_notes("#rust").unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, id);
        assert_eq!(notes[0].tags, "rust fts");
    }

    #[test]
    fn locked_titles_survive_body_updates() {
        let store = Store::open_for_test().unwrap();
        let id = store
            .create_note_with_title_lock("Imported Name", "first line\nbody", true)
            .unwrap();

        store.update_note(id, "changed first line\nbody").unwrap();
        let note = store.list_notes("").unwrap().into_iter().find(|note| note.id == id).unwrap();
        assert_eq!(note.title, "Imported Name");
        assert!(store.is_title_locked(id).unwrap());
    }

    #[test]
    fn folder_queries_honor_fts_and_tags() {
        let store = Store::open_for_test().unwrap();
        let alpha = store.create_note("Untitled", "alpha rust #work").unwrap();
        let beta = store.create_note("Untitled", "beta rust #personal").unwrap();
        store.set_folder(alpha, "projects").unwrap();
        store.set_folder(beta, "projects").unwrap();

        let notes = store.list_notes_in_folder("projects", "rust #work").unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, alpha);
    }

    #[test]
    fn fts_search_treats_colons_as_literal_text() {
        let store = Store::open_for_test().unwrap();
        let id = store
            .create_note("Untitled", "Use context-mode:ctx-stats for plugin checks")
            .unwrap();

        let notes = store.list_notes("context-mode:ctx-stats").unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, id);
    }

    #[test]
    fn tag_meta_includes_unused_tags_and_colors() {
        let store = Store::open_for_test().unwrap();
        store.create_note("Untitled", "alpha #rust body").unwrap();
        store.create_tag("idea").unwrap();
        store.set_tag_color("idea", Some("purple")).unwrap();
        store.set_tag_color("rust", Some("teal")).unwrap();

        let tags = store.list_tags().unwrap();
        assert_eq!(tags[0].tag, "rust");
        assert_eq!(tags[0].count, 1);
        assert_eq!(tags[0].color.as_deref(), Some("teal"));

        let idea = tags.iter().find(|entry| entry.tag == "idea").unwrap();
        assert_eq!(idea.count, 0);
        assert_eq!(idea.color.as_deref(), Some("purple"));

        let colors = store.list_tag_colors().unwrap();
        assert_eq!(colors.get("rust").map(String::as_str), Some("teal"));
        assert_eq!(colors.get("idea").map(String::as_str), Some("purple"));
    }

    #[test]
    fn delete_tag_everywhere_removes_tag_from_notes_and_meta() {
        let store = Store::open_for_test().unwrap();
        let id_a = store.create_note("Untitled", "alpha #rust #work\nbody").unwrap();
        let id_b = store.create_note("Untitled", "beta #rust\nbody").unwrap();
        store.set_tag_color("rust", Some("teal")).unwrap();

        let updated = store.delete_tag_everywhere("rust").unwrap();
        assert_eq!(updated, 2);

        let note_a = store.get_note(id_a).unwrap().unwrap();
        let note_b = store.get_note(id_b).unwrap().unwrap();
        assert!(!note_a.body.contains("#rust"));
        assert!(!note_b.body.contains("#rust"));
        assert!(note_a.body.contains("#work"));

        let tags = store.list_tags().unwrap();
        assert!(tags.iter().all(|entry| entry.tag != "rust"));
    }
}
