//! One-time legacy conversions; reruns must be no-ops on migrated files.
use super::discovery::lua_files_sorted;
use super::{
    SlotEntry, SlotWidgets, TopConfig, WidgetDef, WidgetDefaults, WidgetPlacement, WidgetsFile,
};
use std::collections::HashMap;

/// Move legacy library files, preserving existing root-level overrides.
pub(super) fn migrate_components_in(legacy: &std::path::Path, target: &std::path::Path) {
    let files = lua_files_sorted(legacy);
    if files.is_empty() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(target) {
        eprintln!("components: cannot create {}: {e}", target.display());
        return;
    }
    for path in files {
        let destination = target.join(path.file_name().expect("Lua file name"));
        if destination.exists() {
            continue;
        }
        if let Err(e) = std::fs::rename(&path, &destination) {
            eprintln!("components: cannot migrate {}: {e}", path.display());
        }
    }
    // Only removes an empty directory; conflicting files remain available.
    let _ = std::fs::remove_dir(legacy);
}
impl WidgetsFile {
    /// Parse retired `widgets.toml` content for the one-time
    /// migration (the caller logs context).
    pub(crate) fn parse(content: &str) -> Result<Vec<WidgetDef>, String> {
        toml::from_str::<WidgetsFile>(content)
            .map(|file| file.widget)
            .map_err(|e| e.to_string())
    }

    /// Fold retired defs into bar placements (pure; the caller persists
    /// the bars and renames the file). For every slot entry naming a
    /// migrated widget, stamp `interval`/`size`/`file` overrides that
    /// differ from the widget's Lua defaults (builtin fallbacks when
    /// its file is missing). Returns the stamped placement count.
    /// Bare names stay bare when nothing differs, so clean configs
    /// migrate to clean configs.
    pub(crate) fn migrate_placements(
        bars: &mut [TopConfig],
        old_defs: &[WidgetDef],
        lua_defaults: &HashMap<String, WidgetDefaults>,
    ) -> usize {
        let old_by_name: HashMap<&str, &WidgetDef> = old_defs
            .iter()
            .map(|def| (def.name.as_str(), def))
            .collect();
        let mut stamped = 0;
        for bar in bars.iter_mut() {
            for slot in bar.widgets.iter_mut() {
                match slot {
                    SlotWidgets::One(name) => {
                        if let Some(full) = Self::stamp_placement(name, &old_by_name, lua_defaults)
                        {
                            *slot = SlotWidgets::Many(vec![SlotEntry::Full(full)]);
                            stamped += 1;
                        }
                    }
                    SlotWidgets::Many(entries) => {
                        for entry in entries.iter_mut() {
                            if let SlotEntry::Name(name) = entry
                                && let Some(full) =
                                    Self::stamp_placement(name, &old_by_name, lua_defaults)
                            {
                                *entry = SlotEntry::Full(full);
                                stamped += 1;
                            }
                        }
                    }
                }
            }
        }
        stamped
    }

    /// Override placement for one legacy name, or `None` when the old
    /// def matches the Lua defaults (clean inherit, nothing stamped).
    /// When in doubt we preserve: absent TOML values read as the
    /// built-in fallbacks, so an explicit `interval = 1.0` against a
    /// Lua `interval = 2.0` still stamps (the old behavior wins).
    fn stamp_placement(
        name: &str,
        old_by_name: &HashMap<&str, &WidgetDef>,
        lua_defaults: &HashMap<String, WidgetDefaults>,
    ) -> Option<WidgetPlacement> {
        let old = old_by_name.get(name)?;
        let defaults = lua_defaults.get(name).cloned().unwrap_or_default();
        let mut placement = WidgetPlacement {
            name: name.to_string(),
            ..Default::default()
        };
        let mut touched = false;
        if (old.interval - defaults.interval).abs() > f32::EPSILON {
            placement.interval = Some(old.interval);
            touched = true;
        }
        if (old.size - defaults.size).abs() > f32::EPSILON {
            placement.size = Some(old.size);
            touched = true;
        }
        let default_file = format!("{name}.lua");
        if !old.file.trim().is_empty() && old.file.trim() != default_file {
            placement.file = Some(old.file.clone());
            touched = true;
        }
        touched.then_some(placement)
    }
}

/// Retired tables are converted only by the one-time TOML → Lua
/// migration; the daemon never reads TOML after migration.
pub(super) fn migrate_composable_sources(value: &mut toml::Value) -> Result<(), String> {
    let Some(table) = value
        .get_mut("composable")
        .and_then(toml::Value::as_table_mut)
    else {
        return Ok(());
    };
    // Flatten `[composable.<name>]` source tables into plain kebab keys
    // (`context-menu`, ...); `format_migration` normalizes those to the
    // snake_case keys `Config` parses. New-style plain `context_menu`
    // strings pass through and merge below. `table.get` + `remove` below
    // avoids a borrow conflict when reading and writing entries.
    for name in ["selection-rect", "context-menu", "context-menu-item"] {
        let snake = name.replace('-', "_");
        // Handle both spellings: kebab tables flatten to the kebab key,
        // plain snake strings merge upward, then `format_migration`
        // converges everything on snake_case.
        let flattened = if let Some(entry) = table.get(name).and_then(toml::Value::as_table) {
            entry.get("src").cloned()
        } else {
            table
                .get(&snake)
                .and_then(toml::Value::as_table)
                .and_then(|entry| entry.get("src").cloned())
        };
        let plain = table.get(&snake).filter(|value| !value.is_table()).cloned();
        table.remove(name);
        table.remove(&snake);
        match (flattened, plain) {
            (Some(source), None) if source.is_str() => {
                table.insert(name.into(), source);
            }
            (Some(_), None) => {
                return Err(format!("composable.{name}.src must be a string"));
            }
            (Some(_), Some(_)) => {
                return Err(format!("both composable table and {snake} are configured"));
            }
            (None, Some(source)) => {
                table.insert(name.into(), source);
                table.remove(&snake);
            }
            (None, None) => {}
        }
    }
    table.remove("menu");
    // Any leftover `[composable.<name>]` tables here are unknown entries
    // (mistyped names); `Config` strictness rejects them after migration.
    Ok(())
}

