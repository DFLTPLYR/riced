//! Config persistence and parse-error reporting; schema and migrations are separate.
use super::migrate::migrate_legacy_top_table;
use super::{Config, config_migrated_path, config_path, legacy_config_path};
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
/// Parse a `config.lua` source into the resolved config. The source runs in
/// the restricted config sandbox and must return a plain-data table.
///
/// Parse errors are always reported through [`note_parse_error`] (the
/// Settings panel mirrors them as notifications); callers fall back to
/// defaults or keep the last good config depending on context.
pub(super) fn parse(content: &str) -> Config {
    match try_parse(content) {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("config: parse error, keeping defaults: {error}");
            note_parse_error("config", &error);
            Config::default()
        }
    }
}
fn try_parse(content: &str) -> Result<Config, String> {
    super::lua::parse(content, "config.lua")
}
/// Evaluate one legacy `config.toml` into the new resolved `Config`. Used
/// only by the one-time TOML → Lua migration; the daemon never reads TOML.
///
/// `top` needs its own deserialize step: `format_migration` normalizes each
/// legacy `[top.<name>]` table into the structured slot shape, the map moves
/// into the `bar` array by name, and `Config` parses without legacy fields.
pub(super) fn parse_legacy_toml(content: &str) -> Result<Config, String> {
    let mut value: toml::Value = toml::from_str(content).map_err(|e| e.to_string())?;
    super::migrate::migrate_composable_sources(&mut value)?;
    rename_key(
        value.as_table_mut().expect("TOML root"),
        "top",
        "__legacy_top",
    )?;
    super::format_migration::migrate(&mut value)?;
    legacy_rename(&mut value)?;
    // Legacy `[top.<name>]` arrives here as a map; the `bar` array holds
    // `TopConfig` tables after the normalizers above ran.
    let mut top: std::collections::HashMap<String, super::TopConfig> = value
        .get("__legacy_top")
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(name, entry)| {
                    entry
                        .clone()
                        .try_into()
                        .map(|bar| (name.clone(), bar))
                        .map_err(|e: toml::de::Error| e.to_string())
                })
                .collect::<Result<_, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let root = value.as_table_mut().expect("TOML root");
    root.remove("__legacy_top");
    if let Some(bars) = root.get_mut("bar").and_then(toml::Value::as_array_mut)
        && bars.is_empty()
    {
        root.remove("bar");
    }
    let mut cfg: Config = value
        .try_into()
        .map_err(|e: toml::de::Error| e.to_string())?;
    migrate_legacy_top_table(&mut cfg.bar, &mut top);
    Ok(cfg)
}

/// Rewrite legacy on-disk TOML spellings (kebab-case keys, old `[top.*]`
/// recipes, hyphenated enum values) into the new snake_case shapes the
/// Lua-side schema parses. Migration-only: `config.lua` is canonical.
fn legacy_rename(value: &mut toml::Value) -> Result<(), String> {
    let Some(table) = value.as_table_mut() else {
        return Err("the config root must be a table".into());
    };
    rename_key(table, "component-defaults", "component_defaults")?;
    rename_key(table, "notification", "notifications")?;
    rename_key(table, "overlay", "overlays")?;
    if let Some(table) = table
        .get_mut("composable")
        .and_then(toml::Value::as_table_mut)
    {
        // `format_migration` runs after `migrate_composable_sources` and
        // normalizes the three source keys to snake_case; any other kebab
        // key is a hand-written TOML typo — reject loudly.
        if table.keys().any(|key| key.contains('-')) {
            let keys: Vec<_> = table.keys().cloned().collect();
            return Err(format!("composable has unknown keys: {keys:?}"));
        }
    }
    if let Some(table) = table
        .get_mut("notifications")
        .and_then(toml::Value::as_table_mut)
    {
        rename_key(table, "timeout-ms", "timeout_ms")?;
    }
    if let Some(table) = table.get_mut("theme").and_then(toml::Value::as_table_mut) {
        rename_key(table, "templates-dir", "templates_dir")?;
        rename_key(table, "icon-theme", "icon_theme")?;
    }
    if let Some(table) = table
        .get_mut("background")
        .and_then(toml::Value::as_table_mut)
        && let Some(mode) = table.get("mode").and_then(toml::Value::as_str)
    {
        let normalized = match mode {
            "free-form" => "free_form",
            "per-monitor" => "per_monitor",
            owned @ ("free_form" | "per_monitor") => owned,
            other => return Err(format!("unknown background mode {other:?}")),
        };
        table.insert("mode".into(), toml::Value::String(normalized.to_string()));
    }
    if let Some(Value::Array(bars)) = table.get_mut("bar") {
        for table in bars.iter_mut().filter_map(toml::Value::as_table_mut) {
            legacy_rename_bar(table)?;
        }
    }
    if let Some(table) = table
        .get_mut("__legacy_top")
        .and_then(toml::Value::as_table_mut)
    {
        for table in table.iter_mut().filter_map(|(_, bar)| bar.as_table_mut()) {
            legacy_rename_bar(table)?;
        }
    }
    Ok(())
}

