//! Bounded document codecs and serialized atomic file operations, off the UI thread.
use iced::futures::future::{Either, select};
use mlua::{Lua, LuaSerdeExt, Table, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Semaphore, oneshot};

pub(crate) const MAX_BYTES: usize = 1_048_576;
const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 32;
static WORKERS: Semaphore = Semaphore::const_new(4);
type PathLocks = HashMap<PathBuf, Weak<Mutex<()>>>;
static LOCKS: LazyLock<Mutex<PathLocks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    Json,
    Toml,
    Lua,
}
#[derive(Debug, Clone)]
pub(crate) enum Baseline {
    Unloaded,
    Missing,
    Contents(Arc<str>),
}
#[derive(Debug, Clone)]
pub(crate) enum Action {
    Reload,
    Watch { known: Baseline },
    Save { data: Arc<Json>, expected: Baseline },
}
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub path: PathBuf,
    pub format: Format,
    pub create_parents: bool,
    pub action: Action,
}
#[derive(Debug, thiserror::Error, Serialize)]
#[error("{code}: {message}")]
pub(crate) struct StorageError {
    pub code: &'static str,
    pub message: String,
}
fn error(code: &'static str, message: impl Into<String>) -> StorageError {
    let mut message = message.into();
    if message.len() > 4096 {
        let mut end = 4096;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str("...");
    }
    StorageError { code, message }
}
fn io(error: impl std::fmt::Display) -> StorageError {
    self::error("io", error.to_string())
}

