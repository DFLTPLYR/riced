//! First-run installation. Existing projects and user-owned files always win.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};

const VERSION: &str = "1\n";
const COMPLETE: &str = ".frontend-initialized";
const PENDING: &str = ".frontend-installing";
const FILES: &[(&str, &str)] = &[
    ("root.lua", include_str!("../../scripts/frontend/root.lua")),
    (
        "state.lua",
        include_str!("../../scripts/frontend/state.lua"),
    ),
    (
        "layers/background.lua",
        include_str!("../../scripts/frontend/layers/background.lua"),
    ),
    (
        "layers/bar.lua",
        include_str!("../../scripts/frontend/layers/bar.lua"),
    ),
    (
        "layers/notifications.lua",
        include_str!("../../scripts/frontend/layers/notifications.lua"),
    ),
    (
        "data/bars/top.lua",
        include_str!("../../scripts/frontend/data/bars/top.lua"),
    ),
    (
        "plugins/hello.lua",
        include_str!("../../scripts/plugins/hello.lua"),
    ),
    (
        "plugins/counter.lua",
        include_str!("../../scripts/plugins/counter.lua"),
    ),
    // Publish the bootstrap only after its source files are in place.
    (
        "config.lua",
        include_str!("../../scripts/frontend/config.lua"),
    ),
];

/// Automatic startup never repairs deletions. Repair is an explicit request
/// that restores missing files only; it cannot replace existing content.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Startup,
    Repair,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Preserved,
    Installed,
}

/// Called only at the daemon/CLI boundary, never by `Config::load` or tests
/// constructing shell state. An explicit project must not seed the default XDG
/// directory as a side effect.
pub(crate) fn selected(mode: Mode) -> Result<Outcome, String> {
    let path = super::paths::config_path();
    let root = path.parent().ok_or("config path has no parent directory")?;
    initialize_in(root, mode).map_err(|error| format!("{}: {error}", root.display()))
}

fn present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true), // Includes dangling symlinks: they are user-owned too.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn preserved(root: &Path, mode: Mode) -> io::Result<bool> {
    if mode == Mode::Repair {
        return Ok(false);
    }
    if present(&root.join(COMPLETE))? {
        if !present(&root.join("config.lua"))? {
            return Err(io::Error::other(
                "config.lua was removed after initialization; use `riced init --repair` to restore missing defaults",
            ));
        }
        return Ok(true);
    }
    if present(&root.join(PENDING))? {
        return Ok(false);
    }
    // Do not interpret or modify existing (even invalid) Lua or legacy TOML.
    // The existing TOML migration continues to own the latter.
    Ok(present(&root.join("root.lua"))?
        || present(&root.join("config.lua"))?
        || present(&root.join("config.toml"))?)
}

