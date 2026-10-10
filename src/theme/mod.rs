//! Theme storage, palette resolution, styling, and Lua snapshots.
pub mod cache;
pub mod lua;
mod schema;
pub mod store;
pub mod style;
#[cfg(test)]
pub(crate) use store::BUILTINS;
pub(crate) use store::user_file;
// Public surface: canonical paths stay `crate::theme::*`; implementations live in submodules.
pub use cache::{active, poll, sync};
#[cfg(test)]
use iced::widget::button;
#[cfg(test)]
use lua::hex;
pub use lua::lua_palette;
#[cfg(test)]
use schema::ThemeFile;
use schema::VariantRaw;
#[cfg(test)]
use store::resolve;
pub use store::{available_themes, builtin_text, ensure_user_themes, theme_dir, to_palette};
pub use style::{
    BORDER_WIDTH, RADIUS, bar, border_color, button_text, card, grid, map_label, menu_box,
    menu_button, menu_button_tinted, nav_button, output_border, output_wash, overlap_wash, preview,
    preview_button, selection_box, swatch, text, text_dim,
};
#[cfg(test)]
use style::{Class, color, container_style};

use iced::{Color, Theme};

use crate::config::ThemeConfig;

// ---------------------------------------------------------------------------
// Reshell schema
// ---------------------------------------------------------------------------

/// Parse `#rrggbb` / `#rrggbbaa` into an iced [`Color`]. Anything else is
/// `None` (caller substitutes the fallback field color and logs once).
pub(crate) fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.is_ascii() {
        return None;
    }
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
    pub fn fallback() -> Self {
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
    fn menu_buttons_have_distinct_hover_and_pressed_feedback() {
        let _guard = SERIAL.lock().unwrap();
        let theme = theme_for(&ThemeConfig {
            name: "dracula".into(),
            ..Default::default()
        });
        let style = menu_button(RADIUS);
        let active = style(&theme, button::Status::Active);
        let hovered = style(&theme, button::Status::Hovered);
        let pressed = style(&theme, button::Status::Pressed);
        assert_ne!(active.background, hovered.background);
        assert_ne!(hovered.background, pressed.background);
        assert_ne!(hovered.border.color, pressed.border.color);
        let tint = Color::from_rgb(1.0, 0.0, 0.5);
        let tinted = menu_button_tinted(RADIUS, tint);
        assert_eq!(tinted(&theme, button::Status::Hovered).text_color, tint);
        assert_eq!(tinted(&theme, button::Status::Pressed).text_color, tint);
        assert_ne!(
            tinted(&theme, button::Status::Hovered).background,
            tinted(&theme, button::Status::Pressed).background
        );
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
    fn hex_writes_rrggbb_and_round_trips() {
        assert_eq!(hex(Color::from_rgb(1.0, 0.0, 0.5)), "#ff0080");
        assert_eq!(hex(Color::BLACK), "#000000");
        assert_eq!(hex(Color::WHITE), "#ffffff");
        // Round-trips through the theme-file parser (alpha dropped).
        let back = parse_hex(&hex(Color::from_rgb(0.1, 0.2, 0.3))).unwrap();
        assert!((back.r - 0.1).abs() < 0.01);
        assert!((back.g - 0.2).abs() < 0.01);
        assert!((back.b - 0.3).abs() < 0.01);
    }

    #[test]
    fn lua_palette_flattens_live_iced_pairs() {
        let _guard = SERIAL.lock().unwrap();
        // Unknown name falls back to vendored gruvbox (deterministic).
        let pairs = lua_palette(&ThemeConfig {
            name: "no-such-theme".to_string(),
            ..ThemeConfig::default()
        });
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| *k).collect();
        for key in [
            "background",
            "on_background",
            "surface",
            "on_surface",
            "primary",
            "on_primary",
            "secondary",
            "on_secondary",
            "success",
            "on_success",
            "warning",
            "on_warning",
            "error",
            "on_error",
        ] {
            assert!(keys.contains(&key), "{key}");
        }
        for (_, value) in &pairs {
            assert!(
                value.len() == 7 && value.starts_with('#'),
                "not #rrggbb: {value}"
            );
        }
    }

    #[test]
    fn theme_service_lands_hex_in_lua() {
        let _guard = SERIAL.lock().unwrap();
        let lua = mlua::Lua::new();
        let sys = sysinfo::System::new();
        let outputs = std::collections::HashMap::new();
        let queue = std::collections::VecDeque::new();
        let workspaces = crate::services::WorkspaceCache::default();
        let toplevels = crate::services::ToplevelCache::default();
        let theme = ThemeConfig {
            name: "no-such-theme".to_string(),
            ..ThemeConfig::default()
        };
        let ctx = crate::services::ServiceCtx {
            sys: &sys,
            gpu: None,
            theme: &theme,
            outputs: &outputs,
            notifications: &queue,
            toplevels: &toplevels,
            workspaces: &workspaces,
        };
        crate::services::publish_all(&ctx, &lua).expect("publish");
        let primary: String = lua.load("return theme.primary").eval().expect("eval");
        assert!(primary.len() == 7 && primary.starts_with('#'), "{primary}");
    }

    #[test]
    fn preview_resolves_the_listed_theme_not_the_active_one() {
        let _guard = SERIAL.lock().unwrap();
        sync(&ThemeConfig::default());
        let dracula = preview("dracula", true);
        assert_eq!(dracula.primary, parse_hex("#bd93f9").unwrap());
        assert_eq!(dracula.surface, parse_hex("#282a36").unwrap());
        // light variant follows the mode flag
        let light = preview("dracula", false);
        assert_eq!(light.surface, parse_hex("#f8f8f2").unwrap());
    }

    #[test]
    fn preview_button_paints_surface_with_primary_ring_when_selected() {
        let _guard = SERIAL.lock().unwrap();
        let theme = theme_for(&ThemeConfig::default());
        let dracula = preview("dracula", true);
        let selected = preview_button(dracula, true)(&theme, button::Status::Active);
        assert_eq!(selected.background, Some(dracula.surface.into()));
        assert_eq!(selected.text_color, dracula.on_surface);
        assert_eq!(selected.border.color, dracula.primary);
        let plain = preview_button(dracula, false)(&theme, button::Status::Active);
        assert_eq!(plain.border.color, dracula.outline);
        let hovered = preview_button(dracula, false)(&theme, button::Status::Hovered);
        assert_eq!(hovered.background, Some(dracula.surface_variant.into()));
    }

    #[test]
    fn classes_resolve_and_cascade_later_wins() {
        let _guard = SERIAL.lock().unwrap();
        sync(&ThemeConfig::default());
        let theme = theme_for(&ThemeConfig::default());
        // single tokens
        assert_eq!(color(Class::TextPrimary), active().primary);
        assert_eq!(color(Class::BorderOutline), active().outline);
        // later tokens override earlier ones
        let style = container_style(&[Class::BgPrimary, Class::BgSurface])(&theme);
        assert_eq!(style.background, Some(active().surface.into()));
        // Rounded only touches the radius, BorderOutline only color+width
        let style = container_style(&[Class::BorderOutline, Class::Rounded])(&theme);
        assert_eq!(style.border.color, active().outline);
        assert_eq!(style.border.width, BORDER_WIDTH);
        assert!(style.background.is_none());
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
