//! StatusNotifier host: asynchronous D-Bus delivery, owned Lua snapshots.
mod menu;
mod watcher;

use iced::{futures::channel::mpsc, widget::image::Handle};
pub(crate) use menu::MenuNode;
use std::sync::{Arc, Mutex};
pub(crate) use watcher::serve;

#[derive(Debug, Clone, Default)]
pub struct TrayCache(Arc<Mutex<Cache>>);

#[derive(Debug, Default)]
struct Cache {
    items: Vec<TrayItem>,
    commands: Option<mpsc::Sender<TrayCommand>>,
}

impl std::hash::Hash for TrayCache {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl TrayCache {
    pub(crate) fn refresh_icons(&self) {
        for item in self.snapshot() {
            if let Err(error) = self.command(TrayCommand {
                id: item.id,
                action: TrayAction::RefreshIcons,
            }) {
                tracing::debug!(%error, "tray icon refresh unavailable");
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn from_items(items: Vec<TrayItem>) -> Self {
        Self(Arc::new(Mutex::new(Cache {
            items,
            commands: None,
        })))
    }

    pub(crate) fn snapshot(&self) -> Vec<TrayItem> {
        self.0
            .lock()
            .map(|cache| cache.items.clone())
            .unwrap_or_default()
    }

    pub(crate) fn command(&self, command: TrayCommand) -> Result<(), String> {
        let mut cache = self.0.lock().map_err(|e| e.to_string())?;
        cache
            .commands
            .as_mut()
            .ok_or("system tray is unavailable")?
            .try_send(command)
            .map_err(|e| e.to_string())
    }

    fn replace(&self, items: Vec<TrayItem>) {
        if let Ok(mut cache) = self.0.lock() {
            cache.items = items;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TrayItem {
    /// Stable routing key: bus name plus object path (not the app's nonunique Id).
    pub id: String,
    pub app_id: String,
    pub title: String,
    pub status: String,
    pub icon_name: String,
    pub icon_path: Option<String>,
    pub icon_pixmap: Option<Handle>,
    pub attention_icon: Option<Handle>,
    pub overlay_icon: Option<Handle>,
    pub tooltip_title: String,
    pub tooltip: String,
    pub is_menu: bool,
    pub menu: Option<MenuNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayAction {
    RefreshIcons,
    Activate(i32, i32),
    Secondary(i32, i32),
    Context(i32, i32),
    Scroll(i32, String),
    Open(i32),
    MenuEvent(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayCommand {
    pub id: String,
    pub action: TrayAction,
}

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

fn pixmap(pixmaps: Pixmaps) -> Option<Handle> {
    let (w, h, bytes) = pixmaps
        .into_iter()
        .filter(|(w, h, bytes)| {
            *w > 0
                && *h > 0
                && *w <= 4096
                && *h <= 4096
                && bytes.len() == *w as usize * *h as usize * 4
        })
        .max_by_key(|(w, h, _)| i64::from(*w) * i64::from(*h))?;
    // SNI pixmaps contain network-order ARGB, iced consumes RGBA bytes.
    let rgba: Vec<u8> = bytes
        .chunks_exact(4)
        .flat_map(|p| [p[1], p[2], p[3], p[0]])
        .collect();
    Some(Handle::from_rgba(w as u32, h as u32, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_pixmaps_are_skipped() {
        assert!(pixmap(vec![(1, 1, vec![1, 2, 3]), (-1, 1, vec![])]).is_none());
    }
}
