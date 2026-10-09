//! Config persistence and parse-error reporting; schema and migrations are separate.
use super::{Config, config_path, has_legacy_composables, migrate_legacy_top};
use std::{path::Path, time::SystemTime};
static LAST_PARSE_ERROR: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);
pub(super) fn read_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
pub(crate) fn take_parse_error() -> Option<(String, String)> {
    LAST_PARSE_ERROR.lock().ok()?.take()
}
pub(super) fn note_parse_error(source: &str, error: &impl std::fmt::Display) {
    if let Ok(mut slot) = LAST_PARSE_ERROR.lock() {
        *slot = Some((source.to_owned(), error.to_string()));
    }
}
pub(super) fn parse(content: &str) -> Config {
    match toml::from_str(content) {
        Ok(cfg) => migrate_legacy_top(cfg),
        Err(error) => {
            eprintln!("config: parse error, keeping defaults: {error}");
            note_parse_error("config", &error);
            Config::default()
        }
    }
}
impl Config {
    pub fn load_from(path: &Path) -> (Self, Option<SystemTime>) {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let cfg = parse(&content);
                if has_legacy_composables(&content) && toml::from_str::<Config>(&content).is_ok() {
                    let backup = path.with_extension("toml.pre-composable");
                    let migrated = (|| -> Result<(), Box<dyn std::error::Error>> {
                        if !backup.exists() {
                            std::fs::copy(path, &backup)?;
                        }
                        std::fs::write(path, toml::to_string_pretty(&cfg)?)?;
                        Ok(())
                    })();
                    if let Err(error) = migrated {
                        eprintln!("config: cannot persist composable migration: {error}");
                    }
                }
                (cfg, read_mtime(path))
            }
            Err(_) => (Config::default(), None),
        }
    }
    pub fn load() -> (Self, Option<SystemTime>) {
        let path = config_path();
        if !path.exists() {
            let cfg = Config::default();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match toml::to_string_pretty(&cfg) {
                Ok(text) => {
                    if let Err(error) = std::fs::write(&path, text) {
                        eprintln!("config: cannot write {}: {error}", path.display());
                    }
                }
                Err(error) => eprintln!("config: cannot serialize defaults: {error}"),
            }
            return (cfg, read_mtime(&path));
        }
        Self::load_from(&path)
    }
    pub fn poll(known_mtime: &Option<SystemTime>) -> Option<(Self, Option<SystemTime>)> {
        let path = config_path();
        let mtime = read_mtime(&path);
        if mtime != *known_mtime && path.exists() {
            let (cfg, mtime) = Self::load_from(&path);
            if mtime != *known_mtime {
                return Some((cfg, mtime));
            }
        }
        None
    }
    pub fn save(&self) -> Option<SystemTime> {
        let path = config_path();
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(error) = std::fs::write(&path, text) {
                    eprintln!("config: cannot write {}: {error}", path.display());
                    return read_mtime(&path);
                }
                read_mtime(&path)
            }
            Err(error) => {
                eprintln!("config: cannot serialize: {error}");
                None
            }
        }
    }
}
