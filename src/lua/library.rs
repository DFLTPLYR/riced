//! One source-library validator and last-good cache, scoped by VM profile.
use super::sandbox::{self, Profile};
use crate::ui::dsl::inject_ui_base;
use mlua::Lua;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

pub(crate) type Library = Vec<(PathBuf, String)>;
#[derive(Default)]
struct Revision {
    good: Library,
    rejected: Option<Library>,
}
#[derive(Default)]
struct Registry {
    profiles: HashMap<Profile, Revision>,
}

impl Registry {
    fn select(&mut self, profile: Profile, candidate: Library) -> mlua::Result<Library> {
        let revision = self.profiles.entry(profile).or_default();
        if candidate == revision.good {
            return Ok(candidate);
        }
        if revision.rejected.as_ref() == Some(&candidate) {
            return Ok(revision.good.clone());
        }
        let trial = sandbox::new_lua(profile)?;
        inject_ui_base(&trial)?;
        match execute(&trial, &candidate) {
            Ok(()) => {
                revision.rejected = None;
                revision.good = candidate.clone();
                Ok(candidate)
            }
            Err(error) => {
                revision.rejected = Some(candidate);
                eprintln!("components: keeping last working library: {error}");
                Ok(revision.good.clone())
            }
        }
    }
}

fn execute(lua: &Lua, files: &Library) -> mlua::Result<()> {
    for (path, source) in files {
        sandbox::reset_budget(lua)?;
        lua.load(source)
            .set_name(format!("@{}", path.display()))
            .exec()?;
    }
    Ok(())
}

pub(crate) fn install(lua: &Lua, profile: Profile, candidate: Library) -> mlua::Result<()> {
    static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));
    let selected = REGISTRY
        .lock()
        .expect("component library registry")
        .select(profile, candidate)?;
    execute(lua, &selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_keeps_last_good_rejects_runaway_code_and_recovers() {
        let mut registry = Registry::default();
        let good = vec![(
            PathBuf::from("builder.lua"),
            "ui.define('test', function(p) return ui.text(p.text) end)".into(),
        )];
        assert_eq!(
            registry.select(Profile::Widget, good.clone()).unwrap(),
            good
        );
        let runaway = vec![(PathBuf::from("builder.lua"), "while true do end".into())];
        assert_eq!(
            registry.select(Profile::Widget, runaway.clone()).unwrap(),
            good
        );
        assert_eq!(registry.select(Profile::Widget, runaway).unwrap(), good);
        let repaired = vec![(
            PathBuf::from("builder.lua"),
            "ui.define('test', function(p) return ui.text('repaired') end)".into(),
        )];
        assert_eq!(
            registry.select(Profile::Widget, repaired.clone()).unwrap(),
            repaired
        );
    }
}
