//! Tiny persisted settings (last opened library folder).

use crate::library::Role;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub library: Option<PathBuf>,
    /// Folder roles the user picked (absolute paths). Unlisted folders are guessed.
    #[serde(default)]
    pub roles: HashMap<PathBuf, Role>,
    /// Folders whose art was set in easytag. New songs added to them get
    /// that art automatically.
    #[serde(default)]
    pub folder_art: HashSet<PathBuf>,
}

fn file() -> Option<PathBuf> {
    // Tests never touch the real settings file.
    if cfg!(test) {
        return Some(
            std::env::temp_dir()
                .join(format!("easytag-test-settings-{}", std::process::id()))
                .join("settings.json"),
        );
    }
    Some(dirs::config_dir()?.join("easytag").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(f, s);
        }
    }
}
