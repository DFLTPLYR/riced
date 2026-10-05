//! D-Bus freedesktop Notifications server (mako replacement, v1).
//!
//! Lives in an iced subscription stream: owns the session-bus
//! connection for the app lifetime and forwards arrivals/closes as
//! [`Plant::Notify`] messages. If the name is taken (mako/dunst
//! running) or no bus exists, it logs once and the future ends — the
//! shell runs fine without D-Bus.
//!
//! v1 claims `body` only. `actions` are accepted and ignored (claiming
//! them would break clients that wait for `ActionInvoked`); buttons
//! and image hints are later stages.

use crate::app::{NotifyEvent, Plant};
use iced::futures::channel::mpsc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use zbus::{interface, object_server::SignalEmitter};

/// Hard ceiling for client-requested timeouts (5 minutes); larger
/// values clamp. `<= 0` means server default.
const MAX_TIMEOUT_MS: u64 = 300_000;

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
        _actions: Vec<String>,
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
            crate::app::layers::notification::Notification::from_dbus(
                crate::app::layers::notification::Incoming {
                    id,
                    app: app_name,
                    icon: app_icon,
                    title: summary,
                    body,
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
        let _ = self
            .tx
            .clone()
            .try_send(Plant::Notify(NotifyEvent::Dismissed(id)));
        emitter.notification_closed(id, 3).await?;
        Ok(())
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_string()]
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
            Ok(conn) => conn,
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
