//! Native Wayland workspaces via `ext-workspace-v1`.
//!
//! Same shape as [`super::toplevels`]: a dedicated connection +
//! listener thread (blocking dispatch, no polling) whose snapshots
//! widget ticks clone. Compositors without the protocol (Sway, Mutter,
//! KWin, …) yield empty tables. Published rows are `{name, monitor,
//! active}` metadata only — no activate/deactivate/remove/create
//! requests are ever sent, so this stays a read-only overview.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use wayland_client::backend::ObjectData;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1, ext_workspace_handle_v1, ext_workspace_manager_v1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

/// One workspace: protocol metadata only, no handles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub monitor: String,
    pub active: bool,
}

/// Tick-shared snapshot. `spawn()` starts the listener thread (once);
/// `snapshot()` clones the latest rows, sorted for deterministic Lua
/// order. `from_rows` is the thread-free constructor for tests.
#[derive(Debug, Default)]
pub struct WorkspaceCache {
    rows: Arc<Mutex<Vec<Workspace>>>,
}

impl WorkspaceCache {
    pub fn spawn() -> Self {
        let cache = Self::default();
        std::thread::Builder::new()
            .name("riced-workspaces".to_string())
            .spawn({
                let rows = cache.rows.clone();
                move || listen(rows)
            })
            .ok();
        cache
    }

    #[cfg(test)]
    pub fn from_rows(rows: Vec<Workspace>) -> Self {
        Self {
            rows: Arc::new(Mutex::new(rows)),
        }
    }

    pub fn snapshot(&self) -> Vec<Workspace> {
        self.rows
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default()
    }
}

/// Per-workspace accumulation: protocol ids key everything (stable for
/// the object's lifetime; the `workspace` event always resets the row
/// first, so id reuse across objects is safe).
#[derive(Debug, Default)]
struct WsRow {
    name: String,
    active: bool,
    group: Option<u32>,
}

struct Listener {
    workspaces: HashMap<u32, WsRow>,
    /// Group id → member workspace protocol ids.
    groups: HashMap<u32, HashSet<u32>>,
    /// Group id → output protocol ids currently assigned.
    group_outputs: HashMap<u32, HashSet<u32>>,
    /// Output protocol id → connector name (`wl_output.name`).
    outputs: HashMap<u32, String>,
    /// Output protocol id → logical name (`xdg-output name`, preferred:
    /// `wl_output.name` is duplicated on some compositors).
    xdg_names: HashMap<u32, String>,
    /// xdg-output protocol id → wl_output protocol id it describes.
    xdg_output_of: HashMap<u32, u32>,
    /// Kept alive: dropping proxies may release the objects.
    xdg_outputs: HashMap<u32, zxdg_output_v1::ZxdgOutputV1>,
    /// Kept alive: xdg-outputs die with the manager otherwise.
    _xdg_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    shared: Arc<Mutex<Vec<Workspace>>>,
    /// Set on manager `finished` (revoked): loop exits, snapshot clears.
    finished: bool,
    debug: bool,
}

/// `RICED_DEBUG=workspaces` (comma-separated with other scopes):
/// stderr trace of every protocol event plus each published snapshot.
fn debug_enabled() -> bool {
    std::env::var("RICED_DEBUG")
        .map(|v| v.split(',').any(|s| s.trim() == "workspaces"))
        .unwrap_or(false)
}

macro_rules! dtrace {
    ($st:expr, $($arg:tt)*) => {
        if $st.debug {
            eprintln!("riced(workspaces): {}", format!($($arg)*));
        }
    };
}

impl Listener {
    /// Bind one output global and attach its xdg-output so logical
    /// names resolve even where `wl_output.name` is wrong.
    fn bind_output(&mut self, output: wl_output::WlOutput, qh: &QueueHandle<Self>) {
        let oid = output.id().protocol_id();
        if let Some(manager) = &self._xdg_manager {
            let xdg = manager.get_xdg_output(&output, qh, ());
            self.xdg_output_of.insert(xdg.id().protocol_id(), oid);
            self.xdg_outputs.insert(oid, xdg);
        }
    }

    fn output_name(&self, id: u32) -> Option<&str> {
        self.xdg_names
            .get(&id)
            .or_else(|| self.outputs.get(&id))
            .map(String::as_str)
    }

    fn monitor_of(&self, group: Option<u32>) -> String {
        let mut names: Vec<&str> = group
            .and_then(|g| self.group_outputs.get(&g))
            .into_iter()
            .flatten()
            .filter_map(|id| self.output_name(*id))
            .collect();
        names.sort_unstable();
        names.dedup();
        names.join(",")
    }

