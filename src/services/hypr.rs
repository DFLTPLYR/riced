//! Hyprland socket snapshot for `wayland.workspaces` /
//! `wayland.toplevels`.
//!
//! The compositor socket (`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`)
//! answers `j/workspaces`, `j/clients`, `j/activeworkspace` with JSON.
//! [`HyprCache::refresh`] runs **once per widget tick** (see
//! `handle_widget_tick`) and every widget state shares the result, so
//! N widgets never mean N socket round-trips. Unreachable socket or
//! unparseable reply degrades to empty tables — never an error, never
//! a block: reads carry a 500ms timeout.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const READ_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HyprWorkspace {
    pub id: i32,
    pub name: String,
    pub monitor: String,
    pub windows: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HyprClient {
    pub class: String,
    pub title: String,
    pub workspace: i32,
}

/// Tick-shared Hyprland snapshot. Plain owned rows; Lua sees copies.
#[derive(Debug, Default)]
pub struct HyprCache {
    pub workspaces: Vec<HyprWorkspace>,
    pub clients: Vec<HyprClient>,
    pub active_workspace: Option<i32>,
}

impl HyprCache {
    /// Re-fetch from the compositor socket, falling back to empty on
    /// any failure (non-Hypr compositors, missing socket, bad JSON).
    pub fn refresh(&mut self) {
        *self = Self::query().unwrap_or_default();
    }

    fn query() -> Option<Self> {
        let workspaces = Self::parse_workspaces(&Self::fetch("j/workspaces")?);
        let clients = Self::parse_clients(&Self::fetch("j/clients")?);
        let active_workspace = Self::fetch("j/activeworkspace")
            .as_deref()
            .map(Self::parse_active)
            .unwrap_or_default();
        Some(Self {
            workspaces,
            clients,
            active_workspace,
        })
    }

    fn socket_path() -> Option<std::path::PathBuf> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let mut path = std::path::PathBuf::from(runtime);
        path.push("hypr");
        path.push(sig);
        path.push(".socket.sock");
        Some(path)
    }

    fn fetch(command: &str) -> Option<String> {
        let path = Self::socket_path()?;
        let mut stream = UnixStream::connect(path).ok()?;
        stream.set_read_timeout(Some(READ_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(READ_TIMEOUT)).ok()?;
        stream.write_all(command.as_bytes()).ok()?;
        let mut reply = String::new();
        // The compositor closes the connection after each reply.
        stream.read_to_string(&mut reply).ok()?;
        Some(reply)
    }

    pub(crate) fn parse_workspaces(json: &str) -> Vec<HyprWorkspace> {
        let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
            return Vec::new();
        };
        list.iter()
            .map(|w| HyprWorkspace {
                id: w.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                name: w
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                monitor: w
                    .get("monitor")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                windows: w.get("windows").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            })
            .collect()
    }

    pub(crate) fn parse_clients(json: &str) -> Vec<HyprClient> {
        let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
            return Vec::new();
        };
        list.iter()
            .map(|c| HyprClient {
                class: c
                    .get("class")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                title: c
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                workspace: c
                    .get("workspace")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32,
            })
            .collect()
    }

    pub(crate) fn parse_active(json: &str) -> Option<i32> {
        serde_json::from_str::<serde_json::Value>(json)
            .ok()?
            .get("id")?
            .as_i64()
            .map(|id| id as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACES: &str = r#"[{"id":3,"name":"3","monitor":"DP-1","monitorID":0,"windows":1},{"id":5,"name":"5","monitor":"DP-1","monitorID":0,"windows":2}]"#;
    const CLIENTS: &str = r#"[{"class":"foot","title":"t","workspace":{"id":5,"name":"5"}}]"#;

    #[test]
    fn parses_workspace_and_client_rows() {
        let ws = HyprCache::parse_workspaces(WORKSPACES);
        assert_eq!(ws.len(), 2);
        assert_eq!(
            ws[1],
            HyprWorkspace {
                id: 5,
                name: "5".to_string(),
                monitor: "DP-1".to_string(),
                windows: 2,
            }
        );
        let clients = HyprCache::parse_clients(CLIENTS);
        assert_eq!(
            clients,
            vec![HyprClient {
                class: "foot".to_string(),
                title: "t".to_string(),
                workspace: 5,
            }]
        );
        assert_eq!(
            HyprCache::parse_active(r#"{"id":5,"monitor":"DP-1"}"#),
            Some(5)
        );
    }

    #[test]
    fn malformed_json_degrades_to_empty() {
        assert!(HyprCache::parse_workspaces("nope").is_empty());
        assert!(HyprCache::parse_clients("[}").is_empty());
        assert_eq!(HyprCache::parse_active("null"), None);
    }

    #[test]
    fn refresh_never_panics_without_a_socket() {
        // No live compositor is required: any failure is empty tables.
        let mut cache = HyprCache::default();
        cache.refresh();
        let _ = (cache.workspaces.len(), cache.active_workspace);
    }
}
