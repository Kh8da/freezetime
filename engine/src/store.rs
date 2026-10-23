//! freezetime's own files: `settings.json` and `history.json` in the app's data folder.

use crate::engine::Rules;
use crate::stats::History;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 38127;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    #[serde(flatten)]
    pub rules: Rules,
    /// Play the notification sound with warnings and reminders.
    pub sound: bool,
    pub port: u16,
    /// Shared secret written into the cfg; CS2 sends it with every update.
    pub token: String,
    /// CS2's cfg folder, when it was picked by hand instead of found through Steam.
    pub cfg_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { rules: Rules::default(), sound: true, port: DEFAULT_PORT, token: String::new(), cfg_dir: None }
    }
}

pub fn new_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("the OS random number generator is unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| {
            // Keep the unreadable file around instead of silently losing it.
            let _ = fs::rename(path, path.with_extension("json.broken"));
            T::default()
        }),
        Err(_) => T::default(),
    }
}

fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("couldn't save {}: {e}", path.display()))?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("couldn't save {}: {e}", path.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("couldn't save {}: {e}", path.display()))
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Loads settings, creating a token on first run and saving it straight away
    /// so the cfg and the app always agree.
    pub fn settings(&self) -> Settings {
        let path = self.dir.join("settings.json");
        let mut s: Settings = load(&path);
        let mut dirty = !path.exists();
        if s.token.len() < 16 {
            s.token = new_token();
            dirty = true;
        }
        if s.port == 0 {
            s.port = DEFAULT_PORT;
            dirty = true;
        }
        if dirty {
            let _ = self.save_settings(&s);
        }
        s
    }

    pub fn save_settings(&self, s: &Settings) -> Result<(), String> {
        save(&self.dir.join("settings.json"), s)
    }

    pub fn history(&self) -> History {
        load(&self.dir.join("history.json"))
    }

    pub fn save_history(&self, h: &History) -> Result<(), String> {
        save(&self.dir.join("history.json"), h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_creates_a_token_and_keeps_it() {
        let dir = std::env::temp_dir().join(format!("freezetime-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = Store::new(dir.clone());
        let a = store.settings();
        assert_eq!(a.token.len(), 32);
        assert!(a.token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(store.settings().token, a.token);
        assert_ne!(new_token(), a.token);
        // Older or hand-edited files fill in what's missing.
        fs::write(dir.join("settings.json"), r#"{"music":false}"#).unwrap();
        let b = store.settings();
        assert!(!b.rules.music);
        assert_eq!(b.rules.tilt_after, 3);
        assert_eq!(b.port, DEFAULT_PORT);
        // A broken file is set aside, not lost.
        fs::write(dir.join("history.json"), "{nope").unwrap();
        assert_eq!(store.history(), History::default());
        assert!(dir.join("history.json.broken").exists());
    }
}
