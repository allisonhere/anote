mod config;
mod editor;
mod render;
mod storage;
mod tui;
mod types;
mod utils;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use storage::Store;
use tui::App;

#[derive(Parser, Debug)]
#[command(name = "anote", version, about = "A fast TUI note app")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Open interactive TUI
    Tui,

    /// Capture a quick note from the command line
    Capture {
        /// Optional explicit title
        #[arg(short, long)]
        title: Option<String>,
        /// Body text of the note (reads stdin if omitted)
        body: Option<String>,
    },

    /// Import one or more markdown/text files as notes
    Import {
        /// Files to import
        #[arg(required = true)]
        files: Vec<std::path::PathBuf>,
        /// Override title (single-file import only)
        #[arg(short, long)]
        title: Option<String>,
    },

    /// Export a note body to stdout or a file
    Export {
        /// Note ID
        id: i64,
        /// Write to file instead of stdout
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },

    /// List notes
    List {
        /// Filter query (#tag /folder text)
        #[arg(short, long, default_value = "")]
        query: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Search notes and print matches
    Search {
        /// FTS query string (e.g. "rust AND async")
        query: String,
    },

    /// Delete a note by ID (soft-delete, moves to trash)
    Delete {
        /// Note ID(s) to delete
        #[arg(required = true)]
        ids: Vec<i64>,
    },

    /// Restore a trashed note by ID
    Restore {
        /// Note ID(s) to restore
        #[arg(required = true)]
        ids: Vec<i64>,
    },

    /// Permanently delete a trashed note by ID
    Purge {
        /// Note ID(s) to purge
        #[arg(required = true)]
        ids: Vec<i64>,
    },

    /// List all tags and their counts
    ListTags {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Open the TUI with a specific note selected
    Edit {
        /// Note ID
        id: i64,
    },

    /// Open today's daily note (creates it with a template if it doesn't exist)
    Daily,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open_default()?;

    match cli.command {
        None | Some(Commands::Tui) => {
            App::new(store)?.run()?;
        }

        Some(Commands::Capture { title, body }) => {
            let body = match body {
                Some(b) => b,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            let id = store.capture(title.as_deref(), &body)?;
            println!("captured note {}", id);
        }

        Some(Commands::Import { files, title }) => {
            if title.is_some() && files.len() > 1 {
                bail!("--title can only be used when importing a single file");
            }
            for path in &files {
                let body = std::fs::read_to_string(path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let note_title = match &title {
                    Some(t) => t.clone(),
                    None => path
                        .file_stem()
                        .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
                        .unwrap_or_else(|| "Untitled".to_string()),
                };
                let id = store.create_note_with_title_lock(&note_title, &body, true)?;
                println!("imported '{}' as note {}", note_title, id);
            }
        }

        Some(Commands::Export { id, output }) => {
            match store.get_note(id)? {
                None => bail!("note {} not found", id),
                Some(note) => match output {
                    None => {
                        let body = note.body;
                        if body.ends_with('\n') {
                            print!("{}", body);
                        } else {
                            println!("{}", body);
                        }
                    }
                    Some(path) => {
                        std::fs::write(&path, &note.body)
                            .with_context(|| format!("failed to write {}", path.display()))?;
                        println!("exported note {} to {}", id, path.display());
                    }
                },
            }
        }

        Some(Commands::List { query, json }) => {
            let notes = store.list_notes(&query)?;
            if json {
                println!("[");
                for (i, n) in notes.iter().enumerate() {
                    let comma = if i + 1 < notes.len() { "," } else { "" };
                    println!(
                        "  {{\"id\":{},\"title\":{},\"updated_at\":{},\"folder\":{},\"tags\":{},\"pinned\":{},\"archived\":{}}}{}",
                        n.id,
                        json_str(&n.title),
                        json_str(&n.updated_at),
                        json_str(&n.folder),
                        json_str(&n.tags),
                        n.pinned,
                        n.archived,
                        comma
                    );
                }
                println!("]");
            } else {
                for n in notes {
                    println!("{}\t{}\t{}", n.id, n.updated_at, n.title);
                }
            }
        }

        Some(Commands::Search { query }) => {
            let notes = store.list_notes(&query)?;
            for n in notes {
                println!("{}\t{}\t{}", n.id, n.updated_at, n.title);
            }
        }

        Some(Commands::Delete { ids }) => {
            for id in ids {
                store.delete_note(id)?;
                println!("deleted note {}", id);
            }
        }

        Some(Commands::Restore { ids }) => {
            for id in ids {
                store.restore_note(id)?;
                println!("restored note {}", id);
            }
        }

        Some(Commands::Purge { ids }) => {
            for id in ids {
                store.purge_note(id)?;
                println!("purged note {}", id);
            }
        }

        Some(Commands::ListTags { json }) => {
            let tags = store.list_tags()?;
            if json {
                println!("[");
                for (i, tag) in tags.iter().enumerate() {
                    let comma = if i + 1 < tags.len() { "," } else { "" };
                    println!(
                        "  {{\"tag\":{},\"count\":{},\"color\":{}}}{}",
                        json_str(&tag.tag),
                        tag.count,
                        json_str(&tag.color.clone().unwrap_or_default()),
                        comma
                    );
                }
                println!("]");
            } else {
                for tag in tags {
                    let color = tag.color.as_deref().unwrap_or("-");
                    println!("{}\t{}\t{}", tag.tag, tag.count, color);
                }
            }
        }

        Some(Commands::Edit { id }) => {
            let mut app = App::new(store)?;
            app.open_note_id(id, false)?;
            app.run()?;
        }

        Some(Commands::Daily) => {
            let mut app = App::new(store)?;
            app.open_daily_note()?;
            app.run()?;
        }
    }

    Ok(())
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let code = c as u32;
                out.push_str(&format!("\\u{:04x}", code));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
