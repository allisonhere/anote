mod config;
mod storage;
mod tui;

use anyhow::Result;
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
    /// Capture a note quickly
    Capture {
        /// Optional explicit title
        #[arg(short, long)]
        title: Option<String>,
        /// Body text of the note
        body: String,
    },
    /// Search notes from CLI
    Search {
        /// FTS query string (e.g. "rust AND async")
        query: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open_default()?;

    match cli.command {
        Some(Commands::Capture { title, body }) => {
            let id = store.capture(title.as_deref(), &body)?;
            println!("captured note {}", id);
        }
        Some(Commands::Search { query }) => {
            let notes = store.list_notes(&query)?;
            for n in notes {
                println!("{}\t{}\t{}", n.id, n.updated_at, n.title);
            }
        }
        Some(Commands::Tui) | None => {
            let app = App::new(store)?;
            app.run()?;
        }
    }

    Ok(())
}
