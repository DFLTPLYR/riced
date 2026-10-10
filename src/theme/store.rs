//! Theme file sources: vendored copies plus user overrides.
use super::ActiveTheme;
use super::schema::ThemeFile;
use crate::config::ThemeConfig;
use iced::theme::Palette;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Sources: user dir (~/.config/riced/theme/) + vendored copies
// ---------------------------------------------------------------------------

/// Vendored reshell themes (`themes/*.json` in this repo). User files win;
/// these are the fallback and the seed content for first run.
/// (`dynamic` is deliberately absent: it is generated from the wallpaper
/// views via `riced generate-theme`, never a static snapshot.)
pub(crate) const BUILTINS: &[(&str, &str)] = &[
    ("ayu", include_str!("../../themes/ayu.json")),
    ("ayu-blue", include_str!("../../themes/ayu-blue.json")),
    ("catppuccin", include_str!("../../themes/catppuccin.json")),
    ("dracula", include_str!("../../themes/dracula.json")),
    ("eldritch", include_str!("../../themes/eldritch.json")),
    ("ferra", include_str!("../../themes/ferra.json")),
    ("gruvbox", include_str!("../../themes/gruvbox.json")),
    ("kanagawa", include_str!("../../themes/kanagawa.json")),
    ("nord", include_str!("../../themes/nord.json")),
    ("rosepine", include_str!("../../themes/rosepine.json")),
    ("tokyo-night", include_str!("../../themes/tokyo-night.json")),
];

const GENERATED: &[&str] = &["dynamic"];

pub fn builtin_text(name: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

pub fn theme_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("theme"))
        .unwrap_or_else(|| PathBuf::from("theme"))
}

pub(crate) fn user_file(name: &str) -> PathBuf {
    theme_dir().join(format!("{name}.json"))
}

pub fn available_themes() -> Vec<String> {
    let mut names: Vec<String> = BUILTINS.iter().map(|(n, _)| n.to_string()).collect();
    if let Ok(entries) = std::fs::read_dir(theme_dir()) {
        for entry in entries.flatten() {
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str())
                && entry.path().extension().and_then(|e| e.to_str()) == Some("json")
                && !names.contains(&stem.to_string())
            {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    names
}

pub fn ensure_user_themes() {
    let dir = theme_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("theme: cannot create {}: {e}", dir.display());
        return;
    }
    for (name, content) in BUILTINS {
        if GENERATED.contains(name) {
            continue;
        }
        let path = dir.join(format!("{name}.json"));
        if !path.exists()
            && let Err(e) = std::fs::write(&path, content)
        {
            eprintln!("theme: cannot seed {}: {e}", path.display());
        }
    }
}

fn load_file(name: &str) -> Option<ThemeFile> {
    // User file wins so edits apply without touching the repo.
    let path = user_file(name);
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(file) => return Some(file),
                Err(e) => eprintln!("theme: parse error in {}: {e}", path.display()),
            },
            Err(e) => eprintln!("theme: cannot read {}: {e}", path.display()),
        }
    }
    BUILTINS
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, content)| serde_json::from_str(content).ok())
}

pub fn resolve(cfg: &ThemeConfig) -> ActiveTheme {
    let file = load_file(&cfg.name).or_else(|| {
        if cfg.name != "gruvbox" {
            if cfg.name == "dynamic" {
                eprintln!(
                    "theme: no dynamic.json yet — run `riced generate-theme --set` \
                     to build it from the wallpaper views"
                );
            } else {
                eprintln!(
                    "theme: unknown theme {:?}, falling back to gruvbox",
                    cfg.name
                );
            }
        }
        load_file("gruvbox")
    });
    match file {
        Some(f) => ActiveTheme::from_raw(if cfg.darkmode { &f.dark } else { &f.light }),
        None => ActiveTheme::fallback(),
    }
}

pub fn to_palette(a: &ActiveTheme) -> Palette {
    Palette {
        background: a.surface,
        text: a.on_surface,
        primary: a.primary,
        success: a.tertiary,
        warning: a.secondary,
        danger: a.error,
    }
}
