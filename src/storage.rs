use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone)]
pub struct NoteSummary {
    pub id: i64,
    pub title: String,
    pub updated_at: String,
    pub folder: String,
    pub tags: String,
    pub pinned: bool,
    pub archived: bool,
    pub note_order: i64,
}

#[derive(Debug, Clone)]
pub struct FolderEntry {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
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
        self.list_notes_internal(query, None)
    }

    pub fn list_notes_in_folder(&self, folder: &str, query: &str) -> Result<Vec<NoteSummary>> {
        self.list_notes_internal(query, Some(folder))
    }

    pub fn list_folders(&self) -> Result<Vec<FolderEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, sort_order FROM folders ORDER BY sort_order, name"
        )?;
        let folders = stmt.query_map([], |row| {
            Ok(FolderEntry { id: row.get(0)?, name: row.get(1)?, sort_order: row.get(2)? })
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
        self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        Ok(())
    }
}

impl Store {
    fn list_notes_internal(&self, query: &str, folder_scope: Option<&str>) -> Result<Vec<NoteSummary>> {
        let (tags, folder_filter, show_archived, fts) = parse_query(query);
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
            "SELECT n.id, n.title, n.updated_at, n.folder, n.tags, n.pinned, n.archived, n.note_order \
             {from_clause} \
             WHERE {} \
             ORDER BY {order_by} LIMIT {limit}",
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
                pinned: row.get::<_, i64>(5)? != 0,
                archived: row.get::<_, i64>(6)? != 0,
                note_order: row.get(7)?,
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

fn extract_tags(body: &str) -> String {
    let mut tags: Vec<String> = Vec::new();
    let chars: Vec<char> = body.chars().collect();
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

/// Parse a search query into (tags, folder, show_archived, fts_text).
fn parse_query(query: &str) -> (Vec<String>, Option<String>, bool, String) {
    let mut tags: Vec<String> = Vec::new();
    let mut folder: Option<String> = None;
    let mut show_archived = false;
    let mut fts_tokens: Vec<String> = Vec::new();

    for token in query.split_whitespace() {
        if token.eq_ignore_ascii_case(":archived") {
            show_archived = true;
        } else if let Some(rest) = token.strip_prefix('#') {
            if !rest.is_empty() {
                tags.push(rest.to_ascii_lowercase());
            }
        } else if let Some(rest) = token.strip_prefix('/') {
            if !rest.is_empty() {
                folder = Some(rest.to_ascii_lowercase());
            }
        } else {
            fts_tokens.push(token.to_string());
        }
    }

    let fts = fts_tokens.join(" ");
    (tags, folder, show_archived, fts)
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
}
