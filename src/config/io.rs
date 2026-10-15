//! Config persistence and parse-error reporting; schema and migrations are separate.
use super::migrate::{has_legacy_composables, migrate_composable_sources, migrate_legacy_top};
use super::{Config, config_path};
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
    match parse_checked(content) {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("config: parse error, keeping defaults: {error}");
            note_parse_error("config", &error);
            Config::default()
        }
    }
}
fn parse_checked(content: &str) -> Result<Config, toml::de::Error> {
    let mut value: toml::Value = toml::from_str(content)?;
    migrate_composable_sources(&mut value);
    value.try_into().map(migrate_legacy_top)
}
impl Config {
    pub(crate) fn save_atomic_to(&self, path: &Path) -> Result<Option<SystemTime>, String> {
        let text = toml::to_string_pretty(self).map_err(|error| error.to_string())?;
        let temporary = path.with_extension(format!("toml.{}.pending", std::process::id()));
        let result = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&temporary, text).map_err(|error| error.to_string())?;
            if let Ok(metadata) = std::fs::metadata(path) {
                std::fs::set_permissions(&temporary, metadata.permissions())
                    .map_err(|error| error.to_string())?;
            }
            std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
            Ok(read_mtime(path))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }

    pub fn load_from(path: &Path) -> (Self, Option<SystemTime>) {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let cfg = parse(&content);
                if has_legacy_composables(&content) && parse_checked(&content).is_ok() {
                    let backup = path.with_extension("toml.pre-composable-sources");
                    let migrated = (|| -> Result<(), Box<dyn std::error::Error>> {
                        if !backup.exists() {
                            std::fs::copy(path, &backup)?;
                        }
                        cfg.save_atomic_to(path)?;
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
