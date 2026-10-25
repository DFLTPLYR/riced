//! VM-local document handles. Disk work is staged through the managed executor.
use super::managed;
use crate::framework::{
    storage::{self, Action, Baseline, Format, Request},
    tasks::{Command, Delivery, Operation},
};
use mlua::{Function, Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_HANDLES: usize = 16;
#[cfg(test)]
mod tests;
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WriteMode {
    Generated,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    #[serde(rename = "type")]
    format: Format,
    path: PathBuf,
    #[serde(default)]
    write_mode: Option<WriteMode>,
    #[serde(default)]
    create_parents: bool,
}
struct Document {
    epoch: u64,
    watch: Option<Watch>,
    options: Options,
    defaults: Arc<Json>,
    data: Arc<Json>,
    baseline: Baseline,
    revision: u64,
    saved_revision: u64,
    pending: Option<u64>,
}
struct Watch {
    id: u64,
    interval: Duration,
    due: Instant,
    sequence: u64,
    flight: Option<(u64, u64)>,
    observed: Baseline,
    io_error: Option<Json>,
    content_error: Option<Json>,
}
struct Store {
    generation: u64,
    next: u64,
    root: PathBuf,
    documents: HashMap<u64, Document>,
}
#[derive(Clone, Copy)]
struct Handle {
    generation: u64,
    id: u64,
}
#[derive(Debug)]
pub(crate) struct Pending {
    handle: u64,
    revision: u64,
    kind: PendingKind,
}
#[derive(Debug)]
enum PendingKind {
    Reload { discard: bool },
    Save,
}
fn error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}
fn with_document<T>(
    lua: &Lua,
    handle: Handle,
    f: impl FnOnce(&Document) -> mlua::Result<T>,
) -> mlua::Result<T> {
    let store = lua
        .app_data_ref::<Store>()
        .ok_or_else(|| error("file store missing"))?;
    if store.generation != handle.generation {
        return Err(error("file handle belongs to a retired instance"));
    }
    f(store
        .documents
        .get(&handle.id)
        .ok_or_else(|| error("file handle is closed"))?)
}
fn with_document_mut<T>(
    lua: &Lua,
    handle: Handle,
    f: impl FnOnce(&mut Document) -> mlua::Result<T>,
) -> mlua::Result<T> {
    let mut store = lua
        .app_data_mut::<Store>()
        .ok_or_else(|| error("file store missing"))?;
    if store.generation != handle.generation {
        return Err(error("file handle belongs to a retired instance"));
    }
    f(store
        .documents
        .get_mut(&handle.id)
        .ok_or_else(|| error("file handle is closed"))?)
}
fn path(value: Value) -> mlua::Result<Vec<String>> {
    match value {
        Value::String(key) => Ok(vec![key.to_str()?.to_string()]),
        Value::Table(keys) => {
            let count = keys.clone().pairs::<Value, Value>().count();
            if count == 0 || count > 32 || keys.raw_len() != count {
                return Err(error("key path must be a dense array of 1..32 strings"));
            }
            for pair in keys.clone().pairs::<Value, Value>() {
                let (key, value) = pair?;
                if !matches!(key, Value::Integer(index) if index > 0 && index as usize <= count)
                    || !matches!(value, Value::String(_))
                {
                    return Err(error(
                        "key path must contain only consecutive string segments",
                    ));
                }
            }
            (1..=count).map(|index| keys.raw_get(index)).collect()
        }
        _ => Err(error(
            "key must be a string or an array of string path segments",
        )),
    }
}
fn edit(data: &mut Json, path: &[String], value: Option<Json>) -> mlua::Result<()> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| error("key path cannot be empty"))?;
    let mut map = data
        .as_object_mut()
        .ok_or_else(|| error("document root must be a map"))?;
    for key in parents {
        if !map.contains_key(key) {
            if value.is_none() {
                return Ok(());
            }
            map.insert(key.clone(), json!({}));
        }
        map = map
            .get_mut(key)
            .and_then(Json::as_object_mut)
            .ok_or_else(|| error("intermediate key path must be a map"))?;
    }
    if let Some(value) = value {
        map.insert(last.clone(), value);
    } else {
        map.remove(last);
    }
    Ok(())
}
fn update(lua: &Lua, handle: Handle, key: Value, value: Option<Json>) -> mlua::Result<bool> {
    managed::require_active(lua)?;
    let path = path(key)?;
    with_document_mut(lua, handle, |document| {
        let mut data = (*document.data).clone();
        edit(&mut data, &path, value)?;
        storage::validate(&data).map_err(mlua::Error::external)?;
        if data == *document.data {
            return Ok(false);
        }
        document.revision = document
            .revision
            .checked_add(1)
            .ok_or_else(|| error("file revision exhausted"))?;
        document.data = Arc::new(data);
        document.epoch = document.epoch.wrapping_add(1);
        Ok(true)
    })
}
fn start(
    lua: &Lua,
    handle: Handle,
    callback: Option<Function>,
    kind: PendingKind,
) -> mlua::Result<u64> {
    managed::require_active(lua)?;
    let (request, pending) = with_document(lua, handle, |document| {
        if document.pending.is_some() {
            return Err(error(
                "file handle is busy; wait for completion or cancel its token",
            ));
        }
        let action = match kind {
            PendingKind::Reload { .. } => Action::Reload,
            PendingKind::Save => {
                if document.options.format == Format::Lua && document.options.write_mode.is_none() {
                    return Err(error(
                        "Lua writes require write_mode = 'generated'; comments and expressions will be replaced",
                    ));
                }
                if matches!(document.baseline, Baseline::Unloaded) {
                    return Err(error("reload the file successfully before saving"));
                }
                Action::Save {
                    data: document.data.clone(),
                    expected: document.baseline.clone(),
                }
            }
        };
        Ok((
            Request {
                path: document.options.path.clone(),
                format: document.options.format,
                create_parents: document.options.create_parents,
                action,
            },
            Pending {
                handle: handle.id,
                revision: document.revision,
                kind,
            },
        ))
    })?;
    let callback = match callback {
        Some(callback) => callback,
        None => lua.create_function(|_, _: Value| Ok(()))?,
    };
    let id = managed::register_file(lua, callback, Operation::File(request), pending)?;
    with_document_mut(lua, handle, |document| {
        document.pending = Some(id);
        document.epoch = document.epoch.wrapping_add(1);
        Ok(())
    })?;
    Ok(id)
}
impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |lua, handle, key: Value| {
            let data = with_document(lua, *handle, |document| {
                if key.is_nil() {
                    return Ok(Some((*document.data).clone()));
                }
                let path = path(key)?;
                let mut value = document.data.as_ref();
                for key in path {
                    let Some(next) = value.get(&key) else {
                        return Ok(None);
                    };
                    value = next;
                }
                Ok(Some(value.clone()))
            })?;
            match data {
                Some(data) => lua.to_value(&data),
                None => Ok(Value::Nil),
            }
        });
        methods.add_method("set", |lua, handle, (key, value): (Value, Value)| {
            let wrapper = lua.create_table()?;
            wrapper.raw_set("value", value)?;
            let mut data =
                storage::from_lua(lua, Value::Table(wrapper)).map_err(mlua::Error::external)?;
            let value = data
                .as_object_mut()
                .and_then(|map| map.remove("value"))
                .ok_or_else(|| error("set requires a value; use remove to delete a key"))?;
            update(lua, *handle, key, Some(value))
        });
        methods.add_method("remove", |lua, handle, key: Value| {
            update(lua, *handle, key, None)
        });
        methods.add_method("status", |lua, handle, ()| {
            let status = with_document(lua, *handle, |document| Ok(json!({"path":document.options.path,"type":document.options.format,
                "revision":document.revision,"saved_revision":document.saved_revision,"dirty":document.revision!=document.saved_revision,"loaded":!matches!(document.baseline,Baseline::Unloaded),"busy":document.pending.is_some(),"watching":document.watch.is_some()})))?;
            lua.to_value(&status)
        });
        methods.add_method(
            "reload",
            |lua, handle, (options, callback): (Value, Option<Function>)| {
                #[derive(Default, Deserialize)]
                #[serde(default, deny_unknown_fields)]
                struct ReloadOptions {
                    discard: bool,
                }
                let (options, callback) = match (options, callback) {
                    (Value::Function(callback), None) => (ReloadOptions::default(), Some(callback)),
                    (Value::Nil, callback) => (ReloadOptions::default(), callback),
                    (Value::Table(options), callback) => {
                        (lua.from_value(Value::Table(options))?, callback)
                    }
                    _ => {
                        return Err(error(
                            "reload expects optional {discard=true} and an optional callback",
                        ));
                    }
                };
                start(
                    lua,
                    *handle,
                    callback,
                    PendingKind::Reload {
                        discard: options.discard,
                    },
                )
            },
        );
        methods.add_method("save", |lua, handle, callback: Option<Function>| {
            start(lua, *handle, callback, PendingKind::Save)
        });
        methods.add_method(
            "watch",
            |lua, handle, (options, callback): (Value, Option<Function>)| {
                managed::require_active(lua)?;
                #[derive(Deserialize)]
                #[serde(default, deny_unknown_fields)]
                struct WatchOptions {
                    interval_ms: u64,
                }
                impl Default for WatchOptions {
                    fn default() -> Self {
                        Self { interval_ms: 500 }
                    }
                }
                let (options, callback) = match (options, callback) {
                    (Value::Function(callback), None) => (WatchOptions::default(), callback),
                    (Value::Nil, Some(callback)) => (WatchOptions::default(), callback),
                    (Value::Table(options), Some(callback)) => {
                        (lua.from_value(Value::Table(options))?, callback)
                    }
                    _ => {
                        return Err(error(
                            "watch expects optional {interval_ms=500} and a callback",
                        ));
                    }
                };
                if !(500..=86_400_000).contains(&options.interval_ms) {
                    return Err(error("watch interval_ms must be in 500..86400000"));
                }
                with_document(lua, *handle, |document| {
                    if document.watch.is_some() {
                        Err(error("file already has a watcher; cancel its token first"))
                    } else {
                        Ok(())
                    }
                })?;
                let id = managed::register_watch(lua, callback, handle.id)?;
                with_document_mut(lua, *handle, |document| {
                    document.watch = Some(Watch {
                        id,
                        interval: Duration::from_millis(options.interval_ms),
                        due: Instant::now(),
                        sequence: 0,
                        flight: None,
                        observed: document.baseline.clone(),
                        io_error: None,
                        content_error: None,
                    });
                    Ok(())
                })?;
                Ok(id)
            },
        );
        methods.add_method("close", |lua, handle, ()| {
            managed::require_active(lua)?;
            with_document(lua, *handle, |document| {
                if document.pending.is_some() {
                    Err(error(
                        "cancel or finish pending work before closing the file",
                    ))
                } else {
                    Ok(())
                }
            })?;
            let watch = with_document(lua, *handle, |document| {
                Ok(document.watch.as_ref().map(|watch| watch.id))
            })?;
            if let Some(id) = watch {
                managed::cancel_token(lua, id)?;
            }
            if let Some(mut store) = lua.app_data_mut::<Store>() {
                store.documents.remove(&handle.id);
            }
            Ok(())
        });
    }
}
pub(crate) fn install(lua: &Lua, root: &Path) -> mlua::Result<()> {
    lua.set_app_data(Store {
        generation: managed::generation(lua).ok_or_else(|| error("framework app missing"))?,
        next: 0,
        root: root.to_path_buf(),
        documents: HashMap::new(),
    });
    let riced: Table = lua.globals().get("riced")?;
    riced.set(
        "file",
        lua.create_function(|lua, fields: Table| {
            managed::require_active(lua)?;
            let options = lua.create_table()?;
            for pair in fields.clone().pairs::<String, Value>() {
                let (key, value) = pair?;
                if key != "defaults" {
                    options.raw_set(key, value)?;
                }
            }
            let mut options: Options = lua.from_value(Value::Table(options))?;
            if options.path.as_os_str().is_empty() || options.path.file_name().is_none() {
                return Err(error("file.path must identify a file"));
            }
            let defaults = match fields.raw_get::<Value>("defaults")? {
                Value::Nil => json!({}),
                value => storage::from_lua(lua, value).map_err(mlua::Error::external)?,
            };
            let mut store = lua
                .app_data_mut::<Store>()
                .ok_or_else(|| error("file store missing"))?;
            if store.documents.len() >= MAX_HANDLES {
                return Err(error(
                    "at most 16 file handles may be open per app; close unused handles",
                ));
            }
            options.path = store.root.join(options.path);
            store.next = store
                .next
                .checked_add(1)
                .ok_or_else(|| error("file handle ids exhausted"))?;
            let id = store.next;
            let generation = store.generation;
            let defaults = Arc::new(defaults);
            store.documents.insert(
                id,
                Document {
                    epoch: 0,
                    watch: None,
                    options,
                    data: defaults.clone(),
                    defaults,
                    baseline: Baseline::Unloaded,
                    revision: 0,
                    saved_revision: 0,
                    pending: None,
                },
            );
            Ok(Handle { generation, id })
        })?,
    )?;
    riced.set(
        "array",
        lua.create_function(|lua, table: Table| {
            table.set_metatable(Some(lua.array_metatable()))?;
            Ok(table)
        })?,
    )
}
pub(crate) fn cancel(lua: &Lua, pending: Pending, id: u64) {
    if let Some(mut store) = lua.app_data_mut::<Store>()
        && let Some(document) = store.documents.get_mut(&pending.handle)
        && document.pending == Some(id)
    {
        document.pending = None;
        document.baseline = Baseline::Unloaded;
        document.epoch = document.epoch.wrapping_add(1);
        reset_observation(document);
    }
}
pub(crate) fn complete(lua: &Lua, pending: Pending, delivery: &Delivery) -> Json {
    let mut result = delivery.payload.clone();
    let Some(mut store) = lua.app_data_mut::<Store>() else {
        return json!({"ok":false,"error":{"code":"closed","message":"file store missing"}});
    };
    let Some(document) = store.documents.get_mut(&pending.handle) else {
        return json!({"ok":false,"error":{"code":"closed","message":"file handle closed"}});
    };
    if document.pending != Some(delivery.id) {
        return json!({"ok":false,"error":{"code":"stale","message":"file operation superseded"}});
    }
    document.pending = None;
    document.epoch = document.epoch.wrapping_add(1);
    if result["ok"] == true {
        match pending.kind {
            PendingKind::Reload { discard } => {
                if document.revision != pending.revision
                    || (!discard && document.revision != document.saved_revision)
                {
                    result = json!({"ok":false,"type":"file","operation":"reload","error":{"code":"dirty","message":"reload would replace unsaved edits; explicitly discard or retry"}});
                } else {
                    let data = storage::merge(&document.defaults, result["data"].clone());
                    match storage::validate(&data) {
                        Ok(()) => {
                            if data != *document.data {
                                let Some(revision) = document.revision.checked_add(1) else {
                                    return json!({"ok":false,"error":{"code":"limit","message":"file revision exhausted"}});
                                };
                                document.revision = revision;
                            }
                            document.data = Arc::new(data);
                            document.saved_revision = document.revision;
                            document.baseline = baseline(&result);
                            result["data"] = (*document.data).clone();
                        }
                        Err(error) => {
                            result =
                                json!({"ok":false,"type":"file","operation":"reload","error":error})
                        }
                    }
                }
            }
            PendingKind::Save => {
                document.baseline = baseline(&result);
                document.saved_revision = pending.revision;
            }
        }
    }
    if let Some(map) = result.as_object_mut() {
        map.remove("baseline");
    }
    if result["ok"] == true {
        reset_observation(document);
    }
    result["revision"] = document.revision.into();
    result["path"] = json!(document.options.path);
    result["saved_revision"] = document.saved_revision.into();
    result["dirty"] = (document.revision != document.saved_revision).into();
    result
}
fn baseline(result: &Json) -> Baseline {
    result["baseline"]
        .as_str()
        .map_or(Baseline::Missing, |source| {
            Baseline::Contents(Arc::from(source))
        })
}

