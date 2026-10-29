//! Move legacy project paths without replacing user files or initialization history.
use std::{fs, io, path::Path};

const MOVES: &[(&str, &str)] = &[
    ("state.lua", "data/state.lua"),
    ("settings", "components/settings"),
    ("settings.lua", "components/settings/window.lua"),
    ("shaders", "components/shaders"),
    ("layers", "components/layers"),
    ("theme", "data/theme"),
    ("templates", "data/templates"),
    ("plugins", "data/plugins"),
];

pub(crate) fn directory(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(io::Error::other(format!("{} is not a real directory", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => directory(path),
                result => result,
            }
        }
        Err(error) => Err(error),
    }
}

fn move_missing(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    match fs::symlink_metadata(target) {
        Ok(existing) if metadata.is_dir() && existing.is_dir() => {
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                move_missing(&entry.path(), &target.join(entry.file_name()))?;
            }
            fs::remove_dir(source)
        }
        Ok(existing) if metadata.is_file() && existing.is_file() && fs::read(source)? == fs::read(target)? => {
            fs::remove_file(source)
        }
        Ok(_) => Err(io::Error::other(format!(
            "layout migration conflict: {} and {}; both kept", source.display(), target.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = target.parent() {
                directory(parent)?;
            }
            fs::rename(source, target)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn migrate(root: &Path) -> io::Result<()> {
    if !root.exists() { return Ok(()); }
    let state = root.join("data/state");
    directory(&state)?;
    let lock_path = state.join("layout.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(io::Error::other("layout lock must not be a symlink"));
    }
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(lock_path)?;
    lock.lock()?;
    // History moves first so a deleted library is never mistaken for a fresh install.
    for entry in fs::read_dir(root)?.chain(fs::read_dir(root.join("data"))?) {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let plain = name.trim_start_matches('.');
        let target = if let Some(name) = plain.strip_suffix("-initialized") {
            Some(format!("{name}.initialize"))
        } else if plain == "frontend-installing" {
            Some("frontend.initialize.pending".into())
        } else if plain.contains(".lock") {
            Some(plain.to_owned())
        } else { None };
        if let Some(target) = target {
            move_missing(&entry.path(), &state.join(target))?;
        }
    }
    for (old, new) in MOVES {
        move_missing(&root.join(old), &root.join(new))?;
    }
    for name in ["selection_rect.lua", "context_menu.lua", "context_menu_item.lua", "notifications.lua", "notification_card.lua"] {
        move_missing(&root.join("components").join(name), &root.join("composables").join(name))?;
    }
    if state.join("components.initialize").exists() && !state.join("composables.initialize").exists() {
        fs::OpenOptions::new().write(true).create_new(true).open(state.join("composables.initialize"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_preserves_edits_history_and_deleted_libraries() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        fs::write(root.path().join(".shaders-initialized"), "history").unwrap();
        fs::write(root.path().join("state.lua"), "-- custom").unwrap();
        fs::create_dir(root.path().join("theme")).unwrap();
        fs::write(root.path().join("theme/custom.json"), "custom").unwrap();
        migrate(root.path()).unwrap();
        migrate(root.path()).unwrap();
        assert_eq!(fs::read_to_string(root.path().join("data/state.lua")).unwrap(), "-- custom");
        assert_eq!(fs::read_to_string(root.path().join("data/state/shaders.initialize")).unwrap(), "history");
        assert!(!root.path().join("components/shaders").exists());
        assert_eq!(fs::read_to_string(root.path().join("data/theme/custom.json")).unwrap(), "custom");
    }

    #[test]
    fn migration_refuses_collisions_and_symlinked_destinations() {
        let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
        fs::create_dir(root.path().join("data")).unwrap();
        fs::write(root.path().join("state.lua"), "old").unwrap();
        fs::write(root.path().join("data/state.lua"), "new").unwrap();
        assert!(migrate(root.path()).is_err());
        assert_eq!(fs::read_to_string(root.path().join("state.lua")).unwrap(), "old");
        assert_eq!(fs::read_to_string(root.path().join("data/state.lua")).unwrap(), "new");
        let other = tempfile::tempdir_in("/tmp/opencode").unwrap();
        let outside = tempfile::tempdir_in("/tmp/opencode").unwrap();
        std::os::unix::fs::symlink(outside.path(), other.path().join("data")).unwrap();
        assert!(migrate(other.path()).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
