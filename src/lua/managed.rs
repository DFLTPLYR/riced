//! VM-local callbacks and staged owned requests. Workers never retain this state.
use crate::framework::tasks::{Command, Delivery, DeliveryKind, Operation, ProcessOptions};
use mlua::{Function, Lua, LuaSerdeExt, RegistryKey, Table, Value};
use serde_json::Value as Json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_RESOURCES: usize = 128;
const MAX_PENDING: usize = 256;
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Default, Clone, Copy)]
pub(crate) enum Phase {
    #[default]
    ReadOnly,
    Active,
    Closing,
}
struct Callback {
    key: RegistryKey,
    repeat: bool,
    topic: Option<String>,
    snapshot: Option<Json>,
    revision: u64,
    delivered: u64,
}
struct Bridge {
    generation: u64,
    next: u64,
    phase: Phase,
    root: PathBuf,
    callbacks: HashMap<u64, Callback>,
    pending: Vec<Command>,
}
fn error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}
fn register(
    lua: &Lua,
    function: Function,
    repeat: bool,
    topic: Option<String>,
    operation: Option<Operation>,
) -> mlua::Result<u64> {
    let key = lua.create_registry_value(function)?;
    let mut bridge = lua
        .app_data_mut::<Bridge>()
        .ok_or_else(|| error("managed tasks require a framework app"))?;
    if !matches!(bridge.phase, Phase::Active) {
        return Err(error(
            "resources can only be created in mount, actions, on_tick, or managed callbacks; not views or unmount",
        ));
    }
    if bridge.callbacks.len() >= MAX_RESOURCES || bridge.pending.len() >= MAX_PENDING {
        return Err(error("app managed resource limit exceeded"));
    }
    bridge.next = bridge
        .next
        .checked_add(1)
        .ok_or_else(|| error("operation id exhausted"))?;
    let id = bridge.next;
    bridge.callbacks.insert(
        id,
        Callback {
            key,
            repeat,
            topic,
            snapshot: None,
            revision: 0,
            delivered: 0,
        },
    );
    if let Some(operation) = operation {
        bridge.pending.push(Command::Start { id, operation });
    }
    Ok(id)
}
fn subscribe(lua: &Lua, topic: String, callback: Function) -> mlua::Result<u64> {
    if !crate::services::valid_topic(&topic) {
        return Err(error(format!("unknown service {topic:?}")));
    }
    register(lua, callback, true, Some(topic), None)
}
pub(crate) fn install(lua: &Lua, root: &Path) -> mlua::Result<()> {
    if lua.app_data_ref::<Bridge>().is_some() {
        return Ok(());
    }
    lua.set_app_data(Bridge {
        generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
        next: 0,
        phase: Phase::ReadOnly,
        root: root.to_path_buf(),
        callbacks: HashMap::new(),
        pending: Vec::new(),
    });
    let methods = lua.create_table()?;
    methods.set("spawn", lua.create_function(|lua, (_app, argv, options, callback): (Table, Table, Value, Option<Function>)| {
        let (options, callback) = match (options, callback) {
            (Value::Function(callback), None) => (ProcessOptions::default(), callback),
            (Value::Nil, Some(callback)) => (ProcessOptions::default(), callback),
            (Value::Table(options), Some(callback)) => (lua.from_value(Value::Table(options))?, callback),
            _ => return Err(error("spawn expects an argv array, optional options, and a callback")),
        };
        let argv: Vec<String> = lua.from_value(Value::Table(argv))?;
        if argv.is_empty() || argv.len() > 256 || argv[0].is_empty() || argv.iter().any(|arg| arg.contains('\0')) || argv.iter().map(String::len).sum::<usize>() > 65_536 {
            return Err(error("spawn requires 1..256 NUL-free arguments totaling at most 65536 bytes"));
        }
        if !(1..=86_400_000).contains(&options.timeout_ms) || !(1..=1_048_576).contains(&options.max_output_bytes) {
            return Err(error("spawn timeout_ms must be in 1..86400000 and max_output_bytes in 1..1048576"));
        }
        let root = lua.app_data_ref::<Bridge>().ok_or_else(|| error("app missing"))?.root.clone();
        let cwd = options.cwd.as_ref().map_or(root.clone(), |cwd| root.join(cwd));
        register(lua, callback, false, None, Some(Operation::Process { argv, options:ProcessOptions { cwd:Some(cwd), ..options } }))
    })?)?;
    for (name, repeat, minimum) in [("after", false, 1), ("every", true, 16)] {
        methods.set(
            name,
            lua.create_function(
                move |lua, (_app, millis, callback): (Table, Value, Function)| {
                    let Value::Integer(millis) = millis else {
                        return Err(error(
                            "timer duration must be an integer number of milliseconds",
                        ));
                    };
                    if !(minimum..=86_400_000).contains(&millis) {
                        return Err(error(format!(
                            "{name} duration must be in {minimum}..86400000 milliseconds"
                        )));
                    }
                    register(
                        lua,
                        callback,
                        repeat,
                        None,
                        Some(Operation::Timer {
                            millis: millis as u64,
                            repeat,
                        }),
                    )
                },
            )?,
        )?;
    }
    methods.set(
        "subscribe",
        lua.create_function(|lua, (_app, topic, callback): (Table, String, Function)| {
            subscribe(lua, topic, callback)
        })?,
    )?;
    methods.set(
        "cancel",
        lua.create_function(|lua, (_app, id): (Table, u64)| {
            let key = {
                let mut bridge = lua
                    .app_data_mut::<Bridge>()
                    .ok_or_else(|| error("app missing"))?;
                if matches!(bridge.phase, Phase::ReadOnly) {
                    return Err(error("cancel is not allowed in views"));
                }
                if bridge.pending.len() >= MAX_PENDING {
                    return Err(error("app pending operation limit exceeded"));
                }
                let Some(callback) = bridge.callbacks.remove(&id) else {
                    return Ok(false);
                };
                bridge.pending.push(Command::Cancel(id));
                callback.key
            };
            lua.remove_registry_value(key)?;
            Ok(true)
        })?,
    )?;
    lua.set_named_registry_value("riced.managed_methods", methods)?;
    let riced: Table = lua.globals().get("riced")?;
    riced.set(
        "service",
        lua.create_function(|lua, topic: String| {
            if !crate::services::valid_topic(&topic) {
                return Err(error(format!("unknown service {topic:?}")));
            }
            let service = lua.create_table()?;
            let name = topic.clone();
            service.set(
                "snapshot",
                lua.create_function(move |lua, _service: Table| {
                    lua.to_value(&crate::services::snapshot(lua, &name)?)
                })?,
            )?;
            service.set(
                "subscribe",
                lua.create_function(move |lua, (_service, callback): (Table, Function)| {
                    subscribe(lua, topic.clone(), callback)
                })?,
            )?;
            Ok(service)
        })?,
    )
}
pub(crate) fn attach(lua: &Lua, app: &Table) -> mlua::Result<()> {
    if lua.app_data_ref::<Bridge>().is_none() {
        return Ok(());
    }
    let methods: Table = lua.named_registry_value("riced.managed_methods")?;
    for pair in methods.pairs::<String, Function>() {
        let (name, function) = pair?;
        if !app.raw_get::<Value>(name.as_str())?.is_nil() {
            return Err(error(format!("app.{name} is reserved by the framework")));
        }
        app.raw_set(name, function)?;
    }
    Ok(())
}
pub(crate) fn phase(lua: &Lua, phase: Phase) {
    if let Some(mut bridge) = lua.app_data_mut::<Bridge>() {
        bridge.phase = phase;
    }
}
pub(crate) fn generation(lua: &Lua) -> Option<u64> {
    lua.app_data_ref::<Bridge>().map(|bridge| bridge.generation)
}
pub(crate) fn commands(lua: &Lua) -> Vec<Command> {
    lua.app_data_mut::<Bridge>()
        .map(|mut bridge| std::mem::take(&mut bridge.pending))
        .unwrap_or_default()
}
pub(crate) fn poll(lua: &Lua) -> mlua::Result<Vec<Delivery>> {
    let topics: Vec<_> = lua
        .app_data_ref::<Bridge>()
        .map(|bridge| {
            bridge
                .callbacks
                .iter()
                .filter_map(|(&id, callback)| {
                    callback.topic.as_ref().map(|topic| (id, topic.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut deliveries = Vec::new();
    for (id, topic) in topics {
        let snapshot = crate::services::snapshot(lua, &topic)?;
        let mut bridge = lua
            .app_data_mut::<Bridge>()
            .ok_or_else(|| error("app missing"))?;
        let generation = bridge.generation;
        if let Some(callback) = bridge.callbacks.get_mut(&id)
            && callback.snapshot.as_ref() != Some(&snapshot)
        {
            callback.snapshot = Some(snapshot.clone());
            callback.revision += 1;
            deliveries.push(Delivery {
                kind: DeliveryKind::Service,
                generation,
                id,
                revision: callback.revision,
                payload: snapshot,
            });
        }
    }
    Ok(deliveries)
}
pub(crate) fn invoke(lua: &Lua, delivery: &Delivery) -> mlua::Result<Value> {
    let function: Function = {
        let mut bridge = lua
            .app_data_mut::<Bridge>()
            .ok_or_else(|| error("app missing"))?;
        if bridge.generation != delivery.generation {
            return Ok(Value::Nil);
        }
        let Some(callback) = bridge.callbacks.get_mut(&delivery.id) else {
            return Ok(Value::Nil);
        };
        if delivery.revision <= callback.delivered
            || (callback.topic.is_some() != (delivery.kind == DeliveryKind::Service))
            || (callback.topic.is_some() && delivery.revision != callback.revision)
        {
            return Ok(Value::Nil);
        }
        callback.delivered = delivery.revision;
        let function = lua.registry_value(&callback.key)?;
        if !callback.repeat {
            bridge.callbacks.remove(&delivery.id);
        }
        function
    };
    function.call(lua.to_value(&delivery.payload)?)
}
pub(crate) fn accepts(lua: &Lua, delivery: &Delivery) -> bool {
    lua.app_data_ref::<Bridge>().is_some_and(|bridge| {
        bridge.generation == delivery.generation
            && bridge.callbacks.get(&delivery.id).is_some_and(|callback| {
                delivery.revision > callback.delivered
                    && (callback.topic.is_some() == (delivery.kind == DeliveryKind::Service))
                    && (callback.topic.is_none() || callback.revision == delivery.revision)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lua() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .set("riced", lua.create_table().unwrap())
            .unwrap();
        install(&lua, Path::new("/tmp/opencode")).unwrap();
        let app = lua.create_table().unwrap();
        attach(&lua, &app).unwrap();
        lua.globals().set("app", app).unwrap();
        lua
    }
    #[test]
    fn views_and_teardown_cannot_start_resources_and_invalid_options_fail_early() {
        let lua = lua();
        assert!(lua.load("app:after(1,function() end)").exec().is_err());
        phase(&lua, Phase::Active);
        for source in [
            "app:spawn({},function() end)",
            "app:spawn({'echo'},{timeout_ms=0},function() end)",
            "app:spawn({'echo'},{max_output_bytes=1048577},function() end)",
            "app:spawn({'echo'},{typo=true},function() end)",
            "app:every(0,function() end)",
            "app:every(1.5,function() end)",
            "app:subscribe('missing',function() end)",
        ] {
            assert!(lua.load(source).exec().is_err(), "{source}");
        }
        phase(&lua, Phase::Closing);
        assert!(lua.load("app:after(1,function() end)").exec().is_err());
        assert!(commands(&lua).is_empty());
    }
    #[test]
    fn snapshots_are_detached_and_subscriptions_coalesce_and_cancel() {
        let lua = lua();
        phase(&lua, Phase::Active);
        lua.load(r#"
            system={cpu_usage=1}
            local service=riced.service('system')
            local snapshot=service:snapshot()
            snapshot.data.cpu_usage=99
            assert(service:snapshot().data.cpu_usage==1)
            calls=0
            token=service:subscribe(function(snapshot) calls=calls+1; latest=snapshot.data.cpu_usage end)
        "#).exec().unwrap();
        let first = poll(&lua).unwrap().pop().unwrap();
        lua.load("system.cpu_usage=2").exec().unwrap();
        let second = poll(&lua).unwrap().pop().unwrap();
        assert!(!accepts(&lua, &first));
        invoke(&lua, &first).unwrap();
        invoke(&lua, &second).unwrap();
        assert_eq!(lua.globals().get::<u32>("calls").unwrap(), 1);
        assert_eq!(lua.globals().get::<u32>("latest").unwrap(), 2);
        assert!(poll(&lua).unwrap().is_empty());
        assert!(!accepts(&lua, &second));
        lua.load("assert(app:cancel(token)); assert(not app:cancel(token))")
            .exec()
            .unwrap();
        assert!(poll(&lua).unwrap().is_empty());
    }
    #[test]
    fn callbacks_can_schedule_work_without_reentrant_borrows_and_release_once() {
        let lua = lua();
        phase(&lua, Phase::Active);
        let id: u64 = lua.load("return app:after(1,function(result) app:after(1,function() end); calls=(calls or 0)+1 end)").eval().unwrap();
        let delivery = Delivery {
            kind: DeliveryKind::Job,
            generation: generation(&lua).unwrap(),
            id,
            revision: 1,
            payload: serde_json::json!({"ok":true}),
        };
        invoke(&lua, &delivery).unwrap();
        invoke(&lua, &delivery).unwrap();
        assert_eq!(lua.globals().get::<u32>("calls").unwrap(), 1);
        assert_eq!(lua.app_data_ref::<Bridge>().unwrap().callbacks.len(), 1);
    }
    #[test]
    fn resource_and_pending_limits_bound_native_allocations() {
        let lua = lua();
        phase(&lua, Phase::Active);
        assert!(
            lua.load("for i=1,129 do app:after(1,function() end) end")
                .exec()
                .is_err()
        );
        assert_eq!(lua.app_data_ref::<Bridge>().unwrap().callbacks.len(), 128);
        let lua = self::lua();
        phase(&lua, Phase::Active);
        assert!(
            lua.load("for i=1,257 do local id=app:after(1,function() end); app:cancel(id) end")
                .exec()
                .is_err()
        );
        assert!(lua.app_data_ref::<Bridge>().unwrap().pending.len() <= MAX_PENDING);
    }
}
