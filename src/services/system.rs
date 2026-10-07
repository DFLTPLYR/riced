//! `system` service: CPU/memory plus GPU usage. Moved verbatim from
//! the old `sysinfo`/`gfxinfo` globals (which are retired).

use super::registry::ServiceCtx;

/// Publish `system = { cpu_usage, cpu_count, mem_used, mem_total,
/// mem_usage, gpu_usage }`. `gpu_usage` is nil when the GPU exposes
/// nothing readable (same degrade stance as before).
pub fn publish(ctx: &ServiceCtx, lua: &mlua::Lua) -> mlua::Result<()> {
    let sys = ctx.sys;
    let table = lua.create_table()?;
    table.set("cpu_usage", sys.global_cpu_usage())?;
    table.set("cpu_count", sys.cpus().len())?;
    table.set("mem_used", sys.used_memory())?;
    table.set("mem_total", sys.total_memory())?;
    let total = sys.total_memory();
    table.set(
        "mem_usage",
        if total > 0 {
            sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        },
    )?;
    match ctx.gpu {
        Some(usage) => table.set("gpu_usage", usage)?,
        None => table.set("gpu_usage", mlua::Value::Nil)?,
    }
    lua.globals().set("system", table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_table_carries_cpu_mem_and_optional_gpu() {
        let lua = mlua::Lua::new();
        let sys = sysinfo::System::new();
        let theme = crate::config::ThemeConfig::default();
        let outputs = std::collections::HashMap::new();
        let queue = std::collections::VecDeque::new();
        let toplevels = super::super::ToplevelCache::default();
        let ctx = ServiceCtx {
            sys: &sys,
            gpu: Some(42.0),
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
        };
        publish(&ctx, &lua).expect("publish");
        let cpu: f32 = lua.load("return system.cpu_usage").eval().expect("cpu");
        assert!(cpu >= 0.0);
        let gpu: f32 = lua.load("return system.gpu_usage").eval().expect("gpu");
        assert_eq!(gpu, 42.0);
        let ctx_none = ServiceCtx { gpu: None, ..ctx };
        publish(&ctx_none, &lua).expect("publish");
        let nil: mlua::Value = lua.load("return system.gpu_usage").eval().expect("nil");
        assert!(matches!(nil, mlua::Value::Nil));
    }
}