fn rename_key(table: &mut toml::Table, old: &str, new: &str) -> Result<(), String> {
    if let Some(value) = table.remove(old)
        && table.insert(new.into(), value).is_some()
    {
        return Err(format!("both {old:?} and {new:?} are configured"));
    }
    Ok(())
}

fn legacy_rename_bar(table: &mut toml::Table) -> Result<(), String> {
    rename_key(table, "slot-padding", "slot_padding")?;
    rename_key(table, "slot-spacing", "slot_spacing")?;
    rename_key(table, "top", "bar")?;
    Ok(())
}

/// Rename grouped geometry spellings (`top-left` → `top_left`, ...) left by
/// the legacy normalizer; flat keys were already grouped by
/// `format_migration`.
use toml::Value;
impl Config {
    pub(crate) fn save_atomic_to(&self, path: &Path) -> Result<Option<SystemTime>, String> {
        let text = crate::config::format::to_string(self)?;
        let temporary = path.with_extension(format!("lua.{}.pending", std::process::id()));
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

    /// One-time hard migration: `config.toml` → `config.lua` when no Lua
    /// file exists. Parses the legacy TOML, writes the Lua file, then
    /// renames the TOML to `config.toml.migrated` so the runtime never
    /// reads TOML again. Failures leave both files untouched.
    /// One-time hard migration: `config.toml` → `config.lua` when no Lua
    /// file exists. Parses the legacy TOML, writes the Lua file, then
    /// renames the TOML to `config.toml.migrated` so the runtime never
    /// reads TOML again. Failures leave both files untouched. Tests pass an
    /// explicit `migrated_path` so the user's real file stays out of it.
    fn migrate_toml(
        &self,
        toml_path: &Path,
        lua_path: &Path,
        migrated_path: &Path,
    ) -> Result<(Self, Option<SystemTime>), String> {
        let content = std::fs::read_to_string(toml_path).map_err(|error| error.to_string())?;
        let cfg = parse_legacy_toml(&content)?;
        cfg.save_atomic_to(lua_path)?;
        if !migrated_path.exists() {
            std::fs::rename(toml_path, migrated_path).map_err(|error| error.to_string())?;
        }
        eprintln!(
            "config: migrated {} → {} (original kept as {})",
            toml_path.display(),
            lua_path.display(),
            migrated_path.display()
        );
        Ok((cfg, read_mtime(lua_path)))
    }

    /// Non-reporting load used when the file is known to exist and parse.
    fn try_load_lua(path: &Path) -> Result<(Self, Option<SystemTime>), String> {
        let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        let cfg = try_parse(&content)?;
        Ok((cfg, read_mtime(path)))
    }

    pub fn load_from(path: &Path) -> (Self, Option<SystemTime>) {
        match std::fs::read_to_string(path) {
            Ok(content) => match try_parse(&content) {
                Ok(cfg) => (cfg, read_mtime(path)),
                Err(_) => (parse(&content), read_mtime(path)),
            },
            Err(_) => (Config::default(), None),
        }
    }
    pub fn load() -> (Self, Option<SystemTime>) {
        let path = config_path();
        if !path.exists() {
            let toml_path = legacy_config_path();
            if toml_path.exists() {
                match Self::migrate_toml(
                    &Config::default(),
                    &toml_path,
                    &path,
                    &config_migrated_path(),
                ) {
                    Ok(loaded) => return loaded,
                    Err(error) => {
                        eprintln!("config: cannot migrate {}: {error}", toml_path.display());
                        note_parse_error("config", &error);
                        return (Config::default(), None);
                    }
                }
            }
            let cfg = Config::default();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match crate::config::format::to_string(&cfg) {
                Ok(text) => {
                    if let Err(error) = std::fs::write(&path, text) {
                        eprintln!("config: cannot write {}: {error}", path.display());
                    }
                }
                Err(error) => eprintln!("config: cannot serialize defaults: {error}"),
            }
            return (cfg, read_mtime(&path));
        }
        if legacy_config_path().exists() {
            eprintln!(
                "config: {} is retired (migrated to config.lua) and ignored; remove it",
                legacy_config_path().display()
            );
        }
        Self::load_from(&path)
    }
    /// Hot-reload poll for `config.lua` (mtime check only, cheap). A broken
    /// edit reports once and keeps the last good config instead of
    /// replacing live state with defaults.
    pub fn poll(known_mtime: &Option<SystemTime>) -> Option<(Self, Option<SystemTime>)> {
        let path = config_path();
        let mtime = read_mtime(&path);
        if mtime != *known_mtime && path.exists() {
            match Self::try_load_lua(&path) {
                Ok((cfg, mtime)) if mtime != *known_mtime => return Some((cfg, mtime)),
                Ok(_) => {}
                Err(error) => {
                    eprintln!("config: parse error, keeping last good config: {error}");
                    note_parse_error("config", &error);
                }
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

    fn parse_lua(source: &str) -> Config {
        try_parse(source).expect("valid test config")
    }

    #[test]
    fn empty_lua_config_uses_defaults_and_roundtrips() {
        let config = parse_lua("return {}");
        assert_eq!(config, Config::default());
        let text = crate::config::format::to_string(&Config::default()).unwrap();
        let reloaded = parse_lua(&text);
        assert_eq!(reloaded, Config::default());
    }

    #[test]
    fn saved_lua_config_uses_snake_case_and_compact_slots() {
        let config = parse_lua(
            "return { bar = { { output = 'DP-1', thickness = 34.0, slot_spacing = 11.0, slots = { { align = 'start', widgets = { 'clock', { name = 'stats', size = 17 } } }, { align = 'end', widgets = {} } } } }, notifications = { timeout_ms = 9000 } }",
        );
        assert_eq!(config.bar[0].slots, 2);
        assert_eq!(config.bar[0].aligns, ["start", "end"]);
        assert_eq!(config.notifications.timeout_ms, 9000);
        let saved = crate::config::format::to_string(&config).unwrap();
        assert!(saved.contains("slot_spacing = 11.0"));
        assert!(saved.contains("timeout_ms = 9000"));
        assert!(!saved.contains("kebab"));
        assert!(!saved.contains("timeout-ms"));
        assert!(!saved.contains("slot_spacing = 4.0"));
        let reloaded = parse_lua(&saved);
        assert_eq!(reloaded, config);
    }

    #[test]
    fn unknown_and_mistyped_lua_fields_fail_loudly() {
        for source in [
            "return { bogus = 1 }",
            "return { bar = 42 }",
            "return { bar = { { margin = { sideways = 8 } } } }",
            "return { bar = { { slots = { { align = 'start', widgets = 4 } } } } } }",
            "return { background = { images = { { position = { 1, 2, 3 } } } } }",
            "return { background = { monitors = { ['DP-1'] = { fit = 'squish' } } } } }",
            "return { theme = { mode = 'bright' } }",
            "return { theme = { icon_theme = '../hax' } }",
            "return { animation = { speed = 'ludicrous' } }",
            "return { interaction = { unknown = 1 } }",
        ] {
            assert!(try_parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn lua_config_computes_values_but_cannot_touch_the_filesystem() {
        let config = parse_lua(
            "local n = 8 * 4 return { bar = { { output = 'DP-' .. 1, thickness = n + 2 } } }",
        );
        assert_eq!(config.bar[0].output, "DP-1");
        assert_eq!(config.bar[0].thickness, 34.0);
        for source in [
            "return { notifications = { timeout_ms = os.execute('true') } }",
            "os.exit(0) return {}",
            "local f = loadfile('x') return {}",
            "let leaked = 1 return leaked",
        ] {
            assert!(try_parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn broken_lua_edits_keep_the_last_good_config() {
        let dir = std::env::temp_dir().join(format!("riced-lua-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.lua");
        std::fs::write(&path, "return { bar = { { thickness = 34.0 } } }").unwrap();
        let (good, stamp) = Config::load_from(&path);
        assert_eq!(good.bar[0].thickness, 34.0);
        assert!(stamp.is_some());
        std::fs::write(&path, "return { bar = nope }").unwrap();
        let (reloaded, _) = Config::load_from(&path);
        assert_eq!(reloaded, Config::default());
        // Broken loads always record what failed; `take_parse_error` feeds
        // the Settings notification pane. The drain is best-effort here
        // because the slot is process-global and shared with other tests.
        if let Some((source, message)) = take_parse_error() {
            assert_eq!(source, "config");
            assert!(
                message.contains("nope") || message.contains("config"),
                "unexpected error: {message}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_toml_migrates_once_to_lua_and_is_consumed() {
        let dir = std::env::temp_dir().join(format!("riced-toml-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lua = dir.join("config.lua");
        let toml = dir.join("config.toml");
        let migrated = dir.join("config.toml.migrated");
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
        std::fs::write(&toml, original).unwrap();
        let cfg = Config::default()
            .migrate_toml(&toml, &lua, &migrated)
            .map(|(cfg, _)| cfg)
            .unwrap();
        assert_eq!(cfg.bar[0].slots, 3);
        assert_eq!(cfg.bar[0].margin_top, -2);
        assert_eq!(cfg.bar[0].radius_top_right, 9.0);
        assert_eq!(cfg.bar[0].slot_spacing, 11.0);
        assert!(!cfg.theme.darkmode);
        assert_eq!(cfg.theme.templates_dir, "off");
        assert_eq!(cfg.notifications.timeout_ms, 1234);
        assert_eq!(cfg.background.monitor[0].output, "DP-2");
        assert_eq!(cfg.background.image[0].x, -291.53717);
        let saved = std::fs::read_to_string(&lua).unwrap();
        assert!(!saved.contains("margin_top"));
        assert!(!saved.contains("slot-spacing"));
        assert!(saved.contains("margin = { left = 3, top = -2 }"));
        assert!(saved.contains("background"));
        assert!(!toml.exists());
        assert_eq!(std::fs::read_to_string(&migrated).unwrap(), original);
        let reloaded = parse_lua(&saved);
        assert_eq!(reloaded, cfg);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn conflicting_legacy_formats_are_rejected_before_persistence() {
        for content in [
            "[[bar]]\nmargin = 8\nmargin_top = 4",
            "[[bar]]\nslots = [{ widgets = ['clock'] }]\nwidgets = [['stats']]",
            "[theme]\nmode = 'dark'\ndarkmode = false",
            "[[background.monitor]]\noutput='DP-1'\n[[background.monitor]]\noutput='DP-1'",
        ] {
            assert!(parse_legacy_toml(content).is_err(), "accepted {content}");
        }
    }

    #[test]
    fn composable_legacy_tables_migrate_to_source_strings() {
        let content = r#"
            [composable.menu]
            width = 180.0
            [composable.context_menu]
            src = "custom-menu.lua"
            width = 178.0
            [composable.context_menu_item]
            src = "custom-item.lua"
            [theme]
            name = "dynamic"
            [[bar]]
            output = "DP-1"
            thickness = 34.0
        "#;
        let cfg = parse_legacy_toml(content).unwrap();
        assert_eq!(cfg.composable.context_menu, "custom-menu.lua");
        assert_eq!(cfg.composable.context_menu_item, "custom-item.lua");
        assert_eq!(cfg.composable.selection_rect, "selection_rect.lua");
        assert_eq!(cfg.theme.name, "dynamic");
        assert_eq!(cfg.bar[0].output, "DP-1");
        assert_eq!(cfg.bar[0].thickness, 34.0);
        let saved = crate::config::format::to_string(&cfg).unwrap();
        assert!(saved.contains("context_menu ="), "{saved}");
        assert!(saved.contains("custom-menu.lua"), "{saved}");
    }

    #[test]
    fn legacy_top_tables_migrate_to_the_bar_array() {
        let cfg = parse_legacy_toml(
            "[top.main]\nanchor = \"bottom\"\noutput = \"DP-1\"\nlength = 80.0\n",
        )
        .unwrap();
        assert_eq!(cfg.bar.len(), 1);
        assert_eq!(cfg.bar[0].anchor, "bottom");
        assert_eq!(cfg.bar[0].output, "DP-1");
        let saved = crate::config::format::to_string(&cfg).unwrap();
        assert!(!saved.contains("top"));
    }
}
