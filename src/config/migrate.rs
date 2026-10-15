//! One-time legacy conversions; reruns must be no-ops on migrated files.
use super::discovery::lua_files_sorted;
use super::{
    Config, SlotEntry, SlotWidgets, TopConfig, WidgetDef, WidgetDefaults, WidgetPlacement,
    WidgetsFile,
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

pub(super) fn has_legacy_composables(content: &str) -> bool {
    let Ok(value) = toml::from_str::<toml::Value>(content) else {
        return false;
    };
    let Some(table) = value.get("composable").and_then(toml::Value::as_table) else {
        return false;
    };
    table.values().any(toml::Value::is_table)
}

/// Retired tables are converted only at the file-load boundary. Styling is
/// deliberately retired from TOML; the original file is backed up before save.
pub(super) fn migrate_composable_sources(value: &mut toml::Value) {
    let Some(table) = value
        .get_mut("composable")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    for name in ["selection_rect", "context_menu", "context_menu_item"] {
        if let Some(entry) = table.get(name).and_then(toml::Value::as_table) {
            let source = entry.get("src").cloned();
            if let Some(source) = source {
                table.insert(name.into(), source);
            } else {
                table.remove(name);
            }
        }
    }
    table.remove("menu");
}

/// Move legacy `[top.<name>]` entries into `[[bar]]` when no bars exist
/// yet (sorted by name for determinism). The legacy map is cleared so a
/// later save writes only the new format.
pub(super) fn migrate_legacy_top(mut cfg: Config) -> Config {
    if cfg.bar.is_empty() && !cfg.top.is_empty() {
        let mut names: Vec<_> = cfg.top.keys().cloned().collect();
        names.sort();
        cfg.bar = names
            .into_iter()
            .filter_map(|k| cfg.top.remove(&k))
            .collect();
    } else {
        cfg.top.clear();
    }
    cfg
}
#[cfg(test)]
mod tests {
    use super::super::Config;
    use super::super::io::parse;
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
    fn composable_migration_preserves_sources_and_backs_up_retired_props_once() {
        let dir =
            std::env::temp_dir().join(format!("riced-context-migration-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let content = r#"
            [composable.menu]
            width = 180.0
            height = 92.0
            padding = 0.0
            spacing = 0.0
            rounding = 0.0
            [composable.context_menu]
            src = "custom-menu.lua"
            width = 178.0
            padding = 4.0
            spacing = 5.0
            rounding = 3.0
            [composable.context_menu_item]
            src = "custom-item.lua"
            padding = 5.0
            rounding = 3.0
            [composable.selection_rect]
            src = "custom-selection.lua"
            [composable.selection_rect.props]
            radius = 4.0
            [theme]
            name = "dynamic"
            [[bar]]
            output = "DP-1"
            thickness = 34.0
        "#;
        std::fs::write(&path, content).unwrap();
        let (cfg, stamp) = Config::load_from(&path);
        assert_eq!(cfg.composable.context_menu, "custom-menu.lua");
        assert_eq!(cfg.composable.context_menu_item, "custom-item.lua");
        assert_eq!(cfg.composable.selection_rect, "custom-selection.lua");
        assert_eq!(cfg.theme.name, "dynamic");
        assert_eq!(cfg.bar[0].output, "DP-1");
        assert_eq!(cfg.bar[0].thickness, 34.0);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!has_legacy_composables(&saved));
        assert!(!saved.contains("[composable."));
        assert!(saved.contains("context_menu = \"custom-menu.lua\""));
        assert_eq!(
            std::fs::read_to_string(path.with_extension("toml.pre-composable-sources")).unwrap(),
            content
        );
        let (_, reloaded_stamp) = Config::load_from(&path);
        assert_eq!(stamp, reloaded_stamp);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retired_style_only_tables_use_default_sources() {
        let cfg =
            parse("[composable.menu]\nwidth = 200.0\n[composable.context_menu]\npadding = 12.0\n");
        assert_eq!(cfg.composable, super::super::ComposableConfig::default());
        assert!(!toml::to_string_pretty(&cfg).unwrap().contains("composable"));
    }

    #[test]
    fn invalid_legacy_source_does_not_rewrite_the_file() {
        let dir =
            std::env::temp_dir().join(format!("riced-invalid-composable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let content = "[composable.context_menu]\nsrc = 42\n";
        std::fs::write(&path, content).unwrap();
        Config::load_from(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!path.with_extension("toml.pre-composable-sources").exists());
        std::fs::remove_dir_all(dir).unwrap();
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
    fn legacy_top_tables_migrate_to_bar_array() {
        // NOTE: migration runs in `parse()` (file loads), not on raw
        // `toml::from_str` — the file path is what matters in prod.
        let cfg: Config =
            parse("[top.main]\nanchor = \"bottom\"\noutput = \"DP-1\"\nlength = 80.0\n");
        assert_eq!(cfg.bar.len(), 1);
        assert_eq!(cfg.bar[0].anchor, "bottom");
        assert_eq!(cfg.bar[0].output, "DP-1");
        // Legacy map is consumed, so a re-save writes only [[bar]].
        assert!(cfg.top.is_empty());
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("[[bar]]"));
        assert!(!text.contains("[top."));
    }
}
