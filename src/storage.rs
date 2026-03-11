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
}

#[derive(Debug, Clone)]
pub struct Note {
    pub body: String,
}

pub struct Store {
    conn: Connection,
}

impl Store {
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
        // Suppress "duplicate column name" errors so this is safe on both fresh and existing DBs.
        for sql in &[
            "ALTER TABLE notes ADD COLUMN folder TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE notes ADD COLUMN tags   TEXT NOT NULL DEFAULT ''",
        ] {
            match self.conn.execute_batch(sql) {
                Ok(_) => {}
                Err(e) if e.to_string().contains("duplicate column name") => {}
                Err(e) => return Err(e.into()),
            }
        }

        Ok(())
    }

    pub fn list_notes(&self, query: &str) -> Result<Vec<NoteSummary>> {
        let (tags, folder, fts) = parse_query(query);

        let mut notes = Vec::new();

        if fts.is_empty() {
            // No FTS — scan notes table directly
            let mut where_clauses: Vec<String> = vec!["n.archived = 0".to_string()];
            let mut bind_vals: Vec<String> = Vec::new();

            for tag in &tags {
                where_clauses
                    .push("(' ' || n.tags || ' ') LIKE ?".to_string());
                bind_vals.push(format!("% {} %", tag));
            }
            if let Some(ref f) = folder {
                where_clauses.push("LOWER(n.folder) = ?".to_string());
                bind_vals.push(f.clone());
            }

            let sql = format!(
                "SELECT n.id, n.title, n.updated_at, n.folder, n.tags \
                 FROM notes n \
                 WHERE {} \
                 ORDER BY n.pinned DESC, n.updated_at DESC LIMIT 500",
                where_clauses.join(" AND ")
            );

            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(bind_vals.iter()),
                |row| {
                    Ok(NoteSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        updated_at: row.get(2)?,
                        folder: row.get(3)?,
                        tags: row.get(4)?,
                    })
                },
            )?;
            for row in rows {
                notes.push(row?);
            }
        } else {
            // FTS query
            let mut where_clauses: Vec<String> =
                vec!["notes_fts MATCH ?".to_string(), "n.archived = 0".to_string()];
            let mut bind_vals: Vec<String> = vec![fts.clone()];

            for tag in &tags {
                where_clauses
                    .push("(' ' || n.tags || ' ') LIKE ?".to_string());
                bind_vals.push(format!("% {} %", tag));
            }
            if let Some(ref f) = folder {
                where_clauses.push("LOWER(n.folder) = ?".to_string());
                bind_vals.push(f.clone());
            }

            let sql = format!(
                "SELECT n.id, n.title, n.updated_at, n.folder, n.tags \
                 FROM notes_fts f \
                 JOIN notes n ON n.id = f.rowid \
                 WHERE {} \
                 ORDER BY n.pinned DESC, n.updated_at DESC LIMIT 200",
                where_clauses.join(" AND ")
            );

            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(bind_vals.iter()),
                |row| {
                    Ok(NoteSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        updated_at: row.get(2)?,
                        folder: row.get(3)?,
                        tags: row.get(4)?,
                    })
                },
            )?;
            for row in rows {
                notes.push(row?);
            }
        }

        Ok(notes)
    }

    pub fn get_note(&self, id: i64) -> Result<Option<Note>> {
        let mut stmt = self.conn.prepare("SELECT body FROM notes WHERE id = ?1")?;
        let note = stmt
            .query_row([id], |row| Ok(Note { body: row.get(0)? }))
            .optional()?;
        Ok(note)
    }

    pub fn create_note(&self, title: &str, body: &str) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO notes (title, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![title, body, now, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn capture(&self, title: Option<&str>, body: &str) -> Result<i64> {
        let title = match title {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => body
                .lines()
                .next()
                .unwrap_or("Untitled")
                .trim()
                .chars()
                .take(80)
                .collect::<String>(),
        };
        self.create_note(&title, body)
    }

    pub fn update_note(&self, id: i64, body: &str) -> Result<()> {
        let title = derive_title(body);
        let now = Utc::now().to_rfc3339();
        let tags = extract_tags(body);
        self.conn.execute(
            "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3, tags = ?4 WHERE id = ?5",
            params![title, body, now, tags, id],
        )?;
        Ok(())
    }

    pub fn set_folder(&self, id: i64, folder: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE notes SET folder = ?1 WHERE id = ?2",
            params![folder.trim(), id],
        )?;
        Ok(())
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        Ok(())
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

/// Parse a search query into (tags, folder, fts_text).
/// Tokens starting with '#' → tag filter (lowercased, no '#').
/// Tokens starting with '/' → folder filter (lowercased, no '/'; last one wins).
/// Everything else → FTS text (rejoined).
fn parse_query(query: &str) -> (Vec<String>, Option<String>, String) {
    let mut tags: Vec<String> = Vec::new();
    let mut folder: Option<String> = None;
    let mut fts_tokens: Vec<String> = Vec::new();

    for token in query.split_whitespace() {
        if let Some(rest) = token.strip_prefix('#') {
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
    (tags, folder, fts)
}
