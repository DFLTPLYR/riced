//! Design tokens and iced style factories; no file I/O, no globals.
use super::ActiveTheme;
use super::cache::active;
use super::store::resolve;
use crate::config::ThemeConfig;
use iced::widget::{button, container};
use iced::{Border, Color, Theme};

// ---------------------------------------------------------------------------
// Classes — Tailwind-style composable tokens (enum, not strings, so a
// typo fails at compile time). One token language, three interpreters:
// `color` for single colors, `container_style` for containers, and the
// button factories below for status-dependent styles (hover/pressed
// can't be static tokens without variant machinery — that's where this
// stops and factories take over). Later tokens win, Tailwind-cascade
// style. Import as `use crate::theme::{self, Class as C};` for
// `theme::container_style(&[C::BgSurface, C::BorderOutline, C::Rounded])`.
// ---------------------------------------------------------------------------

/// Atomic style token. `Bg*` paints backgrounds, `Text*` paints text,
/// `BorderOutline` paints a hairline, `Rounded` rounds corners.
/// Resolvers ignore tokens outside their domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    BgSurface,
    BgSurfaceVariant,
    BgPrimary,
    Text,
    TextPrimary,
    TextDim,
    TextFaint,
    TextOnPrimary,
    TextDisabled,
    BorderOutline,
    Rounded,
}

/// The single mapping table: what each token means for one theme.
/// Public resolvers (`color`, `container_style`) and the button factories
/// below all read through here, so a palette tweak propagates everywhere.
fn shade(a: &ActiveTheme, class: Class) -> Color {
    match class {
        Class::BgSurface => a.surface,
        Class::BgSurfaceVariant => a.surface_variant,
        Class::BgPrimary => a.primary,
        Class::Text => a.on_surface,
        Class::TextPrimary => a.primary,
        Class::TextDim => a.on_surface.scale_alpha(0.7),
        Class::TextFaint => a.on_surface.scale_alpha(0.8),
        Class::TextOnPrimary => a.on_primary,
        Class::TextDisabled => a.on_surface.scale_alpha(0.5),
        Class::BorderOutline => a.outline,
        // No color: resolve transparent (prefer `container_style`, which
        // applies it as a radius instead).
        Class::Rounded => Color::TRANSPARENT,
    }
}

/// Resolve one token to a color against the active theme.
pub fn color(class: Class) -> Color {
    shade(&active(), class)
}

/// Fold tokens into one container style: `Bg*` sets the background,
/// `Text*` the text color, `BorderOutline` the hairline, `Rounded` the
/// radius. Later tokens override earlier ones.
pub fn container_style(classes: &[Class]) -> impl Fn(&Theme) -> container::Style {
    let classes = classes.to_vec();
    move |_| resolve_container(&classes)
}

fn resolve_container(classes: &[Class]) -> container::Style {
    let a = active();
    let mut style = container::Style::default();
    for class in classes {
        match class {
            Class::BgSurface | Class::BgSurfaceVariant | Class::BgPrimary => {
                style.background = Some(shade(&a, *class).into());
            }
            Class::Text
            | Class::TextPrimary
            | Class::TextDim
            | Class::TextFaint
            | Class::TextOnPrimary
            | Class::TextDisabled => {
                style.text_color = Some(shade(&a, *class));
            }
            Class::BorderOutline => {
                style.border.color = shade(&a, *class);
                style.border.width = BORDER_WIDTH;
            }
            Class::Rounded => style.border.radius = RADIUS.into(),
        }
    }
    style
}

// ---------------------------------------------------------------------------
// Roles — named colors, following reshell component conventions.
// Thin aliases over [`Class`] so existing call sites stay one word.
// ---------------------------------------------------------------------------

/// Default text on surfaces.
pub fn text() -> Color {
    color(Class::Text)
}

/// Dim label text (debug label over wallpapers).
pub fn text_dim() -> Color {
    color(Class::TextDim)
}

/// Map overlay label text.
pub fn map_label() -> Color {
    color(Class::TextFaint)
}

/// Card background (context menus, settings map boxes): reshell `Menu`
/// paints `surface` with an `outline` border.
pub fn card() -> Color {
    color(Class::BgSurface)
}

/// Button / menu-item label text: reshell `Menu` items paint `primary`.
pub fn button_text() -> Color {
    color(Class::TextPrimary)
}

/// Hairline border for every bordered surface/button.
pub fn border_color() -> Color {
    color(Class::BorderOutline)
}

/// Default corner radius for cards and buttons.
pub const RADIUS: f32 = 6.0;
/// Hairline width shared by all bordered styles.
pub const BORDER_WIDTH: f32 = 1.0;

// Canvas (display-map) colors: bespoke alpha blends, kept as plain fns.
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
// Containers — `container(x).style(theme::menu_box)` etc., each one token
// list (see `container_style` for ad-hoc combinations).
// ---------------------------------------------------------------------------