/// Hermetic installation entry: all reads/writes stay under the supplied root.
pub(crate) fn initialize_in(root: &Path, mode: Mode) -> io::Result<Outcome> {
    if mode == Mode::Repair
        && !present(&root.join("config.lua"))?
        && present(&root.join("config.toml"))?
    {
        return Err(io::Error::other(
            "legacy config.toml exists; start riced to migrate it before repairing defaults",
        ));
    }
    if preserved(root, mode)? {
        return Ok(Outcome::Preserved);
    }
    fs::create_dir_all(root)?;
    let lock_path = root.join(".frontend-install.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(io::Error::other("installation lock must not be a symlink"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    lock.lock()?;
    // Another initializer may have finished while this process waited.
    if preserved(root, mode)? {
        return Ok(Outcome::Preserved);
    }
    write_missing(root, PENDING, VERSION)?;
    if fs::read_to_string(root.join(PENDING))? != VERSION {
        return Err(io::Error::other(
            "unrecognized pending frontend version; installation left untouched",
        ));
    }
    for &(path, source) in FILES {
        write_missing(root, path, source)?;
    }
    write_missing(root, COMPLETE, VERSION)?;
    fs::remove_file(root.join(PENDING))?;
    File::open(root)?.sync_all()?;
    Ok(Outcome::Installed)
}

/// Fully write a temporary file, then atomically publish it only if the target
/// is still absent. An interruption leaves either the full file or no file.
fn write_missing(root: &Path, relative: &str, source: &str) -> io::Result<()> {
    let target = root.join(relative);
    if present(&target)? {
        return Ok(());
    }
    let mut parent = root.to_path_buf();
    if let Some(directories) = Path::new(relative).parent() {
        for component in directories.components() {
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(io::Error::other(format!(
                        "{} is not a real directory",
                        parent.display()
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&parent)?,
                Err(error) => return Err(error),
            }
        }
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    temporary.write_all(source.as_bytes())?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&target) {
        Ok(_) => File::open(parent)?.sync_all(),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_gets_the_lua_frontend() {
        let temp = tempfile::tempdir_in("/tmp/opencode").unwrap();
        let root = temp.path().join("riced");
        assert_eq!(
            initialize_in(&root, Mode::Startup).unwrap(),
            Outcome::Installed
        );
        for &(path, source) in FILES {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), source);
        }
        let config =
            super::super::test_parse_config(&fs::read_to_string(root.join("config.lua")).unwrap())
                .unwrap();
        assert!(config.apps.is_empty());
        // The fresh frontend owns every surface from Lua.
        assert!(config.background.disabled && !config.bar_host);
        assert!(config.bar.is_empty());
        assert_eq!(fs::read_to_string(root.join(COMPLETE)).unwrap(), VERSION);
        assert!(!root.join(PENDING).exists());
    }

    #[test]
    fn existing_configs_including_invalid_and_legacy_are_untouched() {
        for (name, contents) in [
            ("config.lua", "broken Lua"),
            ("config.lua", "return {}"),
            ("config.toml", "legacy = true"),
        ] {
            let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
            fs::write(root.path().join(name), contents).unwrap();
            assert_eq!(
                initialize_in(root.path(), Mode::Startup).unwrap(),
                Outcome::Preserved
            );
            assert_eq!(
                fs::read_to_string(root.path().join(name)).unwrap(),
                contents
            );
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn partial_directory_is_adopted_without_overwriting_custom_files() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        fs::create_dir_all(root.path().join("apps")).unwrap();
        fs::write(root.path().join("apps/root.lua"), "return {custom=true}").unwrap();
        fs::write(root.path().join("wallpaper.png"), "user asset").unwrap();
        initialize_in(root.path(), Mode::Startup).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("apps/root.lua")).unwrap(),
            "return {custom=true}"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("wallpaper.png")).unwrap(),
            "user asset"
        );
    }

    #[test]
    fn startup_preserves_deletions_and_explicit_repair_restores_missing_only() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        initialize_in(root.path(), Mode::Startup).unwrap();
        let removed = root.path().join(FILES[1].0);
        fs::remove_file(&removed).unwrap();
        fs::write(root.path().join("root.lua"), "-- custom").unwrap();
        assert_eq!(
            initialize_in(root.path(), Mode::Startup).unwrap(),
            Outcome::Preserved
        );
        assert!(!removed.exists());
        initialize_in(root.path(), Mode::Repair).unwrap();
        assert!(removed.exists());
        assert_eq!(
            fs::read_to_string(root.path().join("root.lua")).unwrap(),
            "-- custom"
        );
        fs::remove_file(root.path().join("config.lua")).unwrap();
        assert!(initialize_in(root.path(), Mode::Startup).is_err());
    }

    #[test]
    fn interrupted_installation_resumes_even_after_bootstrap_is_published() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        write_missing(root.path(), PENDING, VERSION).unwrap();
        write_missing(root.path(), "config.lua", FILES.last().unwrap().1).unwrap();
        assert_eq!(
            initialize_in(root.path(), Mode::Startup).unwrap(),
            Outcome::Installed
        );
        assert!(root.path().join("root.lua").exists());
        assert!(!root.path().join(PENDING).exists());
    }

    #[test]
    fn failed_installation_stays_pending_and_can_resume() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        fs::write(root.path().join("layers"), "not a directory").unwrap();
        assert!(initialize_in(root.path(), Mode::Startup).is_err());
        assert!(!root.path().join(COMPLETE).exists());
        fs::remove_file(root.path().join("layers")).unwrap();
        assert_eq!(
            initialize_in(root.path(), Mode::Startup).unwrap(),
            Outcome::Installed
        );
    }

    #[test]
    fn installation_does_not_follow_symlinked_directories() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        let outside = tempfile::tempdir_in("/tmp/opencode").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("layers")).unwrap();
        assert!(initialize_in(root.path(), Mode::Startup).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_startups_publish_one_complete_installation() {
        let directory = tempfile::tempdir_in("/tmp/opencode").unwrap();
        let root = directory.path().join("riced");
        std::thread::scope(|scope| {
            let first = scope.spawn(|| initialize_in(&root, Mode::Startup).unwrap());
            let second = scope.spawn(|| initialize_in(&root, Mode::Startup).unwrap());
            let results = [first.join().unwrap(), second.join().unwrap()];
            assert_eq!(
                results
                    .iter()
                    .filter(|result| **result == Outcome::Installed)
                    .count(),
                1
            );
        });
        for &(path, source) in FILES {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), source);
        }
        assert!(!root.join(PENDING).exists());
    }

    #[test]
    fn unknown_pending_versions_are_not_completed_or_overwritten() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        fs::write(root.path().join(PENDING), "999\n").unwrap();
        assert!(initialize_in(root.path(), Mode::Startup).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join(PENDING)).unwrap(),
            "999\n"
        );
        assert!(!root.path().join(COMPLETE).exists());
        assert!(!root.path().join("config.lua").exists());
    }

    #[test]
    fn repair_cannot_shadow_a_legacy_toml_configuration() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        fs::write(root.path().join("config.toml"), "legacy = true").unwrap();
        assert!(initialize_in(root.path(), Mode::Repair).is_err());
        assert!(!root.path().join("config.lua").exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
