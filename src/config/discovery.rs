//! Widget/component discovery and change stamps; no schema, no persistence.
use super::paths::{components_dir, widgets_dir};
use super::seed::seed_marker;
use super::seed::{
    SEED_COMPONENT_CARD, SEED_COMPONENT_DEFINE, SEED_COMPONENT_MENU, SEED_COMPONENT_STYLED,
};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Discovered widget files: `(name, path)` with name = file stem,
/// sorted by name. Non-recursive (so `components/` never becomes
/// widgets), `*.lua` only, dotfiles skipped.
pub fn discover_widget_files() -> Vec<(String, PathBuf)> {
    discover_widget_files_in(&widgets_dir())
}

pub(crate) fn discover_widget_files_in(dir: &std::path::Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().is_none_or(|e| e != "lua") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        out.push((stem.to_string(), path));
    }
    out.sort();
    out
}

/// Newest mtime of the widgets dir itself (add/remove/rename stamp),
/// or `None` when unreadable. Content edits are covered by per-file
/// mtimes; this only drives definition rescan.
pub fn widgets_dir_mtime() -> Option<SystemTime> {
    std::fs::metadata(widgets_dir())
        .and_then(|m| m.modified())
        .ok()
}

/// Concatenated `components/*.lua` source (sorted by filename, tagged
/// with `-- file:` separators for error lines), or `None` when the
/// dir is missing/empty. Tested via [`components_source_in`].
/// Preserve individual chunk names so Lua errors name their source file.
pub(crate) fn component_files() -> Vec<(PathBuf, String)> {
    component_files_in(&components_dir())
}

pub(super) fn component_files_in(dir: &std::path::Path) -> Vec<(PathBuf, String)> {
    let installed = shared_component_files_in(dir);
    if installed.is_empty() && !dir.exists() && !seed_marker(dir).exists() {
        builtin_component_files()
    } else {
        installed
    }
}

pub(super) fn shared_component_files_in(dir: &std::path::Path) -> Vec<(PathBuf, String)> {
    installed_component_paths(dir)
        .into_iter()
        .filter_map(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .filter(|source| !source.trim_start().starts_with("-- riced:composable"))
                .map(|source| (path, source))
        })
        .collect()
}

/// Unnumbered user copies override the corresponding bundled seed file.
/// Keep both files on disk, but execute only one definition of each builder.
pub(super) fn installed_component_paths(dir: &std::path::Path) -> Vec<PathBuf> {
    lua_files_sorted(dir)
        .into_iter()
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str());
            !component_seed_aliases()
                .iter()
                .any(|(seed, alias)| name == Some(*seed) && dir.join(alias).is_file())
        })
        .collect()
}

pub(super) fn component_seed_aliases() -> [(&'static str, &'static str); 4] {
    [
        ("00-define.lua", "define.lua"),
        ("05-styled.lua", "styled.lua"),
        ("10-card.lua", "card.lua"),
        ("20-menu.lua", "menu.lua"),
    ]
}

/// Bundled pure components used when no on-disk library has been installed.
pub(crate) fn builtin_component_files() -> Vec<(PathBuf, String)> {
    [
        ("00-define.lua", SEED_COMPONENT_DEFINE),
        ("05-styled.lua", SEED_COMPONENT_STYLED),
        ("10-card.lua", SEED_COMPONENT_CARD),
        ("20-menu.lua", SEED_COMPONENT_MENU),
    ]
    .into_iter()
    .map(|(name, source)| {
        (
            PathBuf::from("scripts/components").join(name),
            source.to_owned(),
        )
    })
    .collect()
}

/// Newest mtime across `components/*.lua` (folder hot-reload stamp),
/// or `None` when the dir is missing/empty.
pub fn components_mtime() -> Option<SystemTime> {
    components_mtime_in(&components_dir())
}

pub(super) fn lua_files_sorted(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .collect();
    files.sort();
    files
}

fn components_mtime_in(dir: &std::path::Path) -> Option<SystemTime> {
    use std::hash::{Hash, Hasher};
    let files = lua_files_sorted(dir);
    if files.is_empty() {
        return None;
    }
    let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
    for path in files {
        path.hash(&mut fingerprint);
        std::fs::read(&path).ok().hash(&mut fingerprint);
    }
    // Opaque change stamp, rather than the newest file's mtime: detects
    // edits to older files, additions, removals, and timestamp-preserving saves.
    Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(fingerprint.finish()))
}

