//! Process-global resolved theme, mtime-checked.
use super::ActiveTheme;
use super::store::{resolve, user_file};
use crate::config::ThemeConfig;
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Process-global current theme (mtime-checked cache)
// ---------------------------------------------------------------------------

struct Cached {
    name: String,
    darkmode: bool,
    mtime: Option<SystemTime>,
    active: ActiveTheme,
}

static ACTIVE: LazyLock<RwLock<Cached>> = LazyLock::new(|| {
    let cfg = ThemeConfig::default();
    let active = resolve(&cfg);
    let mtime = file_mtime(&cfg.name);
    RwLock::new(Cached {
        name: cfg.name,
        darkmode: cfg.darkmode,
        mtime,
        active,
    })
});

fn file_mtime(name: &str) -> Option<SystemTime> {
    let path = user_file(name);
    if !path.exists() {
        return None;
    }
    std::fs::metadata(&path).and_then(|m| m.modified()).ok()
}

/// Refresh the global [`ActiveTheme`] when the selection or the underlying
/// file changed (mtime-checked, so steady-state calls are one `stat`), and
/// return it. Called by [`theme_for`] and every style helper read path.
pub fn sync(cfg: &ThemeConfig) -> ActiveTheme {
    let mtime = file_mtime(&cfg.name);
    {
        let cached = ACTIVE.read().unwrap();
        if cached.name == cfg.name && cached.darkmode == cfg.darkmode && cached.mtime == mtime {
            return cached.active;
        }
    }
    let active = resolve(cfg);
    let mut cached = ACTIVE.write().unwrap();
    *cached = Cached {
        name: cfg.name.clone(),
        darkmode: cfg.darkmode,
        mtime,
        active,
    };
    active
}

/// Current global theme without a config at hand (canvas code, tests).
/// Prefer [`sync`] on view/update paths so edits hot-reload.
pub fn active() -> ActiveTheme {
    ACTIVE.read().unwrap().active
}

/// Hot-reload check for the `ConfigTick`: `Some(new_stamp)` when the active
/// theme file changed since `known` (or appeared/disappeared). Pure check —
/// the actual reload happens via [`sync`] on the next view.
pub fn poll(cfg: &ThemeConfig, known: &Option<SystemTime>) -> Option<Option<SystemTime>> {
    let mtime = file_mtime(&cfg.name);
    (mtime != *known).then_some(mtime)
}
