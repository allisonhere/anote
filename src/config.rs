use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub theme: String,
    pub keymap: String,
    pub density: String,
    pub sort: String,
    pub last_open_note_id: Option<i64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: "neo-noir".to_string(),
            keymap: "default".to_string(),
            density: "cozy".to_string(),
            sort: "manual".to_string(),
            last_open_note_id: None,
        }
    }
}

impl AppConfig {
    pub fn load_default() -> Result<(Self, PathBuf)> {
        let path = resolve_config_path()?;
        if !path.exists() {
            return Ok((Self::default(), path));
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed reading config {}", path.display()))?;
        let parsed = toml::from_str::<Self>(&content).unwrap_or_default();
        Ok((parsed, path))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed creating config directory {}", parent.display())
            })?;
        }
        let serialized = toml::to_string_pretty(self).context("failed serializing config")?;
        std::fs::write(path, serialized)
            .with_context(|| format!("failed writing config {}", path.display()))?;
        Ok(())
    }
}

fn resolve_config_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("ANOTE_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    if let Some(base) = dirs::config_dir() {
        let dir = base.join("anote");
        if std::fs::create_dir_all(&dir).is_ok() {
            return Ok(dir.join("config.toml"));
        }
    }

    let cwd = std::env::current_dir().context("could not resolve current directory")?;
    Ok(cwd.join(".anote").join("config.toml"))
}
