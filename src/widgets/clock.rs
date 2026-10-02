//! Clock widget: current local time as `HH:MM`.
//!
//! Plain `libc` (`localtime_r`) instead of a datetime crate: one
//! locked dep, no timezone database to ship — the process TZ is
//! enough for a bar clock. Ticks with the bar redraw (no timer of
//! its own).

use crate::app::Plant;
use iced::Element;
use iced::widget::text;

/// Current local time as `HH:MM` (`--:--` when the clock is unreadable).
pub fn label() -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut broken: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `stamp` and `broken` are valid readable/writable locals;
    // `localtime_r` writes a full `tm` or returns null on overflow.
    let ok = unsafe { libc::localtime_r(&stamp, &mut broken) };
    if ok.is_null() {
        return String::from("--:--");
    }
    format!("{:02}:{:02}", broken.tm_hour, broken.tm_min)
}

/// Clock body for a bar grid cell.
pub fn view<'a>() -> Element<'a, Plant> {
    text(label()).size(13).into()
}
