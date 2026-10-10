//! Lua-facing palette snapshot; the only Lua bridge in theme/.
use super::theme_for;
use crate::config::ThemeConfig;
use iced::Color;

/// `Color` back to `#rrggbb` for the Lua `theme` table (alpha is
/// dropped — Lua transparency rides the `{r, g, b, a}` table form).
pub fn hex(c: Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8
    )
}
pub fn lua_palette(cfg: &ThemeConfig) -> Vec<(&'static str, String)> {
    use iced::theme::palette::Pair;
    let theme = theme_for(cfg);
    let p = theme.extended_palette();
    let pair = |name: &'static str, on: &'static str, pair: Pair| {
        [(name, hex(pair.color)), (on, hex(pair.text))]
    };
    [
        pair("background", "on_background", p.background.base),
        pair("surface", "on_surface", p.background.weak),
        pair("primary", "on_primary", p.primary.base),
        pair("secondary", "on_secondary", p.secondary.base),
        pair("success", "on_success", p.success.base),
        pair("warning", "on_warning", p.warning.base),
        pair("error", "on_error", p.danger.base),
    ]
    .concat()
}
