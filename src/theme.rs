use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

use serde::Deserialize;

use iced::widget::{button, container};
use iced::{Border, Color, Theme, theme::Palette};

use crate::config::ThemeConfig;

// ---------------------------------------------------------------------------
// Reshell schema
// ---------------------------------------------------------------------------

/// One `dark`/`light` variant of a reshell theme file. Colors are `#rrggbb`
/// hex. The reshell `terminal` block is ignored by serde (unknown fields
/// pass through) — riced has no terminal. `alias`es accept the unseparated
/// spelling some reshell files use (`onprimary` in `ayu-blue.json`).
#[derive(Debug, Clone, Deserialize)]
struct VariantRaw {
    primary: String,
    #[serde(alias = "onprimary")]
    on_primary: String,
    secondary: String,
    #[serde(alias = "onsecondary")]
    on_secondary: String,
    tertiary: String,
    #[serde(alias = "ontertiary")]
    on_tertiary: String,
    error: String,
    #[serde(alias = "onerror")]
    on_error: String,
    surface: String,
    #[serde(alias = "onsurface")]
    on_surface: String,
    #[serde(alias = "surfacevariant")]
    surface_variant: String,
    #[serde(alias = "onsurfacevariant")]
    on_surface_variant: String,
    outline: String,
    shadow: String,
    hover: String,
    #[serde(alias = "onhover")]
    on_hover: String,
}

/// A reshell theme file: `{ "dark": {...}, "light": {...} }`.
#[derive(Debug, Clone, Deserialize)]
struct ThemeFile {
    dark: VariantRaw,
    light: VariantRaw,
}

/// Parse `#rrggbb` / `#rrggbbaa` into an iced [`Color`]. Anything else is
/// `None` (caller substitutes the fallback field color and logs once).
fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.trim().strip_prefix('#')?;
    let (r, g, b, a) = match hex.len() {
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            255,
        ),
        8 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some(Color::from_rgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ))
}

// ---------------------------------------------------------------------------
// ActiveTheme — resolved, ready-to-paint colors
// ---------------------------------------------------------------------------

/// The selected variant of the selected theme file, resolved to iced
/// colors. This is what every style helper paints from.
#[derive(Debug, Clone, Copy)]
pub struct ActiveTheme {
    pub primary: Color,
    pub on_primary: Color,
    pub secondary: Color,
    pub on_secondary: Color,
    pub tertiary: Color,
    pub on_tertiary: Color,
    pub error: Color,
    pub on_error: Color,
    pub surface: Color,
    pub on_surface: Color,
    pub surface_variant: Color,
    pub on_surface_variant: Color,
    pub outline: Color,
    pub shadow: Color,
    pub hover: Color,
    pub on_hover: Color,
}

impl ActiveTheme {
    /// Vendored `gruvbox` dark (reshell's default theme + mode): the
    /// last-resort fallback when no theme file parses.
    pub fn fallback() -> Self {
        // #b8bb26 / #282828 / #fabd2f / #83a598 / #fb4934 / #282828 /
        // #fbf1c7 / #3c3836 / #ebdbb2 / #57514e / #282828 / #83a598
        Self {
            primary: Color::from_rgb(
                0xB8 as f32 / 255.0,
                0xBB as f32 / 255.0,
                0x26 as f32 / 255.0,
            ),
            on_primary: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
            secondary: Color::from_rgb(
                0xFA as f32 / 255.0,
                0xBD as f32 / 255.0,
                0x2F as f32 / 255.0,
            ),
            on_secondary: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
            tertiary: Color::from_rgb(
                0x83 as f32 / 255.0,
                0xA5 as f32 / 255.0,
                0x98 as f32 / 255.0,
            ),
            on_tertiary: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
            error: Color::from_rgb(
                0xFB as f32 / 255.0,
                0x49 as f32 / 255.0,
                0x34 as f32 / 255.0,
            ),
            on_error: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
            surface: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
            on_surface: Color::from_rgb(
                0xFB as f32 / 255.0,
                0xF1 as f32 / 255.0,
                0xC7 as f32 / 255.0,
            ),
            surface_variant: Color::from_rgb(
                0x3C as f32 / 255.0,
                0x38 as f32 / 255.0,
                0x36 as f32 / 255.0,
            ),
            on_surface_variant: Color::from_rgb(
                0xEB as f32 / 255.0,
                0xDB as f32 / 255.0,
                0xB2 as f32 / 255.0,
            ),
            outline: Color::from_rgb(
                0x57 as f32 / 255.0,
                0x51 as f32 / 255.0,
                0x4E as f32 / 255.0,
            ),
            shadow: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
            hover: Color::from_rgb(
                0x83 as f32 / 255.0,
                0xA5 as f32 / 255.0,
                0x98 as f32 / 255.0,
            ),
            on_hover: Color::from_rgb(
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
                0x28 as f32 / 255.0,
            ),
        }
    }