fn reset_observation(document: &mut Document) {
    if let Some(watch) = document.watch.as_mut() {
        watch.observed = document.baseline.clone();
        watch.io_error = None;
        watch.content_error = None;
    }
}
/// Called at the host drain, after candidate validation; never performs filesystem I/O.
pub(crate) fn schedule(lua: &Lua) -> Vec<Command> {
    let Some(mut store) = lua.app_data_mut::<Store>() else {
        return Vec::new();
    };
    let now = Instant::now();
    let mut commands = Vec::new();
    for document in store.documents.values_mut() {
        let Some(watch) = document.watch.as_mut() else {
            continue;
        };
        if document.pending.is_some() || watch.flight.is_some() || now < watch.due {
            continue;
        }
        let Some(sequence) = watch.sequence.checked_add(1) else {
            continue;
        };
        watch.sequence = sequence;
        watch.flight = Some((sequence, document.epoch));
        commands.push(Command::PollFile {
            id: watch.id,
            revision: sequence,
            request: Request {
                path: document.options.path.clone(),
                format: document.options.format,
                create_parents: false,
                action: Action::Watch {
                    known: watch.observed.clone(),
                },
            },
        });
    }
    commands
}
pub(crate) fn cancel_watch(lua: &Lua, handle: u64, id: u64) {
    if let Some(mut store) = lua.app_data_mut::<Store>()
        && let Some(document) = store.documents.get_mut(&handle)
        && document.watch.as_ref().is_some_and(|watch| watch.id == id)
    {
        document.watch = None;
    }
}
pub(crate) fn observed(lua: &Lua, handle: u64, delivery: &Delivery) -> Option<Json> {
    let mut store = lua.app_data_mut::<Store>()?;
    let document = store.documents.get_mut(&handle)?;
    let mut watch = document.watch.take()?;
    let result = apply_observation(document, &mut watch, delivery);
    document.watch = Some(watch);
    result
}
fn watch_event(document: &Document, event: &str, failure: Option<Json>, missing: &Json) -> Json {
    let mut result = json!({"ok":failure.is_none(),"type":"file","operation":"watch","event":event,
        "path":document.options.path,"revision":document.revision,"saved_revision":document.saved_revision,
        "dirty":document.revision!=document.saved_revision});
    if missing.is_boolean() {
        result["missing"] = missing.clone();
    }
    if let Some(failure) = failure {
        result["error"] = failure;
    } else {
        result["data"] = (*document.data).clone();
    }
    result
}
fn apply_observation(
    document: &mut Document,
    watch: &mut Watch,
    delivery: &Delivery,
) -> Option<Json> {
    let (sequence, epoch) = watch.flight?;
    if watch.id != delivery.id || sequence != delivery.revision {
        return None;
    }
    watch.flight = None;
    watch.due = Instant::now() + watch.interval;
    // A save/reload or local edit after this scan started makes its snapshot stale.
    if epoch != document.epoch || document.pending.is_some() {
        return None;
    }
    let payload = &delivery.payload;
    if payload["ok"] != true {
        let failure = payload["error"].clone();
        if watch.io_error.as_ref() == Some(&failure) {
            return None;
        }
        watch.io_error = Some(failure.clone());
        return Some(watch_event(
            document,
            "error",
            Some(failure),
            &payload["missing"],
        ));
    }
    let recovered = watch.io_error.take().is_some();
    if payload["unchanged"] == true {
        if !recovered {
            return None;
        }
        if let Some(failure) = watch.content_error.clone() {
            return Some(watch_event(
                document,
                "error",
                Some(failure),
                &payload["missing"],
            ));
        }
        if document.revision != document.saved_revision {
            return Some(watch_event(
                document,
                "conflict",
                Some(
                    json!({"code":"conflict","message":"file is readable again but unsaved local edits remain"}),
                ),
                &payload["missing"],
            ));
        }
        return Some(watch_event(
            document,
            "recovered",
            None,
            &payload["missing"],
        ));
    }
    watch.observed = baseline(payload);
    if payload["watch_error"].is_object() {
        let failure = payload["watch_error"].clone();
        watch.content_error = Some(failure.clone());
        return Some(watch_event(
            document,
            "error",
            Some(failure),
            &payload["missing"],
        ));
    }
    watch.content_error = None;
    if document.revision != document.saved_revision {
        return Some(watch_event(
            document,
            "conflict",
            Some(
                json!({"code":"conflict","message":"external file change would replace unsaved local edits"}),
            ),
            &payload["missing"],
        ));
    }
    let initial = matches!(document.baseline, Baseline::Unloaded);
    let data = storage::merge(&document.defaults, payload["data"].clone());
    if let Err(error) = storage::validate(&data) {
        let failure = json!(error);
        watch.content_error = Some(failure.clone());
        return Some(watch_event(
            document,
            "error",
            Some(failure),
            &payload["missing"],
        ));
    }
    if data != *document.data {
        let Some(revision) = document.revision.checked_add(1) else {
            return Some(watch_event(
                document,
                "error",
                Some(json!({"code":"limit","message":"file revision exhausted"})),
                &payload["missing"],
            ));
        };
        document.revision = revision;
    }
    document.data = Arc::new(data);
    document.baseline = baseline(payload);
    document.saved_revision = document.revision;
    document.epoch = document.epoch.wrapping_add(1);
    let event = if initial {
        "loaded"
    } else if payload["missing"] == true {
        "deleted"
    } else {
        "changed"
    };
    Some(watch_event(document, event, None, &payload["missing"]))
}
