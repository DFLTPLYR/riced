//! D-Bus freedesktop Notifications server (mako replacement).
//!
//! Lives in an iced subscription stream: owns the session-bus
//! connection for the app lifetime and forwards arrivals/closes as
//! [`Plant::Notify`] messages. If the name is taken (mako/dunst
//! running) or no bus exists, it logs once and the future ends — the
//! shell runs fine without D-Bus.
//!
//! Claims `body` + `actions`. Image hints (`image-data`,
//! `image-path`, icon paths) decode without claiming `icon-static`:
//! senders attach them regardless, and themed names we can't resolve
//! simply render text-only. Action clicks and local dismissals emit
//! `ActionInvoked` / `NotificationClosed` back over the stored
//! connection (see [`emit_action_invoked`], [`emit_closed`).

use crate::app::{NotifyEvent, Plant};
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

/// Hard ceiling for icon pixels per side (abuse guard: a malicious
/// sender could otherwise push megapixel pixbufs through the bus).
const MAX_ICON_PX: i32 = 512;

/// Decode a freedesktop `image-data` pixbuf `(width, height, rowstride,
/// has_alpha, bits_per_sample, channels, data)` into an iced image
/// handle. Only 8-bit RGB/RGBA; rowstride padding is stripped per row.
/// Anything malformed is `None` (the card renders text-only).
fn decode_pixbuf(
    width: i32,
    height: i32,
    rowstride: i32,
    bps: i32,
    channels: i32,
    data: &[u8],
) -> Option<iced::widget::image::Handle> {
    if bps != 8 || !(1..=MAX_ICON_PX).contains(&width) || !(1..=MAX_ICON_PX).contains(&height) {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let stride = rowstride as usize;
    let ch = channels as usize;
    if (ch != 3 && ch != 4) || stride < w * ch || data.len() < stride * h {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let base = y * stride;
        for x in 0..w {
            let p = base + x * ch;
            rgba.push(data[p]);
            rgba.push(data[p + 1]);
            rgba.push(data[p + 2]);
            rgba.push(if ch == 4 { data[p + 3] } else { 255 });
        }
    }
    Some(iced::widget::image::Handle::from_rgba(
        w as u32,
        h as u32,
        bytes::Bytes::from(rgba),
    ))
}
/// `image-data` hint struct to pixels (`None` on any shape/type
/// mismatch — senders vary, never trust the bus).
fn hint_image_data(
    hints: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Option<iced::widget::image::Handle> {
    use zbus::zvariant::Value;
    let Value::Structure(image) = Value::try_from(hints.get("image-data")?).ok()? else {
        return None;
    };
    let f = image.fields();
    let num = |i: usize| match f.get(i) {
        Some(Value::I32(n)) => Some(*n),
        _ => None,
    };
    let (width, height, rowstride, bps, channels) = (num(0)?, num(1)?, num(2)?, num(4)?, num(5)?);
    let data: Vec<u8> = match f.get(6) {
        Some(Value::Array(bytes)) => bytes.iter().filter_map(|v| u8::try_from(v).ok()).collect(),
        _ => return None,
    };
    decode_pixbuf(width, height, rowstride, bps, channels, &data)
}

/// Plain image file (`image-path` hint or `app_icon`): absolute paths
/// (and `file://` URIs) only — freedesktop theme-name lookup is out of
/// scope, so named icons still render text-only.
fn image_file(path: &str) -> Option<iced::widget::image::Handle> {
    let path = path.strip_prefix("file://").unwrap_or(path);
    if !path.starts_with('/') {
        return None;
    }
    let img = image::open(path).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    Some(iced::widget::image::Handle::from_rgba(
        w,
        h,
        bytes::Bytes::from(rgba.into_raw()),
    ))
}

/// Resolve a notification image: `image-data` first, then the
/// `image-path` hint, then `app_icon` as a path. Anything unusable is
/// `None` — the card renders text-only either way.
fn image_of(
    hints: &HashMap<String, zbus::zvariant::OwnedValue>,
    app_icon: &str,
) -> Option<iced::widget::image::Handle> {
    hint_image_data(hints).or_else(|| {
        hints
            .get("image-path")
            .and_then(|v| <&str>::try_from(v).ok())
            .or(Some(app_icon))
            .filter(|p| !p.is_empty())
            .and_then(image_file)
    })
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
            crate::app::layers::notification::Notification::from_dbus(
                crate::app::layers::notification::Incoming {
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

    #[test]
    fn decode_pixbuf_handles_rgb_rgba_and_stride() {
        // 2x1 RGB, no padding: red, green.
        let rgb = vec![255, 0, 0, 0, 255, 0];
        assert!(decode_pixbuf(2, 1, 6, 8, 3, &rgb).is_some());
        // 1x1 RGBA with rowstride padding after the pixel.
        let rgba_pad = vec![10, 20, 30, 40, 0, 0];
        assert!(decode_pixbuf(1, 1, 6, 8, 4, &rgba_pad).is_some());
        // Rejects: wrong bit depth, bad channels, oversize, short data.
        assert!(decode_pixbuf(2, 1, 6, 1, 3, &rgb).is_none());
        assert!(decode_pixbuf(2, 1, 6, 8, 2, &rgb).is_none());
        assert!(decode_pixbuf(600, 1, 1800, 8, 3, &vec![0; 1800]).is_none());
        assert!(decode_pixbuf(0, 1, 6, 8, 3, &rgb).is_none());
        assert!(decode_pixbuf(2, 1, 6, 8, 3, &[1, 2, 3]).is_none());
        // Stride narrower than the row is corrupt.
        assert!(decode_pixbuf(2, 1, 5, 8, 3, &rgb).is_none());
    }

    #[test]
    fn image_of_prefers_data_over_paths() {
        use zbus::zvariant::{OwnedValue, Value};
        // image-data wins even when a path is present.
        // (u8 annotated: plain literals would infer Vec<i32>.)
        let rgb = vec![255u8, 0, 0, 0, 255, 0];
        let structure = zbus::zvariant::StructureBuilder::new()
            .add_field(2i32)
            .add_field(1i32)
            .add_field(6i32)
            .add_field(true)
            .add_field(8i32)
            .add_field(3i32)
            .add_field(rgb)
            .build()
            .unwrap();
        let mut hints = HashMap::new();
        hints.insert(
            "image-data".to_string(),
            OwnedValue::try_from(Value::Structure(structure)).unwrap(),
        );
        hints.insert(
            "image-path".to_string(),
            OwnedValue::try_from(Value::new("/nope.png")).unwrap(),
        );
        assert!(image_of(&hints, "/also-nope.png").is_some());
        // Garbage struct falls back to paths (missing here -> None).
        let mut bad = HashMap::new();
        bad.insert(
            "image-data".to_string(),
            OwnedValue::try_from(Value::new("junk")).unwrap(),
        );
        assert!(image_of(&bad, "themed-name").is_none());
        // Non-paths never touch the filesystem.
        assert!(image_of(&HashMap::new(), "themed-name").is_none());
        assert!(image_of(&HashMap::new(), "").is_none());
    }
}