    fn from_raw(raw: &VariantRaw) -> Self {
        let fb = Self::fallback();
        let pick = |s: &str, fallback: Color| {
            parse_hex(s).unwrap_or_else(|| {
                eprintln!("theme: bad hex {s:?}, keeping fallback");
                fallback
            })
        };
        Self {
            primary: pick(&raw.primary, fb.primary),
            on_primary: pick(&raw.on_primary, fb.on_primary),
            secondary: pick(&raw.secondary, fb.secondary),
            on_secondary: pick(&raw.on_secondary, fb.on_secondary),
            tertiary: pick(&raw.tertiary, fb.tertiary),
            on_tertiary: pick(&raw.on_tertiary, fb.on_tertiary),
            error: pick(&raw.error, fb.error),
            on_error: pick(&raw.on_error, fb.on_error),
            surface: pick(&raw.surface, fb.surface),
            on_surface: pick(&raw.on_surface, fb.on_surface),
            surface_variant: pick(&raw.surface_variant, fb.surface_variant),
            on_surface_variant: pick(&raw.on_surface_variant, fb.on_surface_variant),
            outline: pick(&raw.outline, fb.outline),
            shadow: pick(&raw.shadow, fb.shadow),
            hover: pick(&raw.hover, fb.hover),
            on_hover: pick(&raw.on_hover, fb.on_hover),
        }
    }
}

// ---------------------------------------------------------------------------
// Sources: user dir (~/.config/riced/theme/) + vendored copies
// ---------------------------------------------------------------------------

/// Vendored reshell themes (`themes/*.json` in this repo). User files win;
/// these are the fallback and the seed content for first run.
/// (`dynamic` is deliberately absent: it is generated from the wallpaper
/// views via `riced generate-theme`, never a static snapshot.)
const BUILTINS: &[(&str, &str)] = &[
    ("ayu", include_str!("../themes/ayu.json")),
    ("ayu-blue", include_str!("../themes/ayu-blue.json")),
    ("catppuccin", include_str!("../themes/catppuccin.json")),
    ("dracula", include_str!("../themes/dracula.json")),
    ("eldritch", include_str!("../themes/eldritch.json")),
    ("ferra", include_str!("../themes/ferra.json")),
    ("gruvbox", include_str!("../themes/gruvbox.json")),
    ("kanagawa", include_str!("../themes/kanagawa.json")),
    ("nord", include_str!("../themes/nord.json")),
    ("rosepine", include_str!("../themes/rosepine.json")),
    ("tokyo-night", include_str!("../themes/tokyo-night.json")),
];

/// Names that are generated, not vendored: skipped by seeding (below).
const GENERATED: &[&str] = &["dynamic"];

/// Raw text of a vendored builtin (`None` for generated/unknown names).
/// Used by the `change_theme` object pass without touching disk.
pub fn builtin_text(name: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// `~/.config/riced/theme` (`$XDG_CONFIG_HOME` aware).
pub fn theme_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("theme"))
        .unwrap_or_else(|| PathBuf::from("theme"))
}

pub(crate) fn user_file(name: &str) -> PathBuf {
    theme_dir().join(format!("{name}.json"))
}

/// All known theme names: vendored builtins plus any extra `*.json` in the
/// user dir (including a generated `dynamic.json`), deduplicated and sorted.
/// Drives the Settings Theme page.
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