/// Strict portable values: map roots, dense arrays, signed integers, finite floats.
pub(crate) fn from_lua(lua: &Lua, value: Value) -> Result<Json, StorageError> {
    fn decode(
        lua: &Lua,
        value: Value,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<Json, StorageError> {
        *nodes += 1;
        if depth > MAX_DEPTH || *nodes > MAX_NODES {
            return Err(error("limit", "document nesting or node limit exceeded"));
        }
        match value {
            Value::Boolean(value) => Ok(Json::Bool(value)),
            Value::Integer(value) => Ok(value.into()),
            Value::Number(value) if value.is_finite() => serde_json::Number::from_f64(value)
                .map(Json::Number)
                .ok_or_else(|| error("value", "invalid number")),
            Value::String(value) => Ok(Json::String(value.to_str().map_err(io)?.to_string())),
            Value::Table(table) => {
                let pairs = table
                    .clone()
                    .pairs::<Value, Value>()
                    .collect::<mlua::Result<Vec<_>>>()
                    .map_err(io)?;
                let array = (!pairs.is_empty()
                    && pairs
                        .iter()
                        .all(|(key, _)| matches!(key, Value::Integer(1..))))
                    || table
                        .metatable()
                        .is_some_and(|mt| mt == lua.array_metatable());
                if array {
                    if pairs.iter().any(|(key, _)| !matches!(key, Value::Integer(index) if *index > 0 && *index as usize <= pairs.len())) {
                        return Err(error("value", "arrays must have dense consecutive indices starting at 1"));
                    }
                    let mut values = Vec::with_capacity(pairs.len());
                    for index in 1..=pairs.len() {
                        values.push(decode(
                            lua,
                            table.raw_get(index).map_err(io)?,
                            depth + 1,
                            nodes,
                        )?);
                    }
                    Ok(Json::Array(values))
                } else {
                    let mut values = Map::new();
                    for (key, value) in pairs {
                        let Value::String(key) = key else {
                            return Err(error(
                                "value",
                                "maps require string keys; mixed or sparse tables are unsupported",
                            ));
                        };
                        values.insert(
                            key.to_str().map_err(io)?.to_string(),
                            decode(lua, value, depth + 1, nodes)?,
                        );
                    }
                    Ok(Json::Object(values))
                }
            }
            _ => Err(error(
                "value",
                "documents cannot contain nil/null, functions, userdata, threads, or non-finite numbers",
            )),
        }
    }
    let data = decode(lua, value, 0, &mut 0)?;
    validate(&data)?;
    Ok(data)
}
pub(crate) fn validate(data: &Json) -> Result<(), StorageError> {
    fn visit(data: &Json, depth: usize, nodes: &mut usize) -> Result<(), StorageError> {
        *nodes += 1;
        if depth > MAX_DEPTH || *nodes > MAX_NODES {
            return Err(error("limit", "document nesting or node limit exceeded"));
        }
        match data {
            Json::Null => {
                return Err(error(
                    "value",
                    "null is not supported in portable documents",
                ));
            }
            Json::Number(n) if n.is_u64() && n.as_i64().is_none() => {
                return Err(error(
                    "value",
                    "integers must fit signed 64-bit Lua integers",
                ));
            }
            Json::Array(values) => {
                for value in values {
                    visit(value, depth + 1, nodes)?;
                }
            }
            Json::Object(values) => {
                for value in values.values() {
                    visit(value, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    if !data.is_object() {
        return Err(error("value", "document root must be a string-keyed map"));
    }
    visit(data, 0, &mut 0)?;
    if serde_json::to_vec(data).map_err(io)?.len() > MAX_BYTES {
        return Err(error("limit", "document exceeds 1 MiB"));
    }
    Ok(())
}
pub(crate) fn merge(defaults: &Json, loaded: Json) -> Json {
    match (defaults, loaded) {
        (Json::Object(defaults), Json::Object(mut loaded)) => {
            for (key, value) in defaults {
                let merged = loaded
                    .remove(key)
                    .map_or_else(|| value.clone(), |loaded| merge(value, loaded));
                loaded.insert(key.clone(), merged);
            }
            Json::Object(loaded)
        }
        (_, loaded) => loaded,
    }
}
fn lua_literal(data: &Json) -> String {
    match data {
        Json::Number(value) if value.as_i64() == Some(i64::MIN) => {
            "(-9223372036854775807 - 1)".into()
        }
        Json::Array(values) => format!(
            "array({{ {} }})",
            values
                .iter()
                .map(lua_literal)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Json::Object(values) => format!(
            "{{ {} }}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "[{}] = {}",
                    crate::config::to_lua_literal(&Json::String(key.clone())),
                    lua_literal(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => crate::config::to_lua_literal(data),
    }
}
pub(crate) fn encode(format: Format, data: &Json) -> Result<String, StorageError> {
    validate(data)?;
    let source = match format {
        Format::Json => serde_json::to_string_pretty(data).map_err(io)?,
        Format::Toml => toml::to_string_pretty(data).map_err(|e| error("encode", e.to_string()))?,
        Format::Lua => format!(
            "-- Generated data; expressions, comments, and formatting are not preserved.\nreturn {}\n",
            lua_literal(data)
        ),
    };
    if source.len() > MAX_BYTES {
        return Err(error("limit", "serialized document exceeds 1 MiB"));
    }
    Ok(source)
}
pub(crate) fn decode(format: Format, source: &str) -> Result<Json, StorageError> {
    if source.len() > MAX_BYTES {
        return Err(error("limit", "file exceeds 1 MiB"));
    }
    let data = match format {
        Format::Json => serde_json::from_str(source).map_err(|e| error("parse", e.to_string()))?,
        Format::Toml => {
            fn portable(value: &toml::Value) -> bool {
                match value {
                    toml::Value::Datetime(_) => false,
                    toml::Value::Array(values) => values.iter().all(portable),
                    toml::Value::Table(values) => values.values().all(portable),
                    _ => true,
                }
            }
            let value: toml::Value =
                toml::from_str(source).map_err(|e| error("parse", e.to_string()))?;
            if !portable(&value) {
                return Err(error(
                    "value",
                    "TOML datetimes are unsupported; use a quoted string",
                ));
            }
            serde_json::to_value(value).map_err(io)?
        }
        Format::Lua => {
            let lua =
                crate::lua::sandbox::new_lua(crate::lua::sandbox::Profile::Config).map_err(io)?;
            lua.globals()
                .set(
                    "array",
                    lua.create_function(|lua, table: Table| {
                        table.set_metatable(Some(lua.array_metatable()))?;
                        Ok(table)
                    })
                    .map_err(io)?,
                )
                .map_err(io)?;
            let value = lua
                .load(source)
                .set_name("@riced-file")
                .eval()
                .map_err(|e| error("parse", e.to_string()))?;
            from_lua(&lua, value)?
        }
    };
    validate(&data)?;
    Ok(data)
}
fn read(path: &Path) -> Result<Option<String>, StorageError> {
    let fd = match rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    let file = File::from(fd);
    if !file.metadata().map_err(io)?.is_file() {
        return Err(error("path", "only regular files are supported"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > MAX_BYTES {
        return Err(error("limit", "file exceeds 1 MiB"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| error("parse", e.to_string()))
}
fn check_cancel(cancelled: &AtomicBool) -> Result<(), StorageError> {
    if cancelled.load(Ordering::Acquire) {
        Err(error("cancelled", "file operation cancelled"))
    } else {
        Ok(())
    }
}
fn execute(request: &Request, cancelled: &AtomicBool) -> Result<Json, StorageError> {
    check_cancel(cancelled)?;
    let parent = request
        .path
        .parent()
        .ok_or_else(|| error("path", "file needs a parent directory"))?;
    if matches!(request.action, Action::Save { .. }) && request.create_parents {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && matches!(request.action, Action::Reload | Action::Watch { .. }) =>
        {
            return observation(&request.action, request.format, None);
        }
        Err(e) => return Err(io(e)),
    };
    let path = parent.join(
        request
            .path
            .file_name()
            .ok_or_else(|| error("path", "file name missing"))?,
    );
    let lock = {
        let mut locks = LOCKS
            .lock()
            .map_err(|_| error("lock", "path registry poisoned"))?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&path).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(Mutex::new(()));
            locks.insert(path.clone(), Arc::downgrade(&lock));
            lock
        }
    };
    let _guard = lock
        .lock()
        .map_err(|_| error("lock", "file lock poisoned"))?;
    check_cancel(cancelled)?;
    let current = read(&path)?;
    match &request.action {
        Action::Watch { .. } => observation(&request.action, request.format, current),
        Action::Reload => match current {
            Some(source) => Ok(
                json!({"data":decode(request.format, &source)?,"baseline":source,"missing":false}),
            ),
            None => Ok(json!({"data":{},"baseline":null,"missing":true})),
        },
        Action::Save { data, expected } => {
            let expected = match expected {
                Baseline::Unloaded => return Err(error("reload_required", "reload before saving")),
                Baseline::Missing => None,
                Baseline::Contents(source) => Some(source.as_ref()),
            };
            if current.as_deref() != expected {
                return Err(error(
                    "conflict",
                    "file changed externally; reload before saving",
                ));
            }
            let source = encode(request.format, data)?;
            let mut temp = tempfile::NamedTempFile::new_in(&parent).map_err(io)?;
            if current.is_some() {
                temp.as_file()
                    .set_permissions(std::fs::metadata(&path).map_err(io)?.permissions())
                    .map_err(io)?;
            }
            check_cancel(cancelled)?;
            temp.write_all(source.as_bytes()).map_err(io)?;
            temp.flush().map_err(io)?;
            temp.as_file().sync_all().map_err(io)?;
            check_cancel(cancelled)?;
            if read(&path)?.as_deref() != expected {
                return Err(error("conflict", "file changed during save"));
            }
            check_cancel(cancelled)?;
            temp.persist(&path).map_err(io)?;
            Ok(json!({"baseline":source,"missing":false}))
        }
    }
}
fn observation(
    action: &Action,
    format: Format,
    current: Option<String>,
) -> Result<Json, StorageError> {
    if let Action::Watch { known } = action {
        let unchanged = match known {
            Baseline::Unloaded => false,
            Baseline::Missing => current.is_none(),
            Baseline::Contents(source) => current.as_deref() == Some(source.as_ref()),
        };
        if unchanged {
            return Ok(json!({"unchanged":true,"missing":current.is_none()}));
        }
    }
    match current {
        Some(source) => match decode(format, &source) {
            Ok(data) => Ok(json!({"data":data,"baseline":source,"missing":false})),
            Err(error) if matches!(action, Action::Watch { .. }) => {
                Ok(json!({"watch_error":error,"baseline":source,"missing":false}))
            }
            Err(error) => Err(error),
        },
        None => Ok(json!({"data":{},"baseline":null,"missing":true})),
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
pub(crate) async fn run(request: Request, mut receiver: oneshot::Receiver<()>) -> Json {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(cancelled.clone());
    let permit = match select(Box::pin(WORKERS.acquire()), &mut receiver).await {
        Either::Left((Ok(permit), _)) => permit,
        _ => {
            return json!({"ok":false,"error":{"code":"cancelled","message":"file operation cancelled before starting"}});
        }
    };
    let operation = match request.action {
        Action::Reload => "reload",
        Action::Watch { .. } => "watch",
        Action::Save { .. } => "save",
    };
    let path = request.path.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        execute(&request, &cancelled)
    });
    let result = match select(worker, receiver).await {
        Either::Left((Ok(result), _)) => result,
        Either::Left((Err(e), _)) => Err(error("worker", e.to_string())),
        Either::Right(_) => Err(error(
            "cancelled",
            "file operation cancelled; an already committed save cannot be rolled back",
        )),
    };
    match result {
        Ok(mut result) => {
            result["ok"] = true.into();
            result["type"] = "file".into();
            result["operation"] = operation.into();
            result["path"] = json!(path);
            result
        }
        Err(error) => {
            json!({"ok":false,"type":"file","operation":operation,"path":path,"error":error})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("riced-storage-")
            .tempdir_in("/tmp/opencode")
            .unwrap()
    }
    fn request(path: &Path, action: Action) -> Request {
        Request {
            path: path.to_path_buf(),
            format: Format::Json,
            create_parents: false,
            action,
        }
    }
    fn execute_request(request: &Request) -> Result<Json, StorageError> {
        execute(request, &AtomicBool::new(false))
    }
    #[test]
    fn codecs_preserve_portable_values_including_empty_arrays_and_large_integers() {
        let data = json!({"string":"quoted \" text\n\u{00e9}","yes":true,"integer":i64::MIN,"float":0.5,"empty_map":{},"empty_array":[],"nested":{"array":[1,2]}});
        for format in [Format::Json, Format::Toml, Format::Lua] {
            let source = encode(format, &data).unwrap();
            assert_eq!(
                decode(format, &source).unwrap(),
                data,
                "{format:?}: {source}"
            );
        }
    }
    #[test]
    fn watcher_checks_skip_unchanged_lua_and_bound_error_messages() {
        let source = "while true do end";
        let known = Action::Watch {
            known: Baseline::Contents(Arc::from(source)),
        };
        let result = observation(&known, Format::Lua, Some(source.into())).unwrap();
        assert_eq!(result["unchanged"], true);
        let changed = observation(
            &Action::Watch {
                known: Baseline::Unloaded,
            },
            Format::Lua,
            Some(source.into()),
        )
        .unwrap();
        assert_eq!(changed["watch_error"]["code"], "parse");
        assert_eq!(changed["baseline"], source);
        assert!(error("parse", "\u{00e9}".repeat(10000)).message.len() <= 4099);
    }
    #[test]
    fn lua_files_have_a_bounded_pure_environment_and_strict_data_values() {
        for source in [
            "while true do end",
            "return {value=io.open('x')}",
            "return {value=function() end}",
            "return {value=0/0}",
            "local x={}; x.loop=x; return x",
            "return {value={[2]='hole'}}",
        ] {
            assert!(decode(Format::Lua, source).is_err(), "{source}");
        }
        assert!(decode(Format::Json, r#"{"value":null}"#).is_err());
        assert!(decode(Format::Toml, "date = 2026-10-25").is_err());
    }
    #[test]
    fn missing_files_do_not_write_and_external_edits_block_saves() {
        let dir = directory();
        let path = dir.path().join("settings.json");
        let loaded = execute_request(&request(&path, Action::Reload)).unwrap();
        assert_eq!(loaded["missing"], true);
        assert!(!path.exists());
        let data = Arc::new(json!({"count":1}));
        execute_request(&request(
            &path,
            Action::Save {
                data: data.clone(),
                expected: Baseline::Missing,
            },
        ))
        .unwrap();
        let loaded = execute_request(&request(&path, Action::Reload)).unwrap();
        std::fs::write(&path, r#"{"count":2}"#).unwrap();
        let saved = execute_request(&request(
            &path,
            Action::Save {
                data,
                expected: Baseline::Contents(Arc::from(loaded["baseline"].as_str().unwrap())),
            },
        ));
        assert_eq!(saved.unwrap_err().code, "conflict");
        assert_eq!(std::fs::read_to_string(path).unwrap(), r#"{"count":2}"#);
    }
    #[test]
    fn saves_are_atomic_preserve_permissions_and_require_explicit_parent_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = directory();
        let path = dir.path().join("nested/settings.json");
        let mut save = request(
            &path,
            Action::Save {
                data: Arc::new(json!({"value":1})),
                expected: Baseline::Missing,
            },
        );
        assert!(execute_request(&save).is_err());
        assert!(!path.parent().unwrap().exists());
        save.create_parents = true;
        let saved = execute_request(&save).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        save.action = Action::Save {
            data: Arc::new(json!({"value":2})),
            expected: Baseline::Contents(Arc::from(saved["baseline"].as_str().unwrap())),
        };
        execute_request(&save).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
    #[test]
    fn symlinks_special_files_and_oversized_files_are_rejected() {
        let dir = directory();
        let target = dir.path().join("target.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(execute_request(&request(&link, Action::Reload)).is_err());
        assert!(execute_request(&request(dir.path(), Action::Reload)).is_err());
        std::fs::write(&target, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert_eq!(
            execute_request(&request(&target, Action::Reload))
                .unwrap_err()
                .code,
            "limit"
        );
    }
    #[test]
    fn cancellation_before_commit_leaves_the_original_file_unchanged() {
        let dir = directory();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        let save = request(
            &path,
            Action::Save {
                data: Arc::new(json!({"changed":true})),
                expected: Baseline::Contents(Arc::from("{}")),
            },
        );
        let result = execute(&save, &AtomicBool::new(true));
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }
    #[test]
    fn simultaneous_saves_to_the_same_path_cannot_both_replace_the_baseline() {
        let dir = directory();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        let jobs: Vec<_> = [1, 2]
            .into_iter()
            .map(|count| {
                let path = path.clone();
                std::thread::spawn(move || {
                    execute_request(&request(
                        &path,
                        Action::Save {
                            data: Arc::new(json!({"count":count})),
                            expected: Baseline::Contents(Arc::from("{}")),
                        },
                    ))
                })
            })
            .collect();
        let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .any(|result| result.as_ref().is_err_and(|error| error.code == "conflict"))
        );
    }
}