/// Card surface: context menus, settings map boxes.
pub fn menu_box(theme: &Theme) -> container::Style {
    container_style(&[
        Class::BgSurface,
        Class::Text,
        Class::BorderOutline,
        Class::Rounded,
    ])(theme)
}

/// Opaque top-bar backdrop.
pub fn bar(theme: &Theme) -> container::Style {
    container_style(&[Class::BgSurface, Class::Text])(theme)
}

/// Drag-selection rectangle. `opacity` is the speed-scaled fade value (`1.0`
/// while selecting); `radius` is the per-corner radius so corners clipped by
/// the output edge render square instead of sliced.
pub fn selection_box(
    opacity: f32,
    radius: iced::border::Radius,
) -> impl Fn(&Theme) -> container::Style {
    let a = active();
    // Fade rides on `Background::scale_alpha` (base fill at rest alpha ×
    // live fade factor), so gradient fills would fade correctly too —
    // not just the solid color. Border stays `Color::scale_alpha`
    // (`Border.color` is a `Color`, no `Background` API there).
    let background = iced::Background::Color(a.primary).scale_alpha(0.5 * opacity);
    let border_color = a.primary.scale_alpha(opacity);
    move |_| container::Style {
        background: Some(background),
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

/// Press feedback uses a primary-tinted surface and a primary border.
/// Explicit label tints remain readable because the fill stays mostly surface.
fn pressed_menu_style(a: &ActiveTheme, text: Color, radius: f32) -> button::Style {
    let surface = a.surface_variant;
    let tint = 0.2;
    let background = Color::from_rgba(
        surface.r * (1.0 - tint) + a.primary.r * tint,
        surface.g * (1.0 - tint) + a.primary.g * tint,
        surface.b * (1.0 - tint) + a.primary.b * tint,
        surface.a,
    );
    let mut style = button_base(background, text, radius);
    style.border.color = a.primary;
    style
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
            button::Status::Hovered => button_base(
                shade(&a, Class::BgSurfaceVariant),
                shade(&a, Class::Text),
                rounding,
            ),
            button::Status::Pressed => pressed_menu_style(&a, shade(&a, Class::Text), rounding),
            button::Status::Disabled => button_base(
                shade(&a, Class::BgSurface),
                shade(&a, Class::TextDisabled),
                rounding,
            ),
            button::Status::Active => button_base(
                shade(&a, Class::BgSurface),
                shade(&a, Class::TextPrimary),
                rounding,
            ),
        }
    }
}

/// Context-menu / generic raised button with an explicit label color
/// (Lua `:color()`), same surfaces as [`menu_button`]; every status
/// paints the label in `text`.
pub fn menu_button_tinted(
    rounding: f32,
    text: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let a = active();
        match status {
            button::Status::Hovered => {
                button_base(shade(&a, Class::BgSurfaceVariant), text, rounding)
            }
            button::Status::Pressed => pressed_menu_style(&a, text, rounding),
            button::Status::Disabled => button_base(shade(&a, Class::BgSurface), text, rounding),
            button::Status::Active => button_base(shade(&a, Class::BgSurface), text, rounding),
        }
    }
}

/// Settings nav button; selected pages paint `primary`/`on_primary`.
pub fn nav_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, _| {
        let a = active();
        if selected {
            button_base(
                shade(&a, Class::BgPrimary),
                shade(&a, Class::TextOnPrimary),
                RADIUS,
            )
        } else {
            button_base(
                shade(&a, Class::BgSurfaceVariant),
                shade(&a, Class::Text),
                RADIUS,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Theme previews — Settings Theme page rows
// ---------------------------------------------------------------------------

/// Resolve one variant for preview rows: the listed theme's own colors
/// under the current dark/light mode, independent of the active theme.
pub fn preview(name: &str, darkmode: bool) -> ActiveTheme {
    resolve(&ThemeConfig {
        name: name.to_string(),
        darkmode,
        ..Default::default()
    })
}

/// Preview card button: `surface` background, `on_surface` text, selected
/// rows ringed in `primary` (2px, else a hairline `outline`). Captures the
/// previewed theme so every row shows its own palette, not the active one.
pub fn preview_button(
    previewed: ActiveTheme,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(
                if hovered {
                    previewed.surface_variant
                } else {
                    previewed.surface
                }
                .into(),
            ),
            text_color: previewed.on_surface,
            border: Border {
                color: if selected {
                    previewed.primary
                } else {
                    previewed.outline
                },
                width: if selected { 2.0 } else { BORDER_WIDTH },
                radius: RADIUS.into(),
            },
            ..Default::default()
        }
    }
}

/// Single palette swatch box for preview rows.
pub fn swatch(color: Color, border: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(color.into()),
        border: Border {
            color: border,
            width: BORDER_WIDTH,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}