/// Move legacy `[top.<name>]` entries into `[[bar]]` when no bars exist
/// yet (sorted by name for determinism). Used only by the one-time
/// TOML → Lua migration; normal loads never see `top`.
pub(super) fn migrate_legacy_top_table(
    bars: &mut Vec<TopConfig>,
    top: &mut HashMap<String, TopConfig>,
) {
    if bars.is_empty() && !top.is_empty() {
        let mut names: Vec<_> = top.keys().cloned().collect();
        names.sort();
        *bars = names.into_iter().filter_map(|k| top.remove(&k)).collect();
    } else {
        top.clear();
    }
}
#[cfg(test)]
mod tests {
    use super::super::TopConfig;
    use super::super::seed::seed_components_in;
    use super::*;

    #[test]
    fn migration_stamps_only_differences() {
        use std::collections::HashMap as Map;
        let old = |name: &str, interval: f32, size: f32| WidgetDef {
            name: name.to_string(),
            file: String::new(),
            interval,
            size,
        };
        let old_defs = vec![old("clock", 1.0, 13.0), old("stats", 5.0, 13.0)];
        // Lua declares clock at 2s; stats matches its old values.
        let mut lua_defaults = Map::new();
        lua_defaults.insert(
            "clock".to_string(),
            WidgetDefaults {
                interval: 2.0,
                ..Default::default()
            },
        );
        lua_defaults.insert(
            "stats".to_string(),
            WidgetDefaults {
                interval: 5.0,
                ..Default::default()
            },
        );
        let mut bars = vec![TopConfig {
            widgets: vec![SlotWidgets::Many(vec![
                SlotEntry::Name("clock".to_string()),
                SlotEntry::Name("stats".to_string()),
            ])],
            ..Default::default()
        }];
        let stamped = WidgetsFile::migrate_placements(&mut bars, &old_defs, &lua_defaults);
        // clock's old 1.0 differs from Lua's 2.0 → stamped; stats is
        // clean → stays a bare name.
        assert_eq!(stamped, 1);
        let SlotWidgets::Many(entries) = &bars[0].widgets[0] else {
            panic!("expected many");
        };
        match &entries[0] {
            SlotEntry::Full(p) => assert_eq!(p.interval, Some(1.0)),
            _ => panic!("clock should stamp"),
        }
        assert!(matches!(entries[1], SlotEntry::Name(_)));
    }

    #[test]
    fn legacy_top_tables_move_into_the_bar_array() {
        // `top` is migration-only: the runtime `Config` no longer parses it.
        let mut bars = vec![TopConfig {
            output: "DP-1".into(),
            ..Default::default()
        }];
        let mut top: HashMap<String, TopConfig> = HashMap::new();
        migrate_legacy_top_table(&mut bars, &mut top);
        assert_eq!(bars.len(), 1);
        let mut bars = Vec::new();
        top.insert(
            "main".into(),
            TopConfig {
                output: "DP-1".into(),
                ..Default::default()
            },
        );
        migrate_legacy_top_table(&mut bars, &mut top);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].output, "DP-1");
        assert!(top.is_empty());
    }

    #[test]
    fn components_migration_preserves_root_overrides() {
        let dir =
            std::env::temp_dir().join(format!("riced-components-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let legacy = dir.join("widgets/components");
        let target = dir.join("components");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(legacy.join("custom.lua"), "-- custom").unwrap();
        std::fs::write(legacy.join("card.lua"), "-- legacy").unwrap();
        std::fs::write(target.join("card.lua"), "-- root").unwrap();
        migrate_components_in(&legacy, &target);
        assert_eq!(
            std::fs::read_to_string(target.join("custom.lua")).unwrap(),
            "-- custom"
        );
        assert!(!legacy.join("custom.lua").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("card.lua")).unwrap(),
            "-- root"
        );
        assert!(legacy.join("card.lua").exists());
        migrate_components_in(&legacy, &target);
        seed_components_in(&target);
        assert!(target.join("05-styled.lua").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_lua_config_mentions_bars_without_legacy_top() {
        let mut config = super::super::Config::default();
        config.bar.push(TopConfig {
            anchor: "bottom".into(),
            output: "DP-1".into(),
            length: 80.0,
            ..Default::default()
        });
        let text = crate::config::format::to_string(&config).unwrap();
        assert!(text.contains("bar"));
        assert!(!text.contains("top"));
    }
}