/// Seed the user theme dir on first run: copy every vendored theme that the
/// user doesn't already have (never overwrites edits). Generated names
/// (`dynamic`) are skipped — they only ever come from `riced generate-theme`.
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

/// Resolve `name` + `darkmode` to paintable colors. Unknown names fall back
/// to vendored `gruvbox` (same variant); a fully broken setup falls back to
/// hardcoded gruvbox-dark so the shell never goes unstyled.
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

/// `iced::Palette` projection of an [`ActiveTheme`]: background/text carry
/// the surface pair, primary/tertiary/secondary/error fill the accent slots
/// so built-in widget styles (sliders, radios, …) follow the theme file too.
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

// ---------------------------------------------------------------------------
// Daemon hooks
// ---------------------------------------------------------------------------

/// `daemon(...).theme(...)` hook: one `iced::Theme` for every surface, built
/// from the active reshell variant. Syncs the global [`ActiveTheme`] first so
/// style helpers and canvas code paint the same colors this frame.
pub fn theme_for(cfg: &ThemeConfig) -> Theme {
    let active = sync(cfg);
    Theme::custom(cfg.name.clone(), to_palette(&active))
}

/// `daemon(...).style(...)` hook: transparent clear color (lets Hyprland
/// blur/opacity windowrules see through layer surfaces) with theme text.
pub fn app_style<State>(_: &State, _: &Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: active().on_surface,
    }
}

// ---------------------------------------------------------------------------
// Roles — named colors, following reshell component conventions
// ---------------------------------------------------------------------------

/// Default text on surfaces.
pub fn text() -> Color {
    active().on_surface
}

/// Dim label text (debug label over wallpapers).
pub fn text_dim() -> Color {
    active().on_surface.scale_alpha(0.7)
}

/// Map overlay label text.
pub fn map_label() -> Color {
    active().on_surface.scale_alpha(0.8)
}

/// Card background (context menus, settings map boxes): reshell `Menu`
/// paints `surface` with an `outline` border.
pub fn card() -> Color {
    active().surface
}

/// Button / menu-item label text: reshell `Menu` items paint `primary`.
pub fn button_text() -> Color {
    active().primary
}

/// Hairline border for every bordered surface/button.
pub fn border_color() -> Color {
    active().outline
}

/// Top bar backdrop (opaque on purpose: the daemon clears transparent, so the
/// bar must paint its own backdrop or wallpaper shows through).
pub fn bar_bg() -> Color {
    active().surface
}

/// Default corner radius for cards and buttons.
pub const RADIUS: f32 = 6.0;
/// Hairline width shared by all bordered styles.
pub const BORDER_WIDTH: f32 = 1.0;

// Canvas (display-map) colors.
pub fn grid() -> Color {
    active().on_surface.scale_alpha(0.35)
}
pub fn output_wash() -> Color {
    active().primary.scale_alpha(0.06)
}
pub fn overlap_wash() -> Color {
    active().on_surface.scale_alpha(0.15)
}
pub fn output_border() -> Color {
    active().primary
}

// ---------------------------------------------------------------------------
// Containers — `container(x).style(theme::menu_box)` etc.
// ---------------------------------------------------------------------------

fn bordered(background: Color, radius: f32) -> container::Style {
    container::Style {
        background: Some(background.into()),
        text_color: Some(active().on_surface),
        border: Border {
            color: border_color(),
            width: BORDER_WIDTH,
            radius: radius.into(),
        },
        ..Default::default()
    }
}

/// Card surface: context menus, settings map boxes.
pub fn menu_box(_: &Theme) -> container::Style {
    bordered(card(), RADIUS)
}

/// Opaque top-bar backdrop.
pub fn bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(bar_bg().into()),
        text_color: Some(active().on_surface),
        ..Default::default()
    }
}

/// Fully transparent container (background label, settings root).
pub fn transparent_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::TRANSPARENT.into()),
        ..Default::default()
    }
}

