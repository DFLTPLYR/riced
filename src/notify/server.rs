//! D-Bus freedesktop Notifications server (mako replacement).
//!
//! Lives in an iced subscription stream: owns the session-bus
//! connection for the app lifetime and forwards arrivals/closes as
//! [`Plant::Notify`] messages. If the name is taken (mako/dunst
//! running) or no bus exists, it logs once and the future ends — the
//! shell runs fine without D-Bus.
//!
//! Claims `body` + `actions`. Image hints (`image-data`,
//! `image-path`, icon paths, freedesktop theme names) decode without
//! claiming `icon-static`: senders attach them regardless, and names
//! that resolve to nothing simply render text-only. Action clicks and
//! local dismissals emit `ActionInvoked` / `NotificationClosed` back
//! over the stored connection (see [`emit_action_invoked`],
//! [`emit_closed`).
//!
//! Icon theme lookup follows the freedesktop layout (`$XDG_DATA_HOME`
//! and `$XDG_DATA_DIRS` `icons/` trees, legacy `~/.icons`, legacy
//! `/usr/share/pixmaps`), every theme with `hicolor` last as the
//! mandated fallback. SVG entries rasterize via resvg (no system
//! fonts — text inside icons may not render, which is fine for glyphs).

use super::images::image_of;
use crate::shell::{NotifyEvent, Plant};
use iced::{Task as Command, futures::channel::mpsc};
use std::collections::HashMap;
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicU32, Ordering},
};
use zbus::{interface, object_server::SignalEmitter};

/// Hard ceiling for client-requested timeouts (5 minutes); larger
/// values clamp. `<= 0` means server default.
const MAX_TIMEOUT_MS: u64 = 300_000;

/// Live bus connection for emitting `ActionInvoked` /
/// `NotificationClosed` from message handlers (which can't await).
/// Set once by [`serve`]; `None` when the server never came up.
static DBUS_CONN: LazyLock<Mutex<Option<zbus::Connection>>> = LazyLock::new(|| Mutex::new(None));

/// Emit `ActionInvoked(id, key)` as a fire-and-forget command.
pub(crate) fn emit_action_invoked(id: u32, key: String) -> Command<Plant> {
    Command::perform(
        async move {
            if let Some(conn) = DBUS_CONN.lock().ok().and_then(|c| c.clone()) {
                let _ = conn
                    .emit_signal(
                        None::<()>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "ActionInvoked",
                        &(id, key),
                    )
                    .await;
            }
            Plant::Tend
        },
        |m| m,
    )
}

/// Emit `NotificationClosed(id, reason)` as a fire-and-forget command.
/// Reasons: 1 expired, 2 dismissed by user, 3 closed by peer call.
pub(crate) fn emit_closed(id: u32, reason: u32) -> Command<Plant> {
    Command::perform(
        async move {
            if let Some(conn) = DBUS_CONN.lock().ok().and_then(|c| c.clone()) {
                let _ = conn
                    .emit_signal(
                        None::<()>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "NotificationClosed",
                        &(id, reason),
                    )
                    .await;
            }
            Plant::Tend
        },
        |m| m,
    )
}

/// D-Bus `actions` array is flat `[key, label, ...]`; pair it up,
/// dropping a dangling tail.
fn parse_actions(flat: Vec<String>) -> Vec<(String, String)> {
    flat.chunks_exact(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

struct Server {
    tx: mpsc::Sender<Plant>,
    next_id: AtomicU32,
    default_ms: u64,
}

fn urgency_of(hints: &HashMap<String, zbus::zvariant::OwnedValue>) -> u8 {
    hints
        .get("urgency")
        .and_then(|v| u8::try_from(v).ok())
        .unwrap_or(1)
        .min(2)
}

#[interface(name = "org.freedesktop.Notifications")]
#[allow(clippy::too_many_arguments)]
impl Server {
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            // IDs start at 1; 0 is reserved as "assign me" upstream.
            self.next_id.fetch_add(1, Ordering::Relaxed).max(1)
        };
        let timeout_ms = (expire_timeout > 0).then(|| (expire_timeout as u64).min(MAX_TIMEOUT_MS));
        let msg = Plant::Notify(NotifyEvent::Arrived(
            crate::shell::screens::notification::Notification::from_dbus(
                crate::shell::screens::notification::Incoming {
                    id,
                    app: app_name,
                    icon: app_icon.clone(),
                    title: summary,
                    body,
                    actions: parse_actions(actions),
                    image: image_of(&hints, &app_icon),
                    urgency: urgency_of(&hints),
                    timeout_ms,
                },
                self.default_ms,
            ),
        ));
        // Channel full/closed: drop rather than stall the bus caller.
        let _ = self.tx.clone().try_send(msg);
        Ok(id)
    }

    async fn close_notification(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        id: u32,
    ) -> zbus::fdo::Result<()> {
        // Peer-initiated close: the signal below is the acknowledgment,
        // so forward a silent drop (no second signal from the app).
        let _ = self
            .tx
            .clone()
            .try_send(Plant::Notify(NotifyEvent::PeerClosed(id)));
        emitter.notification_closed(id, 3).await?;
        Ok(())
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_string(), "actions".to_string()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "riced".to_string(),
            "riced".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

/// Serve forever (or until the bus/name is unavailable). Entry point
/// for the iced subscription stream in `Plots::subscription`
/// (`default_ms` threads the configured timeout through, since the
/// stream builder is a bare fn pointer).
pub async fn serve(mut tx: mpsc::Sender<Plant>, default_ms: u64) {
    let server = Server {
        tx: tx.clone(),
        next_id: AtomicU32::new(1),
        default_ms,
    };
    let built = zbus::connection::Builder::session()
        .and_then(|b| b.name("org.freedesktop.Notifications"))
        .and_then(|b| b.serve_at("/org/freedesktop/Notifications", server));
    let _conn = match built {
        Ok(b) => match b.build().await {
            Ok(conn) => {
                // Stash for fire-and-forget signal emission from message
                // handlers (which can't await). Best effort only.
                if let Ok(mut slot) = DBUS_CONN.lock() {
                    *slot = Some(conn.clone());
                }
                conn
            }
            Err(e) => {
                eprintln!("riced: notifications: D-Bus unavailable ({e}), continuing without it");
                return;
            }
        },
        Err(e) => {
            eprintln!("riced: notifications: D-Bus unavailable ({e}), continuing without it");
            return;
        }
    };
    let _ = tx
        .try_send(Plant::Notify(NotifyEvent::DBusUp))
        .map_err(|_| ());
    // Park: the connection serves for the app lifetime. Dropping (on
    // shutdown) releases the name.
    std::future::pending::<()>().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_actions_pairs_keys_and_labels() {
        assert!(parse_actions(vec![]).is_empty());
        assert_eq!(
            parse_actions(vec!["default".to_string(), "Activate".to_string()]),
            vec![("default".to_string(), "Activate".to_string())]
        );
        // Dangling tail is dropped, never half-paired.
        assert_eq!(
            parse_actions(vec![
                "a".to_string(),
                "A".to_string(),
                "dangling".to_string()
            ]),
            vec![("a".to_string(), "A".to_string())]
        );
    }
}
