//! Native Wayland toplevels via `ext-foreign-toplevel-list-v1`.
//!
//! A dedicated connection + listener thread feeds snapshots: iced owns the
//! main connection and won't pump our globals, so a second connection is the
//! established pattern. The thread blocks on the compositor socket — no
//! polling, no CPU spin — and reports changes through a host-provided wakeup.
//! Compositors without the protocol (or no Wayland session at all) yield empty
//! tables; only `{app_id, title}` metadata crosses into Lua.
//!
//! The crate exposes the listener as a [`Service`] (`toplevels`, no commands)
//! for the compiled-in and supervised-process backends, and the host's
//! `services::toplevels` re-exports [`ToplevelCache`] so there is one
//! implementation.
#![deny(missing_docs)]
use riced_extension_sdk::{
    Cancellation, Descriptor, Error, Extension, Manifest, Output, SDK_VERSION, Service,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wayland_client::backend::ObjectData;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};

/// Host wakeup: called from the listener thread when the snapshot changes.
/// Must be cheap, non-blocking, and safe to call from any thread.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// One mapped toplevel: protocol metadata only, no handles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Toplevel {
    /// Compositor-reported application id.
    pub app_id: String,
    /// Compositor-reported window title.
    pub title: String,
}

/// Event-shared snapshot. [`ToplevelCache::spawn`] starts the listener thread
/// (once); [`ToplevelCache::snapshot`] clones the latest rows, sorted for
/// deterministic Lua order. [`ToplevelCache::from_rows`] is the thread-free
/// constructor for tests and fixtures.
#[derive(Debug, Default)]
pub struct ToplevelCache {
    rows: Arc<Mutex<Vec<Toplevel>>>,
}

impl ToplevelCache {
    /// Start the listener thread with a host wakeup. Returns immediately.
    pub fn spawn(notify: Notify) -> Self {
        let cache = Self::default();
        std::thread::Builder::new()
            .name("riced-toplevels".to_string())
            .spawn({
                let rows = cache.rows.clone();
                move || listen(rows, notify)
            })
            .ok();
        cache
    }

    /// Thread-free constructor for tests and fixtures.
    pub fn from_rows(rows: Vec<Toplevel>) -> Self {
        Self {
            rows: Arc::new(Mutex::new(rows)),
        }
    }

    /// Latest rows, sorted by `(app_id, title)`.
    pub fn snapshot(&self) -> Vec<Toplevel> {
        self.rows
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default()
    }

    fn json(&self) -> Value {
        let toplevels: Vec<Value> = self
            .snapshot()
            .iter()
            .map(|tl| json!({"app_id": tl.app_id, "title": tl.title}))
            .collect();
        json!({"toplevels": toplevels})
    }
}

/// Listener state: pending rows keyed by protocol object id, published
/// to `shared` on change, with a coalesced host wakeup.
struct Listener {
    pending: HashMap<u32, Toplevel>,
    shared: Arc<Mutex<Vec<Toplevel>>>,
    notify: Notify,
    /// Set when the compositor sends `finished` (list revoked): the
    /// dispatch loop exits and the snapshot clears.
    finished: bool,
}

impl Listener {
    fn upsert_title(&mut self, id: u32, title: String) {
        self.pending.entry(id).or_default().title = title;
        self.publish();
    }

    fn upsert_app_id(&mut self, id: u32, app_id: String) {
        self.pending.entry(id).or_default().app_id = app_id;
        self.publish();
    }

    fn remove(&mut self, id: u32) {
        self.pending.remove(&id);
        self.publish();
    }

    fn publish(&self) {
        let mut rows: Vec<Toplevel> = self.pending.values().cloned().collect();
        rows.sort_by(|a, b| (&a.app_id, &a.title).cmp(&(&b.app_id, &b.title)));
        if let Ok(mut shared) = self.shared.lock()
            && *shared != rows
        {
            *shared = rows;
            (self.notify)();
        }
    }
}

/// Run the listener to socket death: connect fails (no session) or
/// the protocol is unadvertised, and this returns with the snapshot
/// left empty. Late-advertised globals are not picked up (bind once).
fn listen(shared: Arc<Mutex<Vec<Toplevel>>>, notify: Notify) {
    let Ok(conn) = Connection::connect_to_env() else {
        return;
    };
    let mut listener = Listener {
        pending: HashMap::new(),
        shared,
        notify,
        finished: false,
    };
    let Ok((globals, mut queue)) = registry_queue_init::<Listener>(&conn) else {
        return;
    };
    let qh = queue.handle();
    if globals
        .bind::<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, _, _>(&qh, 1..=1, ())
        .is_err()
    {
        return;
    }
    while queue.blocking_dispatch(&mut listener).is_ok() {
        if listener.finished {
            break;
        }
    }
    listener.pending.clear();
    listener.publish();
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Listener {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } => {
                state
                    .pending
                    .entry(toplevel.id().protocol_id())
                    .or_default();
                state.publish();
            }
            // Revoked: no more events will ever arrive. Destroy the
            // object per spec and clear back to the empty degrade.
            ext_foreign_toplevel_list_v1::Event::Finished => {
                proxy.destroy();
                state.pending.clear();
                state.publish();
                state.finished = true;
            }
            _ => {}
        }
    }

    /// Opcode 0 (`toplevel`) carries the new handle object; every other
    /// event on this interface is data-only.
    fn event_created_child(opcode: u16, qhandle: &QueueHandle<Self>) -> Arc<dyn ObjectData> {
        assert_eq!(opcode, 0, "unexpected child-creating opcode {opcode}");
        qhandle.make_data::<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()>(())
    }
}

impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            ext_foreign_toplevel_handle_v1::Event::Title { title } => state.upsert_title(id, title),
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                state.upsert_app_id(id, app_id)
            }
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                proxy.destroy();
                state.remove(id);
            }
            // `Done` closes a state batch; every mutation above already
            // republished, so there is nothing left to do.
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// The `toplevels` service: the listener's snapshot, polled by the host.
pub struct Toplevels {
    cache: ToplevelCache,
    last: Mutex<Value>,
}

impl Default for Toplevels {
    fn default() -> Self {
        Self::new()
    }
}

impl Toplevels {
    /// Start a listener and wrap it as a service.
    pub fn new() -> Self {
        Self {
            cache: ToplevelCache::spawn(Arc::new(|| {})),
            last: Mutex::new(json!({"toplevels": []})),
        }
    }
}

impl Service for Toplevels {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: "toplevels",
            commands: &[],
        }
    }
    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(json!({"toplevels": []}))
    }
    fn refresh(&self) -> Result<Option<Value>, Error> {
        let value = self.cache.json();
        let mut last = self
            .last
            .lock()
            .map_err(|_| Error::new("state", "toplevels lock poisoned"))?;
        if *last == value {
            return Ok(None);
        }
        *last = value.clone();
        Ok(Some(value))
    }
    fn command(&self, _: &str, _: Value, _: &Cancellation) -> Result<Output, Error> {
        Err(Error::new("command", "toplevels has no commands"))
    }
}

/// Extension identity for the `toplevels` service.
pub struct WaylandToplevels;
impl Extension for WaylandToplevels {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "wayland_toplevels",
            sdk_version: SDK_VERSION,
        }
    }
    fn services(&self) -> Vec<Arc<dyn Service>> {
        vec![Arc::new(Toplevels::new())]
    }
}

/// Register this extension with a host registry.
///
/// Generated shells call this function from their `main`; the reserved list
/// comes from `riced::builtin_service_topics()`.
pub fn register(
    registry: &mut riced_extension_sdk::Registry,
    reserved: &[&str],
) -> Result<(), Error> {
    registry.register(&WaylandToplevels, reserved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop() -> Notify {
        Arc::new(|| {})
    }

    #[test]
    fn duplicate_toplevel_events_do_not_wake_consumers() {
        let calls = Arc::new(Mutex::new(0usize));
        let counted = {
            let calls = calls.clone();
            Arc::new(move || *calls.lock().unwrap() += 1)
        };
        let mut listener = Listener {
            pending: HashMap::new(),
            shared: Default::default(),
            notify: counted,
            finished: false,
        };
        listener.upsert_title(7, "shell".into());
        assert_eq!(*calls.lock().unwrap(), 1);
        // Same value again: no wakeup.
        listener.upsert_title(7, "shell".into());
        assert_eq!(*calls.lock().unwrap(), 1);
        listener.remove(7);
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    #[test]
    fn listener_accumulates_sorted_rows_and_drops_closed() {
        let shared = Arc::new(Mutex::new(Vec::new()));
        let mut listener = Listener {
            pending: HashMap::new(),
            shared: shared.clone(),
            notify: noop(),
            finished: false,
        };
        listener.upsert_title(7, "shell".to_string());
        listener.upsert_app_id(7, "foot".to_string());
        listener.upsert_app_id(9, "aaa".to_string());
        listener.upsert_title(9, "z".to_string());
        assert_eq!(
            *shared.lock().unwrap(),
            vec![
                Toplevel {
                    app_id: "aaa".to_string(),
                    title: "z".to_string(),
                },
                Toplevel {
                    app_id: "foot".to_string(),
                    title: "shell".to_string(),
                },
            ]
        );
        listener.remove(9);
        assert_eq!(
            *shared.lock().unwrap(),
            vec![Toplevel {
                app_id: "foot".to_string(),
                title: "shell".to_string(),
            }]
        );
    }

    #[test]
    fn spawn_without_a_session_stays_empty() {
        // No compositor is required: failures leave empty tables.
        let cache = ToplevelCache::spawn(noop());
        let _ = cache.snapshot();
    }

    #[test]
    fn service_publishes_only_on_change() {
        let service = Toplevels {
            cache: ToplevelCache::from_rows(vec![Toplevel {
                app_id: "foot".into(),
                title: "shell".into(),
            }]),
            last: Mutex::new(json!({"toplevels": []})),
        };
        let snapshot = service.refresh().unwrap().unwrap();
        assert_eq!(snapshot["toplevels"][0]["app_id"], "foot");
        assert_eq!(snapshot["toplevels"][0]["title"], "shell");
        // Unchanged: no repeat publication.
        assert!(service.refresh().unwrap().is_none());
        assert_eq!(
            service
                .command("anything", Value::Null, &Cancellation::default())
                .err()
                .unwrap()
                .code,
            "command"
        );
    }
}