/// Drag-selection rectangle. `opacity` is the 150ms fade value (`1.0` while
/// selecting); `radius` is the per-corner radius so corners clipped by the
/// output edge render square instead of sliced.
pub fn selection_box(
    opacity: f32,
    radius: iced::border::Radius,
) -> impl Fn(&Theme) -> container::Style {
    let a = active();
    let background = a.primary.scale_alpha(0.5 * opacity);
    let border_color = a.primary.scale_alpha(opacity);
    move |_| container::Style {
        background: Some(background.into()),
        border: Border {
            color: border_color,
            width: BORDER_WIDTH,
            radius,
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Buttons — `.style(theme::menu_button(rounding))`
// ---------------------------------------------------------------------------

fn button_base(background: Color, text_color: Color, radius: f32) -> button::Style {
    button::Style {
        background: Some(background.into()),
        text_color,
        border: Border {
            color: border_color(),
            width: BORDER_WIDTH,
            radius: radius.into(),
        },
        ..Default::default()
    }
}

/// Context-menu / generic raised button, mirroring reshell `Menu` items
/// (`primary` label, `surface` base, `surface_variant` + `on_surface` when
/// highlighted). `rounding` comes from config
/// (`context_menu_item.rounding`), so this is a factory returning the
/// `Fn(&Theme, Status) -> Style` that `Button::style` expects.
pub fn menu_button(rounding: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let a = active();
        match status {
            button::Status::Hovered | button::Status::Pressed => {
                button_base(a.surface_variant, a.on_surface, rounding)
            }
            button::Status::Disabled => {
                button_base(a.surface, a.on_surface.scale_alpha(0.5), rounding)
            }
            button::Status::Active => button_base(a.surface, a.primary, rounding),
        }
    }
}

/// Settings nav button; selected pages paint `primary`/`on_primary`.
pub fn nav_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, _| {
        let a = active();
        if selected {
            button_base(a.primary, a.on_primary, RADIUS)
        } else {
            button_base(a.surface_variant, a.on_surface, RADIUS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    /// Serializes tests that touch the process-global [`ActiveTheme`]:
    /// `sync` writes it, helpers read it, and cargo runs tests in parallel.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex("#ffffff"), Some(Color::WHITE));
        assert_eq!(parse_hex("#000000"), Some(Color::BLACK));
        let c = parse_hex("#b8bb26").unwrap();
        assert!((c.r - 0xB8 as f32 / 255.0).abs() < 1e-6);
        assert!(parse_hex("not-a-color").is_none());
        assert!(parse_hex("#12345").is_none());
    }

    #[test]
    fn vendored_gruvbox_parses_to_active_theme() {
        let (_, content) = BUILTINS.iter().find(|(n, _)| *n == "gruvbox").unwrap();
        let file: ThemeFile = serde_json::from_str(content).unwrap();
        let dark = ActiveTheme::from_raw(&file.dark);
        assert_eq!(dark.surface, parse_hex("#282828").unwrap());
        assert_eq!(dark.on_surface, parse_hex("#fbf1c7").unwrap());
        let light = ActiveTheme::from_raw(&file.light);
        assert_eq!(light.surface, parse_hex("#fbf1c7").unwrap());
    }

    #[test]
    fn all_vendored_themes_parse() {
        for (name, content) in BUILTINS {
            let file: ThemeFile =
                serde_json::from_str(content).unwrap_or_else(|e| panic!("{name} parses: {e}"));
            // Both variants resolve without falling back on any field: spot
            // check primary + surface against the raw hex.
            for (variant, raw) in [(&file.dark, "dark"), (&file.light, "light")] {
                let active = ActiveTheme::from_raw(variant);
                assert_eq!(
                    active.primary,
                    parse_hex(&variant.primary).unwrap(),
                    "{name}/{raw} primary"
                );
                assert_eq!(
                    active.surface,
                    parse_hex(&variant.surface).unwrap(),
                    "{name}/{raw} surface"
                );
            }
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_gruvbox() {
        let _guard = SERIAL.lock().unwrap();
        let cfg = ThemeConfig {
            name: "no-such-theme".to_string(),
            ..ThemeConfig::default()
        };
        // No user file in test env: resolves via vendored gruvbox.
        let active = resolve(&cfg);
        assert_eq!(active.surface, parse_hex("#282828").unwrap());
    }

    #[test]
    fn user_file_overrides_vendored_builtin() {
        let _guard = SERIAL.lock().unwrap();
        // Point XDG_CONFIG_HOME at a scratch dir (config_dir() honors it;
        // HOME is untouched so other tests are unaffected).
        let dir = std::env::temp_dir().join(format!("riced-theme-e2e-{}", std::process::id()));
        let theme_sub = dir.join("riced").join("theme");
        std::fs::create_dir_all(&theme_sub).unwrap();
        let custom = r##"{"dark": {
            "primary": "#010203", "on_primary": "#282828",
            "secondary": "#fabd2f", "on_secondary": "#282828",
            "tertiary": "#83a598", "on_tertiary": "#282828",
            "error": "#fb4934", "on_error": "#282828",
            "surface": "#010203", "on_surface": "#fbf1c7",
            "surface_variant": "#3c3836", "on_surface_variant": "#ebdbb2",
            "outline": "#57514e", "shadow": "#282828",
            "hover": "#83a598", "on_hover": "#282828"},
            "light": {
            "primary": "#010203", "on_primary": "#fbf1c7",
            "secondary": "#d79921", "on_secondary": "#fbf1c7",
            "tertiary": "#458588", "on_tertiary": "#fbf1c7",
            "error": "#cc241d", "on_error": "#fbf1c7",
            "surface": "#fbf1c7", "on_surface": "#3c3836",
            "surface_variant": "#ebdbb2", "on_surface_variant": "#7c6f64",
            "outline": "#57514e", "shadow": "#282828",
            "hover": "#458588", "on_hover": "#fbf1c7"}}"##;
        std::fs::write(theme_sub.join("gruvbox.json"), custom).unwrap();
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        // SAFETY: test-only; SERIAL excludes other ACTIVE users and no other
        // test reads XDG_CONFIG_HOME.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let surface = resolve(&ThemeConfig::default()).surface;
        match prev {
            // SAFETY: same as above.
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
        // User copy won over the vendored gruvbox (#282828).
        assert_eq!(surface, parse_hex("#010203").unwrap());
    }

    #[test]
    fn theme_for_builds_custom_iced_theme() {
        let _guard = SERIAL.lock().unwrap();
        let cfg = ThemeConfig {
            name: "dracula".to_string(),
            ..ThemeConfig::default()
        };
        let theme = theme_for(&cfg);
        assert_eq!(theme.to_string(), "dracula");
        assert_eq!(theme.palette().background, parse_hex("#282a36").unwrap());
        assert_eq!(theme.palette().text, parse_hex("#f8f8f2").unwrap());
    }

    #[test]
    fn app_style_stays_transparent_for_layer_shell() {
        let _guard = SERIAL.lock().unwrap();
        sync(&ThemeConfig::default());
        let style = app_style::<()>(&(), &theme_for(&ThemeConfig::default()));
        assert_eq!(style.background_color, Color::TRANSPARENT);
        assert_eq!(style.text_color, active().on_surface);
    }

    #[test]
    fn menu_box_uses_card_and_outline() {
        let _guard = SERIAL.lock().unwrap();
        sync(&ThemeConfig::default());
        let style = menu_box(&theme_for(&ThemeConfig::default()));
        assert_eq!(style.background, Some(card().into()));
        assert_eq!(style.border.color, border_color());
    }

    #[test]
    fn selection_box_fades_with_opacity() {
        let _guard = SERIAL.lock().unwrap();
        sync(&ThemeConfig::default());
        let theme = theme_for(&ThemeConfig::default());
        let full = selection_box(1.0, RADIUS.into())(&theme);
        let half = selection_box(0.5, RADIUS.into())(&theme);
        match (full.background, half.background) {
            (Some(iced::Background::Color(a)), Some(iced::Background::Color(b))) => {
                assert!(a.a > b.a)
            }
            _ => panic!("selection background must be a color"),
        }
    }
}
