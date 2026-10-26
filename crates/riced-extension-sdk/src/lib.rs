//! Versioned, owned-data boundary for compiled-in riced extensions.
//! No Lua VM, UI toolkit, or asynchronous runtime is required by the SDK.
#![deny(missing_docs)]

use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Source-level SDK contract version (not a dynamic-library ABI).
pub const SDK_VERSION: u32 = 1;
/// Maximum serialized command argument/result size.
pub const MAX_COMMAND_BYTES: usize = 65_536;
/// Maximum serialized cached snapshot size.
pub const MAX_SNAPSHOT_BYTES: usize = 1_048_576;

/// Structured error exposed to Lua as `{code, message}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    /// Machine-readable error identifier.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}
impl Error {
    /// Construct an extension or host error.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Extension identity and SDK compatibility declaration.
pub struct Manifest {
    /// Unique lowercase identifier, e.g. `palette`.
    pub name: &'static str,
    /// Required source contract version.
    pub sdk_version: u32,
}

/// One service's public command surface.
pub struct Descriptor {
    /// Unique service topic; built-in topics are reserved by the host.
    pub name: &'static str,
    /// Allowed command names. Unknown commands fail before scheduling work.
    pub commands: &'static [&'static str],
}

/// Cooperative cancellation for blocking extension work.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Whether the owning app has cancelled or disappeared.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// Signal cancellation. Running work must cooperate to stop early.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Owned result of a command. The host commits snapshots only on accepted delivery.
pub struct Output {
    /// Callback result data.
    pub data: Value,
    /// Optional replacement snapshot, shared by all app instances.
    pub snapshot: Option<Value>,
}

/// A trusted native service. Commands run on bounded blocking workers.
///
/// Do not mutate published state from `command`: return a replacement snapshot
/// instead. External side effects cannot be rolled back by cancellation.
pub trait Service: Send + Sync {
    /// Declare the topic and supported commands.
    fn descriptor(&self) -> Descriptor;
    /// Initial cached state. Called once during registration, not during rendering.
    fn initial_snapshot(&self) -> Result<Value, Error>;
    /// Execute a command without access to Lua or shell state.
    fn command(&self, method: &str, args: Value, cancel: &Cancellation) -> Result<Output, Error>;
}

/// A standard Rust crate supplying one or more services.
pub trait Extension {
    /// Extension identity and compatibility.
    fn manifest(&self) -> Manifest;
    /// Construct the extension's service implementations.
    fn services(&self) -> Vec<Arc<dyn Service>>;
}

struct Entry {
    service: Arc<dyn Service>,
    commands: &'static [&'static str],
    snapshot: Mutex<Value>,
}

/// Host-owned registry. Registration is explicit and atomic per extension.
#[derive(Default)]
pub struct Registry {
    extensions: HashSet<String>,
    services: HashMap<String, Entry>,
}

fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Validate transport bounds before dispatching or accepting owned data.
pub fn validate(value: &Value, max_bytes: usize) -> Result<(), Error> {
    fn visit(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), Error> {
        *nodes += 1;
        if depth > 32 || *nodes > 16_384 {
            return Err(Error::new(
                "limit",
                "extension data exceeds depth/node limits",
            ));
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, depth + 1, nodes)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    visit(value, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, 0, &mut 0)?;
    struct Budget(usize);
    impl Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or_else(|| io::Error::other("extension data exceeds byte limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(max_bytes), value)
        .map_err(|e| Error::new("limit", e.to_string()))?;
    Ok(())
}

impl Registry {
    /// Register all services, rejecting incompatible versions and collisions.
    /// Reserved names are host globals/topics unavailable to extensions.
    pub fn register(&mut self, extension: &dyn Extension, reserved: &[&str]) -> Result<(), Error> {
        let manifest = extension.manifest();
        if manifest.sdk_version != SDK_VERSION {
            return Err(Error::new("version", "incompatible extension SDK version"));
        }
        if !identifier(manifest.name) || self.extensions.contains(manifest.name) {
            return Err(Error::new(
                "registration",
                "invalid or duplicate extension name",
            ));
        }
        let mut entries = HashMap::new();
        for service in extension.services() {
            let descriptor = service.descriptor();
            if !identifier(descriptor.name)
                || reserved.contains(&descriptor.name)
                || self.services.contains_key(descriptor.name)
                || entries.contains_key(descriptor.name)
            {
                return Err(Error::new(
                    "registration",
                    "invalid, reserved, or duplicate service name",
                ));
            }
            let mut commands = HashSet::new();
            if descriptor.commands.len() > 64
                || descriptor
                    .commands
                    .iter()
                    .any(|name| !identifier(name) || !commands.insert(*name))
            {
                return Err(Error::new(
                    "registration",
                    "invalid or duplicate command name",
                ));
            }
            let snapshot = service.initial_snapshot()?;
            validate(&snapshot, MAX_SNAPSHOT_BYTES)?;
            entries.insert(
                descriptor.name.to_owned(),
                Entry {
                    service,
                    commands: descriptor.commands,
                    snapshot: Mutex::new(snapshot),
                },
            );
        }
        if entries.is_empty() || self.services.len() + entries.len() > 64 {
            return Err(Error::new(
                "registration",
                "extension requires services; maximum is 64",
            ));
        }
        self.services.extend(entries);
        self.extensions.insert(manifest.name.to_owned());
        Ok(())
    }
    /// Whether a service is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }
    /// Clone an immutable snapshot for publication to an app.
    pub fn snapshot(&self, name: &str) -> Result<Value, Error> {
        let entry = self.entry(name)?;
        entry
            .snapshot
            .lock()
            .map(|value| value.clone())
            .map_err(|_| Error::new("state", "extension snapshot lock poisoned"))
    }
    fn entry(&self, name: &str) -> Result<&Entry, Error> {
        self.services
            .get(name)
            .ok_or_else(|| Error::new("service", "unknown extension service"))
    }
    /// Validate and capture a command for later worker execution.
    pub fn request(&self, name: &str, method: &str, args: Value) -> Result<Request, Error> {
        let entry = self.entry(name)?;
        if !entry.commands.contains(&method) {
            return Err(Error::new("command", "unknown extension command"));
        }
        validate(&args, MAX_COMMAND_BYTES)?;
        Ok(Request {
            service: Arc::clone(&entry.service),
            method: method.to_owned(),
            args,
        })
    }
    /// Replace published state after the host accepts a live command delivery.
    pub fn publish(&self, name: &str, snapshot: Value) -> Result<(), Error> {
        validate(&snapshot, MAX_SNAPSHOT_BYTES)?;
        *self
            .entry(name)?
            .snapshot
            .lock()
            .map_err(|_| Error::new("state", "extension snapshot lock poisoned"))? = snapshot;
        Ok(())
    }
}

/// Owned, validated command ready for a worker. Contains no VM references.
#[derive(Clone)]
pub struct Request {
    service: Arc<dyn Service>,
    method: String,
    args: Value,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
impl Request {
    /// Execute and validate both result and optional replacement state.
    pub fn execute(self, cancel: &Cancellation) -> Result<Output, Error> {
        if cancel.is_cancelled() {
            return Err(Error::new("cancelled", "extension command cancelled"));
        }
        let output = self.service.command(&self.method, self.args, cancel)?;
        if cancel.is_cancelled() {
            return Err(Error::new("cancelled", "extension command cancelled"));
        }
        validate(&output.data, MAX_COMMAND_BYTES)?;
        if let Some(snapshot) = &output.snapshot {
            validate(snapshot, MAX_SNAPSHOT_BYTES)?;
        }
        Ok(output)
    }
}
