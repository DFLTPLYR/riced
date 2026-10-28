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
    Arc, LazyLock, Mutex,
    atomic::{AtomicU32, Ordering},
};
use zbus::{interface, object_server::SignalEmitter};

/// Hard ceiling for client-requested timeouts (5 minutes); larger
/// values clamp. Negative means server default; zero means never expire.
const MAX_TIMEOUT_MS: u64 = 300_000;
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Shared by D-Bus and internal events; restarting the subscription must not
/// reuse IDs belonging to retained history.
pub(crate) fn allocate_id() -> u32 {
    loop {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

/// Live bus connection for emitting `ActionInvoked` /
/// `NotificationClosed` from message handlers (which can't await).
/// Present only while the serving subscription is alive.
static DBUS_CONN: LazyLock<Mutex<Option<Arc<zbus::Connection>>>> =
    LazyLock::new(|| Mutex::new(None));

struct ServingConnection(Arc<zbus::Connection>);
impl ServingConnection {
    fn new(connection: zbus::Connection) -> Self {
        let connection = Arc::new(connection);
        if let Ok(mut slot) = DBUS_CONN.lock() {
            *slot = Some(connection.clone());
        }
        Self(connection)
    }
}
impl Drop for ServingConnection {
    fn drop(&mut self) {
        if let Ok(mut slot) = DBUS_CONN.lock()
            && slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.0))
        {
            *slot = None;
        }
    }
}

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
            allocate_id()
        };
        let timeout_ms = (expire_timeout >= 0).then(|| (expire_timeout as u64).min(MAX_TIMEOUT_MS));
        let msg = Plant::Notify(NotifyEvent::Arrived(Box::new(
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
        )));
        // Channel full/closed: drop rather than stall the bus caller.
        let _ = self.tx.clone().try_send(msg);
        Ok(id)
    }

    async fn close_notification(&self, id: u32) -> zbus::fdo::Result<()> {
        // The service emits the close signal once, only for a live record.
        let _ = self
            .tx
            .clone()
            .try_send(Plant::Notify(NotifyEvent::PeerClosed(id)));
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
pub async fn serve(tx: mpsc::Sender<Plant>, default_ms: u64) {
    match zbus::connection::Builder::session() {
        Ok(builder) => serve_on(tx, default_ms, builder).await,
        Err(error) => {
            eprintln!("riced: notifications: D-Bus unavailable ({error}), continuing without it")
        }
    }
}

async fn serve_on(
    mut tx: mpsc::Sender<Plant>,
    default_ms: u64,
    builder: zbus::connection::Builder<'_>,
) {
    let server = Server {
        tx: tx.clone(),
        default_ms,
    };
    let built = builder
        .name("org.freedesktop.Notifications")
        .and_then(|b| b.serve_at("/org/freedesktop/Notifications", server));
    let _conn = match built {
        Ok(b) => match b.build().await {
            Ok(conn) => ServingConnection::new(conn),
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
    // The guard clears shared ownership even when this future is cancelled.
    std::future::pending::<()>().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_server_releases_name_and_restart_emits_signals() {
        use iced::futures::StreamExt;
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        struct Bus(std::process::Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut bus = Bus(Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap());
        let mut address = String::new();
        BufReader::new(bus.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let peer = zbus::connection::Builder::address(address)
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                let proxy = zbus::Proxy::new(
                    &peer,
                    "org.freedesktop.Notifications",
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                )
                .await
                .unwrap();
                for timeout in [1000, 2000, 2000] {
                    let (tx, mut rx) = mpsc::channel(32);
                    let server = tokio::spawn(serve_on(
                        tx,
                        timeout,
                        zbus::connection::Builder::address(address).unwrap(),
                    ));
                    assert!(matches!(
                        tokio::time::timeout(std::time::Duration::from_secs(5), rx.next())
                            .await
                            .unwrap(),
                        Some(Plant::Notify(NotifyEvent::DBusUp))
                    ));
                    let id: u32 = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        proxy.call(
                            "Notify",
                            &(
                                "test",
                                0u32,
                                "",
                                "title",
                                "body",
                                Vec::<String>::new(),
                                HashMap::<String, zbus::zvariant::OwnedValue>::new(),
                                -1i32,
                            ),
                        ),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    let Some(Plant::Notify(NotifyEvent::Arrived(item))) =
                        tokio::time::timeout(std::time::Duration::from_secs(5), rx.next())
                            .await
                            .unwrap()
                    else {
                        panic!("missing arrival")
                    };
                    assert_eq!(item.id, id);
                    assert_eq!(
                        item.timeout,
                        Some(std::time::Duration::from_millis(timeout))
                    );
                    let mut signals = proxy.receive_signal("NotificationClosed").await.unwrap();
                    let task = emit_closed(u32::MAX, 2);
                    let mut stream = iced_runtime::task::into_stream(task).unwrap();
                    while stream.next().await.is_some() {}
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        while let Some(signal) = signals.next().await {
                            if signal.body().deserialize::<(u32, u32)>().unwrap() == (u32::MAX, 2) {
                                return;
                            }
                        }
                        panic!("close signal stream ended");
                    })
                    .await
                    .unwrap();
                    drop(signals);
                    let mut actions = proxy.receive_signal("ActionInvoked").await.unwrap();
                    let mut stream = iced_runtime::task::into_stream(emit_action_invoked(
                        u32::MAX,
                        "open:details".into(),
                    ))
                    .unwrap();
                    while stream.next().await.is_some() {}
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        while let Some(signal) = actions.next().await {
                            if signal.body().deserialize::<(u32, String)>().unwrap()
                                == (u32::MAX, "open:details".into())
                            {
                                return;
                            }
                        }
                        panic!("action signal stream ended");
                    })
                    .await
                    .unwrap();
                    drop(actions);
                    server.abort();
                    let _ = server.await;
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        loop {
                            if peer
                                .request_name_with_flags(
                                    "org.freedesktop.Notifications",
                                    zbus::fdo::RequestNameFlags::DoNotQueue.into(),
                                )
                                .await
                                .is_ok()
                            {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("disabled server retained notification bus name");
                    peer.release_name("org.freedesktop.Notifications")
                        .await
                        .unwrap();
                }
                let old = ServingConnection::new(peer.clone());
                let new = ServingConnection::new(peer.clone());
                drop(old);
                assert!(Arc::ptr_eq(
                    DBUS_CONN.lock().unwrap().as_ref().unwrap(),
                    &new.0
                ));
                drop(new);
                assert!(DBUS_CONN.lock().unwrap().is_none());
            });
    }

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
