//! Native Wayland toplevels via `ext-foreign-toplevel-list-v1`.
//!
//! A dedicated connection + listener thread feeds snapshots: iced owns
//! the main connection and won't pump our globals, and a second
//! connection is the established pattern here (see `run_daemon`).
//! The thread blocks on the compositor socket — no polling, no CPU
//! spin — and widget ticks read the latest snapshot. Compositors
//! without the protocol (or no Wayland session at all) yield empty
//! tables; only `{app_id, title}` metadata crosses into Lua.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};

/// One mapped toplevel: protocol metadata only, no handles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Toplevel {
    pub app_id: String,
    pub title: String,
}

/// Tick-shared snapshot. `spawn()` starts the listener thread (once);
/// `snapshot()` clones the latest rows, sorted for deterministic Lua
/// order. `from_rows` is the thread-free constructor for tests.
#[derive(Debug, Default)]
pub struct ToplevelCache {
    rows: Arc<Mutex<Vec<Toplevel>>>,
}

impl ToplevelCache {
    pub fn spawn() -> Self {
        let cache = Self::default();
        std::thread::Builder::new()
            .name("riced-toplevels".to_string())
            .spawn({
                let rows = cache.rows.clone();
                move || listen(rows)
            })
            .ok();
        cache
    }

    #[cfg(test)]
    pub fn from_rows(rows: Vec<Toplevel>) -> Self {
        Self {
            rows: Arc::new(Mutex::new(rows)),
        }
    }

    pub fn snapshot(&self) -> Vec<Toplevel> {
        self.rows
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default()
    }
}

/// Listener state: pending rows keyed by protocol object id, published
/// to `shared` on every event batch.
struct Listener {
    pending: HashMap<u32, Toplevel>,
    shared: Arc<Mutex<Vec<Toplevel>>>,
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
        if let Ok(mut shared) = self.shared.lock() {
            *shared = rows;
        }
    }
}

/// Run the listener to socket death: connect fails (no session) or
/// the protocol is unadvertised, and this returns with the snapshot
/// left empty. Late-advertised globals are not picked up (bind once).
fn listen(shared: Arc<Mutex<Vec<Toplevel>>>) {
    let Ok(conn) = Connection::connect_to_env() else {
        return;
    };
    let mut listener = Listener {
        pending: HashMap::new(),
        shared,
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
    while queue.blocking_dispatch(&mut listener).is_ok() {}
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
        _: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
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
            ext_foreign_toplevel_list_v1::Event::Finished => {}
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_accumulates_sorted_rows_and_drops_closed() {
        let shared = Arc::new(Mutex::new(Vec::new()));
        let mut listener = Listener {
            pending: HashMap::new(),
            shared: shared.clone(),
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
        let cache = ToplevelCache::spawn();
        let _ = cache.snapshot();
    }
}