/// Hot-reload check for the components dir: fresh stamp when any
/// `*.lua` changed (or appeared) since `known`. Never fails.
pub fn poll_components(known_mtime: &Option<SystemTime>) -> Option<Option<SystemTime>> {
    let mtime = components_mtime();
    if mtime != *known_mtime {
        return Some(mtime);
    }
    None
}
#[cfg(test)]
fn components_source_in(dir: &std::path::Path) -> Option<String> {
    let files = lua_files_sorted(dir);
    if files.is_empty() {
        return None;
    }
    let mut out = String::new();
    for path in &files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(path) {
            Ok(content) => {
                out.push_str(&format!("-- file: {name}\n{content}"));
                if !content.ends_with('\n') {
                    out.push('\n');
                }
            }
            Err(e) => eprintln!("components: cannot read {}: {e}", path.display()),
        }
    }
    if out.is_empty() { None } else { Some(out) }
}
#[cfg(test)]
mod tests {
    use super::super::seed::seed_components_in;
    use super::*;

    #[test]
    fn discovery_skips_non_widgets() {
        let dir = std::env::temp_dir().join(format!("riced-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("components")).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for name in ["clock.lua", "stats.lua", ".hidden.lua", "README.md"] {
            std::fs::write(dir.join(name), "-- x").unwrap();
        }
        std::fs::write(dir.join("components").join("card.lua"), "-- x").unwrap();
        std::fs::write(dir.join("sub").join("y.lua"), "-- x").unwrap();
        let found = discover_widget_files_in(&dir);
        assert_eq!(
            found.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            ["clock", "stats"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_components_are_not_executed_as_shared_library_definitions() {
        let dir = std::env::temp_dir().join(format!("riced-chrome-library-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("app.lua"),
            "-- riced:composable\nerror('only the host should load me')",
        )
        .unwrap();
        std::fs::write(
            dir.join("builder.lua"),
            "ui.define('example', function(props) return ui.text('ok') end)",
        )
        .unwrap();
        let files = shared_component_files_in(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, dir.join("builder.lua"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn component_aliases_replace_seeds_without_duplicate_loading() {
        let dir =
            std::env::temp_dir().join(format!("riced-components-aliases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (_, alias) in component_seed_aliases() {
            std::fs::write(dir.join(alias), "-- user component").unwrap();
        }
        seed_components_in(&dir);
        assert_eq!(lua_files_sorted(&dir).len(), 7);
        // An earlier release may already have seeded the numbered copies.
        for (seed, _) in component_seed_aliases() {
            std::fs::write(dir.join(seed), "-- seed component").unwrap();
        }
        let paths = installed_component_paths(&dir);
        assert_eq!(paths.len(), 7);
        assert!(paths.iter().all(|path| {
            !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(|c: char| c.is_ascii_digit())
        }));
        assert_eq!(lua_files_sorted(&dir).len(), 11);
        // Removing an override makes its seed available again.
        std::fs::remove_file(dir.join("styled.lua")).unwrap();
        assert!(installed_component_paths(&dir).contains(&dir.join("05-styled.lua")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn components_source_concatenates_sorted_lua() {
        let dir = std::env::temp_dir().join(format!("riced-components-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Missing/empty dir reads as no library.
        assert!(components_source_in(&dir).is_none());
        assert!(components_mtime_in(&dir).is_none());
        std::fs::create_dir_all(&dir).unwrap();
        assert!(components_source_in(&dir).is_none());
        std::fs::write(dir.join("20-b.lua"), "b = 2\n").unwrap();
        std::fs::write(dir.join("10-a.lua"), "a = 1\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored\n").unwrap();
        let source = components_source_in(&dir).expect("source");
        // Sorted: a before b; non-lua files skipped; file tags present.
        assert!(
            source.find("-- file: 10-a.lua").unwrap() < source.find("-- file: 20-b.lua").unwrap()
        );
        assert!(!source.contains("ignored"));
        assert!(components_mtime_in(&dir).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn components_stamp_detects_content_changes_and_removals() {
        let dir =
            std::env::temp_dir().join(format!("riced-components-mtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(components_mtime_in(&dir).is_none());
        std::fs::create_dir_all(&dir).unwrap();
        assert!(components_mtime_in(&dir).is_none());
        std::fs::write(dir.join("10-a.lua"), "a = 1\n").unwrap();
        assert!(components_mtime_in(&dir).is_some());
        std::fs::write(dir.join("20-b.lua"), "b = 1\n").unwrap();
        let before = components_mtime_in(&dir);
        std::fs::write(dir.join("10-a.lua"), "a = 2\n").unwrap();
        let changed = components_mtime_in(&dir);
        assert_ne!(before, changed);
        assert_eq!(changed, components_mtime_in(&dir));
        std::fs::remove_file(dir.join("10-a.lua")).unwrap();
        assert_ne!(changed, components_mtime_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