    fn publish(&self) {
        let mut rows: Vec<Workspace> = self
            .workspaces
            .values()
            .map(|row| Workspace {
                name: row.name.clone(),
                monitor: self.monitor_of(row.group),
                active: row.active,
            })
            .collect();
        rows.sort_by(|a, b| (&a.monitor, &a.name).cmp(&(&b.monitor, &b.name)));
        if self.debug {
            let summary: Vec<String> = rows
                .iter()
                .map(|r| format!("{}!{}@{}", r.name, r.active as u8, r.monitor))
                .collect();
            eprintln!("riced(workspaces): snapshot [{}]", summary.join(" "));
        }
        if let Ok(mut shared) = self.shared.lock() {
            *shared = rows;
        }
    }
}

/// Run the listener to socket death: connect fails (no session) or the
/// protocol is unadvertised, and this returns with the snapshot left
/// empty. Late-advertised globals are not picked up (bind once,
/// except hotplugged outputs, which bind on registry arrival).
fn listen(shared: Arc<Mutex<Vec<Workspace>>>) {
    let debug = debug_enabled();
    if debug {
        eprintln!("riced(workspaces): listener starting");
    }
    let Ok(conn) = Connection::connect_to_env() else {
        if debug {
            eprintln!("riced(workspaces): no Wayland connection");
        }
        return;
    };
    let mut listener = Listener {
        workspaces: HashMap::new(),
        groups: HashMap::new(),
        group_outputs: HashMap::new(),
        outputs: HashMap::new(),
        xdg_names: HashMap::new(),
        xdg_output_of: HashMap::new(),
        xdg_outputs: HashMap::new(),
        _xdg_manager: None,
        shared,
        finished: false,
        debug,
    };
    let Ok((globals, mut queue)) = registry_queue_init::<Listener>(&conn) else {
        return;
    };
    let qh = queue.handle();
    if globals
        .bind::<ext_workspace_manager_v1::ExtWorkspaceManagerV1, _, _>(&qh, 1..=1, ())
        .is_err()
    {
        if debug {
            eprintln!("riced(workspaces): ext-workspace-v1 not advertised");
        }
        return;
    }
    if debug {
        eprintln!("riced(workspaces): manager bound");
    }
    listener._xdg_manager = globals
        .bind::<zxdg_output_manager_v1::ZxdgOutputManagerV1, _, _>(&qh, 1..=3, ())
        .ok();
    if debug && listener._xdg_manager.is_none() {
        eprintln!("riced(workspaces): xdg-output not advertised; wl names only");
    }
    // Outputs known at startup (hotplugged ones bind on registry arrival).
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" && global.version >= 2 {
            let version = global.version.min(4);
            if let Some(output) = globals
                .bind::<wl_output::WlOutput, _, _>(&qh, version..=version, ())
                .ok()
            {
                listener.bind_output(output, &qh);
            }
        }
    }
    while queue.blocking_dispatch(&mut listener).is_ok() {
        if listener.finished {
            break;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Listener {
    fn event(
        state: &mut Self,
        proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        // Hotplugged outputs: bind so `output_enter` ids stay known.
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_output"
            && version >= 2
        {
            let version = version.min(4);
            let output = proxy.bind::<wl_output::WlOutput, (), Self>(name, version, qhandle, ());
            state.bind_output(output, qhandle);
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            let id = proxy.id().protocol_id();
            dtrace!(state, "output {id}: name {name:?}");
            state.outputs.insert(id, name);
            state.publish();
        }
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for Listener {
    fn event(
        _: &mut Self,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        _: zxdg_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The manager is event-free; outputs arrive via `zxdg_output_v1`.
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::Name { name } = event
            && let Some(oid) = state.xdg_output_of.get(&proxy.id().protocol_id())
        {
            dtrace!(state, "output {oid}: xdg-name {name:?}");
            state.xdg_names.insert(*oid, name);
            state.publish();
        }
    }
}

impl Dispatch<ext_workspace_manager_v1::ExtWorkspaceManagerV1, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_workspace_manager_v1::Event::WorkspaceGroup { workspace_group } => {
                let id = workspace_group.id().protocol_id();
                dtrace!(state, "new group {id}");
                state.groups.entry(id).or_default();
                state.group_outputs.entry(id).or_default();
                state.publish();
            }
            ext_workspace_manager_v1::Event::Workspace { workspace } => {
                let id = workspace.id().protocol_id();
                dtrace!(state, "new workspace {id}");
                state.workspaces.insert(id, WsRow::default());
                state.publish();
            }
            // Revoked: the server destroys the object right after.
            // Clear back to the empty degrade and exit the loop.
            ext_workspace_manager_v1::Event::Finished => {
                state.workspaces.clear();
                state.publish();
                state.finished = true;
            }
            _ => {}
        }
        let _ = proxy;
    }

    /// Opcode 0 (`workspace_group`) and 1 (`workspace`) carry new ids.
    fn event_created_child(opcode: u16, qhandle: &QueueHandle<Self>) -> Arc<dyn ObjectData> {
        match opcode {
            0 => qhandle
                .make_data::<ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()>(()),
            1 => qhandle.make_data::<ext_workspace_handle_v1::ExtWorkspaceHandleV1, ()>(()),
            _ => unreachable!("unexpected child-creating opcode {opcode}"),
        }
    }
}

impl Dispatch<ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            ext_workspace_group_handle_v1::Event::OutputEnter { output } => {
                let out = output.id().protocol_id();
                dtrace!(state, "group {id}: output_enter {out}");
                state.group_outputs.entry(id).or_default().insert(out);
                state.publish();
            }
            ext_workspace_group_handle_v1::Event::OutputLeave { output } => {
                let out = output.id().protocol_id();
                dtrace!(state, "group {id}: output_leave {out}");
                if let Some(outputs) = state.group_outputs.get_mut(&id) {
                    outputs.remove(&out);
                }
                state.publish();
            }
            ext_workspace_group_handle_v1::Event::WorkspaceEnter { workspace } => {
                let ws = workspace.id().protocol_id();
                dtrace!(state, "group {id}: workspace_enter {ws}");
                state.groups.entry(id).or_default().insert(ws);
                if let Some(row) = state.workspaces.get_mut(&ws) {
                    row.group = Some(id);
                }
                state.publish();
            }
            ext_workspace_group_handle_v1::Event::WorkspaceLeave { workspace } => {
                let ws = workspace.id().protocol_id();
                dtrace!(state, "group {id}: workspace_leave {ws}");
                if let Some(members) = state.groups.get_mut(&id) {
                    members.remove(&ws);
                }
                if let Some(row) = state.workspaces.get_mut(&ws) {
                    row.group = None;
                }
                state.publish();
            }
            // Spec guarantees all members left via `workspace_leave`
            // first; drop the group shells defensively.
            ext_workspace_group_handle_v1::Event::Removed => {
                dtrace!(state, "group {id}: removed");
                proxy.destroy();
                state.groups.remove(&id);
                state.group_outputs.remove(&id);
                state.publish();
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_workspace_handle_v1::ExtWorkspaceHandleV1, ()> for Listener {
    fn event(
        state: &mut Self,
        proxy: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            ext_workspace_handle_v1::Event::Name { name } => {
                dtrace!(state, "ws {id}: name {name:?}");
                state.workspaces.entry(id).or_default().name = name;
                state.publish();
            }
            ext_workspace_handle_v1::Event::State { state: bits } => {
                // Bit 1 is `active`; anything else is ignored. Combined
                // flags arrive as `Unknown`, hence the raw bit test.
                let raw = match bits {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(raw) => raw,
                };
                dtrace!(state, "ws {id}: state {raw:#x}");
                state.workspaces.entry(id).or_default().active = raw & 1 != 0;
                state.publish();
            }
            // Spec guarantees removal only while unassigned; drop the
            // row and its group link defensively.
            ext_workspace_handle_v1::Event::Removed => {
                dtrace!(state, "ws {id}: removed");
                proxy.destroy();
                if let Some(row) = state.workspaces.remove(&id)
                    && let Some(group) = row.group
                    && let Some(members) = state.groups.get_mut(&group)
                {
                    members.remove(&id);
                }
                state.publish();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener() -> (Listener, Arc<Mutex<Vec<Workspace>>>) {
        let shared = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener {
            workspaces: HashMap::new(),
            groups: HashMap::new(),
            group_outputs: HashMap::new(),
            outputs: HashMap::from([(10, "DP-1".to_string()), (11, "HDMI-1".to_string())]),
            xdg_names: HashMap::new(),
            xdg_output_of: HashMap::new(),
            xdg_outputs: HashMap::new(),
            _xdg_manager: None,
            shared: shared.clone(),
            finished: false,
            debug: false,
        };
        (listener, shared)
    }

    #[test]
    fn rows_carry_name_monitor_and_active() {
        let (mut st, shared) = listener();
        st.groups.insert(1, HashSet::from([100, 101]));
        st.group_outputs.insert(1, HashSet::from([10]));
        st.workspaces.insert(
            100,
            WsRow {
                name: "code".to_string(),
                active: true,
                group: Some(1),
            },
        );
        st.workspaces.insert(
            101,
            WsRow {
                name: "web".to_string(),
                active: false,
                group: Some(1),
            },
        );
        st.publish();
        assert_eq!(
            *shared.lock().unwrap(),
            vec![
                Workspace {
                    name: "code".to_string(),
                    monitor: "DP-1".to_string(),
                    active: true,
                },
                Workspace {
                    name: "web".to_string(),
                    monitor: "DP-1".to_string(),
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn unknown_outputs_degrade_to_empty_monitor() {
        let (mut st, shared) = listener();
        st.workspaces.insert(
            100,
            WsRow {
                name: "code".to_string(),
                active: false,
                group: None,
            },
        );
        st.publish();
        assert_eq!(
            *shared.lock().unwrap(),
            vec![Workspace {
                name: "code".to_string(),
                monitor: "".to_string(),
                active: false,
            }]
        );
    }

    #[test]
    fn spawn_without_a_session_stays_empty() {
        // No compositor is required: failures leave empty tables.
        let cache = WorkspaceCache::spawn();
        let _ = cache.snapshot();
    }
}
