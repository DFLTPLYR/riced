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
        Ok((cfg, _)) => cfg,
        Err(error) => {
            eprintln!("config: parse error, keeping defaults: {error}");
            note_parse_error("config", &error);
            Config::default()
        }
    }
}
fn parse_checked(content: &str) -> Result<(Config, bool), toml::de::Error> {
    let mut value: toml::Value = toml::from_str(content)?;
    let original = value.clone();
    migrate_composable_sources(&mut value);
    super::format_migration::migrate(&mut value).map_err(serde::de::Error::custom)?;
    let migrated = value != original;
    value
        .try_into()
        .map(|cfg| (migrate_legacy_top(cfg), migrated))
}
impl Config {
    pub(crate) fn save_atomic_to(&self, path: &Path) -> Result<Option<SystemTime>, String> {
        let text = super::format::to_string(self)?;
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
                let (cfg, migrated) = match parse_checked(&content) {
                    Ok(parsed) => parsed,
                    Err(_) => return (parse(&content), read_mtime(path)),
                };
                if migrated {
                    let backup = path.with_extension(if has_legacy_composables(&content) {
                        "toml.pre-composable-sources"
                    } else {
                        "toml.pre-structured"
                    });
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
            match super::format::to_string(&cfg) {
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
        match self.save_atomic_to(&path) {
            Ok(stamp) => stamp,
            Err(error) => {
                eprintln!("config: cannot serialize: {error}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_layout_migrates_once_without_losing_wallpaper_or_slot_overrides() {
        let dir =
            std::env::temp_dir().join(format!("riced-structured-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = r#"
            [background]
            mode = "free-form"
            [[background.monitor]]
            output = "DP-2"
            path = "portrait.png"
            fit = "stretch"
            [[background.image]]
            path = "free-form.png"
            x = -291.53717
            y = -132.1731
            scale = 1.0
            width = 0.0
            height = 0.0
            [theme]
            darkmode = false
            templates_dir = "off"
            [notification]
            timeout_ms = 1234
            [[bar]]
            slots = 3
            aligns = ["start", "center", "end"]
            widgets = [[{ name = "clock", interval = 1.0, size = 8.0, props = { format = "%H" } }], "stats", []]
            slot_spacing = 11.0
            margin_top = -2
            margin_left = 3
            radius_top_right = 9.0
        "#;
        std::fs::write(&path, original).unwrap();
        let (config, stamp) = Config::load_from(&path);
        assert_eq!(config.bar[0].slots, 3);
        assert_eq!(config.bar[0].margin_top, -2);
        assert_eq!(config.bar[0].radius_top_right, 9.0);
        assert_eq!(config.bar[0].slot_spacing, 11.0);
        assert!(!config.theme.darkmode);
        assert_eq!(config.theme.templates_dir, "off");
        assert_eq!(config.notifications.timeout_ms, 1234);
        assert_eq!(config.background.monitor[0].output, "DP-2");
        assert_eq!(config.background.image[0].x, -291.53717);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("aligns ="));
        assert!(!saved.contains("margin_top"));
        assert!(saved.contains("[background.monitors.DP-2]"));
        assert_eq!(
            std::fs::read_to_string(path.with_extension("toml.pre-structured")).unwrap(),
            original
        );
        let (reloaded, reloaded_stamp) = Config::load_from(&path);
        assert_eq!(stamp, reloaded_stamp);
        assert_eq!(
            serde_json::to_value(config).unwrap(),
            serde_json::to_value(reloaded).unwrap()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn conflicting_formats_are_rejected_before_persistence() {
        for content in [
            "[[bar]]\nmargin = 8\nmargin_top = 4",
            "[[bar]]\nslots = [{ widgets = ['clock'] }]\nwidgets = [['stats']]",
            "[theme]\nmode = 'dark'\ndarkmode = false",
            "[[background.monitor]]\noutput='DP-1'\n[[background.monitor]]\noutput='DP-1'",
        ] {
            assert!(parse_checked(content).is_err(), "accepted {content}");
        }
    }
}
