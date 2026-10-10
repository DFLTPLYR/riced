//! Explicit VM profiles. Native widget shell access is intentionally preserved.
use mlua::{AnyUserData, HookTriggers, Lua, LuaOptions, StdLib, Table, UserData, Value, VmState};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const INSTRUCTION_LIMIT: usize = 200_000;
const HOOK_INTERVAL: usize = 1_000;
struct ExecutionBudget(Arc<AtomicUsize>);
impl UserData for ExecutionBudget {}

pub(crate) fn reset_budget(lua: &Lua) -> mlua::Result<()> {
    let budget: AnyUserData = lua.named_registry_value("riced.execution_budget")?;
    budget
        .borrow::<ExecutionBudget>()?
        .0
        .store(INSTRUCTION_LIMIT, Ordering::Relaxed);
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Profile {
    App,
    Widget,
}
pub(crate) fn new_lua(profile: Profile) -> mlua::Result<Lua> {
    let libs = match profile {
        Profile::App => StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        Profile::Widget => StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::OS | StdLib::IO,
    };
    let lua = Lua::new_with(libs, LuaOptions::default())?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    let remaining = Arc::new(AtomicUsize::new(INSTRUCTION_LIMIT));
    lua.set_named_registry_value("riced.execution_budget", ExecutionBudget(remaining.clone()))?;
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL as u32),
        move |_, _| {
            if remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(HOOK_INTERVAL)
                })
                .is_err()
            {
                Err(mlua::Error::RuntimeError(
                    "Lua instruction budget exceeded".into(),
                ))
            } else {
                Ok(VmState::Continue)
            }
        },
    )?;
    if matches!(profile, Profile::Widget) {
        let globals = lua.globals();
        for key in ["dofile", "loadfile", "require"] {
            globals.set(key, Value::Nil)?;
        }
        let os: Table = globals.get("os")?;
        for key in ["exit", "remove", "rename", "setlocale"] {
            os.set(key, Value::Nil)?;
        }
    }
    Ok(lua)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_sandbox_blocks_escapes_but_keeps_time() {
        let lua = new_lua(Profile::Widget).unwrap();
        assert!(matches!(
            lua.load("return os.execute").eval::<mlua::Value>().unwrap(),
            mlua::Value::Function(_)
        ));
        for key in ["exit", "remove", "rename"] {
            assert!(
                matches!(
                    lua.load(format!("return os.{key}"))
                        .eval::<mlua::Value>()
                        .unwrap(),
                    mlua::Value::Nil
                ),
                "{key} blocked"
            );
        }
        assert!(matches!(
            lua.load("return require").eval::<mlua::Value>().unwrap(),
            mlua::Value::Nil
        ));
        assert_eq!(
            lua.load("return os.date('%H')")
                .eval::<String>()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn os_execute_is_native_shell() {
        let lua = new_lua(Profile::Widget).unwrap();
        assert!(
            lua.load("return os.execute('true')")
                .eval::<bool>()
                .unwrap()
        );
        let output: String = lua
            .load(r#"local h=io.popen('echo hi'); local s=h:read('*a'); h:close(); return s"#)
            .eval()
            .unwrap();
        assert_eq!(output.trim(), "hi");
        let output: String = lua
            .load(
                r#"local h=io.popen('echo a; echo b'); local s=h:read('*a'); h:close(); return s"#,
            )
            .eval()
            .unwrap();
        assert!(output.contains('a') && output.contains('b'));
    }

    #[test]
    fn execution_budget_is_shared_and_recovers_for_both_profiles() {
        for profile in [Profile::App, Profile::Widget] {
            let lua = new_lua(profile).unwrap();
            let error = lua.load("while true do end").exec().unwrap_err();
            assert!(error.to_string().contains("instruction budget exceeded"));
            reset_budget(&lua).unwrap();
            assert_eq!(lua.load("return 42").eval::<i32>().unwrap(), 42);
        }
    }

    #[test]
    fn native_widget_shell_access_is_preserved_under_the_shared_budget() {
        let lua = new_lua(Profile::Widget).unwrap();
        assert!(
            lua.load("return type(os.execute) == 'function' and type(io.popen) == 'function'")
                .eval::<bool>()
                .unwrap()
        );
        assert!(
            lua.load("return os.exit == nil and os.remove == nil and require == nil")
                .eval::<bool>()
                .unwrap()
        );
        let error = lua
            .load("return string.rep('x', 128 * 1024 * 1024)")
            .eval::<String>()
            .unwrap_err();
        assert!(error.to_string().to_lowercase().contains("memory"));
    }
}
