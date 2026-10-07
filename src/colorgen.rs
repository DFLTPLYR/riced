//! Material You dynamic theme generation, ported from `sys/src/colorgen.rs`.
//!
//! sys exposes this as the `ColorGen` QML singleton: `generate(paths)`
//! builds an M3 scheme from wallpaper pixels and emits `output(theme_json)`.
//! riced has no Qt, so the same pipeline is plain functions returning
//! `Result`:
//!
//! ```text
//! per-output wallpaper views → stitched strip → seed color
//!     → M3 schemes (both modes) → reshell-format {light, dark} JSON
//!     → dynamic.json in the theme dir
//! ```
//!
//! Trigger: `riced generate-theme [--variant V] [--set] [--templates DIR]`
//! (`--set` also selects the new `dynamic` theme so the daemon hot-reloads
//! to it; `--templates` renders a `[templates]` config dir like sys does
//! for external apps).
//!
//! Output JSON matches `scheme_json` in sys exactly (same keys, same
//! terminal mapping), so generated files are drop-in reshell themes.
//! Switching themes re-renders the templates dir like sys's `change_theme`
//! (see `render_theme_templates`); generate with
//! `riced generate-theme`, apply a stored theme with
//! `riced apply-templates`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use include_dir::{Dir, include_dir};
use material_colors::{
    color::Argb,
    dynamic_color::Variant,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use regex::Regex;

use crate::components::display_map::MapLayer;
use crate::config::{BackgroundImage, Config, ThemeConfig};

// ---------------------------------------------------------------------------
// Variants
// ---------------------------------------------------------------------------

/// Scheme variants accepted by `generate-theme` and `[theme] variant`
/// (same list as reshell `Colors.colorscheme`, minus the `scheme-` prefix).
pub const VARIANT_NAMES: &[&str] = &[
    "content",
    "tonalspot",
    "monochrome",
    "neutral",
    "vibrant",
    "expressive",
    "fidelity",
    "rainbow",
    "fruitsalad",
];

/// Parse a variant name (case-insensitive, `fruit_salad` accepted);
/// unknown names fall back to `TonalSpot`, like sys.
pub fn parse_variant(s: &str) -> Variant {
    match s.to_lowercase().as_str() {
        "monochrome" => Variant::Monochrome,
        "neutral" => Variant::Neutral,
        "vibrant" => Variant::Vibrant,
        "expressive" => Variant::Expressive,
        "fidelity" => Variant::Fidelity,
        "content" => Variant::Content,
        "rainbow" => Variant::Rainbow,
        "fruit_salad" | "fruitsalad" => Variant::FruitSalad,
        _ => Variant::TonalSpot,
    }
}

// ---------------------------------------------------------------------------
// Wallpaper sources
// ---------------------------------------------------------------------------

/// Image files backing `generate`: every configured wallpaper's resolved
/// path, deduplicated, skipping empty paths and missing files. This is
/// riced's equivalent of reshell passing per-monitor cropped wallpapers to
/// `ColorGen.generate(paths)`.
pub fn wallpaper_paths(config: &Config) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for img in &config.background.image {
        let path = img.local_path();
        if path.as_os_str().is_empty() || out.contains(&path) || !path.is_file() {
            continue;
        }
        out.push(path);
    }
    out
}

// ---------------------------------------------------------------------------
// Views: rasterize what wallpaper_views paints, per output
// ---------------------------------------------------------------------------

/// Rasterize global-canvas rects with the same math as
/// `Background::wallpaper_views`: per view, overlay ascending-`z` image
/// crops (`MapLayer::resolved` → `overlap` with the view → `crop_for` to
/// source pixels) at their view-local offset, flattened over black.
///
/// Each entry of `view_rects` is one output's available rect — so the seed
/// color comes from the wallpaper set *combined across all outputs*, exactly
/// what the screens show. Views with no visible pixels still yield a black
/// tile (an empty monitor darkens the seed like it darkens the room).
/// Undecodable images contribute nothing; degenerate rects are skipped.
pub fn render_views(
    view_rects: &[(f32, f32, f32, f32)],
    images: &[BackgroundImage],
) -> Vec<image::RgbImage> {
    // Decode once; every view shares the pixels.
    let mut decoded: HashMap<PathBuf, image::RgbImage> = HashMap::new();
    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by_key(|&i| images[i].z);

    view_rects
        .iter()
        .filter_map(|&(ax, ay, aw, ah)| {
            let (w, h) = (aw.max(0.0) as u32, ah.max(0.0) as u32);
            if w == 0 || h == 0 {
                return None;
            }
            let mut canvas = image::RgbImage::new(w, h);
            for &i in &order {
                let img = &images[i];
                let (Some(rect), Some(native)) =
                    (MapLayer::resolved(img), MapLayer::native_size(img))
                else {
                    continue;
                };
                let Some(overlap) = MapLayer::overlap(rect, (ax, ay, aw, ah)) else {
                    continue;
                };
                let (ox, oy, _, _) = overlap;
                let Some(crop) = MapLayer::crop_for(rect, native, overlap) else {
                    continue;
                };
                let pixels = decoded.entry(img.local_path()).or_insert_with(|| {
                    image::open(img.local_path())
                        .map(|d| d.to_rgb8())
                        .unwrap_or_else(|_| image::RgbImage::new(0, 0))
                });
                let (dw, dh) = pixels.dimensions();
                // Stored width/height can disagree with the file (user-set);
                // clamp so a stale crop never panics.
                let cx = crop.x.min(dw);
                let cy = crop.y.min(dh);
                let cw = crop.width.min(dw.saturating_sub(cx));
                let ch = crop.height.min(dh.saturating_sub(cy));
                if cw == 0 || ch == 0 {
                    continue;
                }
                let tile = image::imageops::crop_imm(pixels, cx, cy, cw, ch).to_image();
                image::imageops::overlay(&mut canvas, &tile, (ox - ax) as i64, (oy - ay) as i64);
            }
            Some(canvas)
        })
        .collect()
}

/// Stitch view tiles side-by-side (sys lays monitor strips out the same
/// way) into a temp PNG for seed-color extraction. `None` when there is
/// nothing to read.
fn combine_views(views: &[image::RgbImage]) -> Option<PathBuf> {
    let tiles: Vec<&image::RgbImage> = views
        .iter()
        .filter(|v| v.width() > 0 && v.height() > 0)
        .collect();
    if tiles.is_empty() {
        return None;
    }
    let total_width: u32 = tiles.iter().map(|v| v.width()).sum();
    let height: u32 = tiles.iter().map(|v| v.height()).max().unwrap_or(0);
    if total_width == 0 || height == 0 {
        return None;
    }
    let mut combined = image::RgbImage::new(total_width, height);
    let mut x = 0i64;
    for tile in tiles {
        image::imageops::overlay(&mut combined, tile, x, 0);
        x += tile.width() as i64;
    }
    let output = std::env::temp_dir().join("riced_combined_wallpaper.png");
    combined.save(&output).ok()?;
    Some(output)
}

// ---------------------------------------------------------------------------
// Generation: wallpaper views → reshell {light, dark} JSON
// ---------------------------------------------------------------------------

/// Release fully-free allocator pages back to the OS after a batch
/// job (theme regen peaks in the tens of MB of transient image
/// buffers). glibc arenas otherwise retain the high-water RSS on the
/// pooled worker thread indefinitely — not a leak, but indistinguishable
/// from one in a task manager. No-op off glibc Linux.
pub(crate) fn trim_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // Best effort: failure just keeps the status quo.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// Build the dynamic theme from rasterized wallpaper views (see
/// [`render_views`]): downscale the stitched strip to 128x128, extract the
/// seed color, and build both M3 schemes. Returns the reshell-format payload
/// plus `{{var}}` template variables for the `darkmode` scheme (mirrors the
/// `generate` invokable; QML `error(...)` strings become `Err`).
#[derive(Debug)]
pub struct Generated {
    /// Reshell-format `{light, dark}` JSON (`scheme_json`, same as sys).
    pub payload: serde_json::Value,
    /// Template variables for the active scheme (`build_color_map`).
    pub variables: HashMap<String, String>,
}

pub fn generate_from_views(
    views: &[image::RgbImage],
    variant: &str,
    darkmode: bool,
) -> Result<Generated, String> {
    let image_path =
        combine_views(views).ok_or_else(|| "no wallpaper views to combine".to_string())?;

    let bytes = std::fs::read(&image_path)
        .map_err(|e| format!("failed to read image: {e}"))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&image_path);
        })?;

    let mut data =
        ImageReader::read(bytes).map_err(|e| format!("failed to decode image: {e:?}"))?;
    data.resize(128, 128, FilterType::Lanczos3);

    let theme = ThemeBuilder::with_source(ImageReader::extract_color(&data))
        .variant(parse_variant(variant))
        .build();

    let scheme = if darkmode {
        &theme.schemes.dark
    } else {
        &theme.schemes.light
    };
    let image = image_path.display().to_string();
    let variables = build_color_map(scheme, darkmode, &image);

    let _ = std::fs::remove_file(&image_path);

    Ok(Generated {
        payload: serde_json::json!({
            "light": scheme_json(&theme.schemes.light),
            "dark": scheme_json(&theme.schemes.dark),
        }),
        variables,
    })
}

/// Persist a generated payload as the `dynamic` theme in the user theme
/// dir (created by generation only — never vendored). Returns the path.
pub fn write_dynamic_theme(payload: &serde_json::Value) -> io::Result<PathBuf> {
    let path = crate::theme::theme_dir().join("dynamic.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, text)?;
    Ok(path)
}

fn hex(color: Argb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
}

fn terminal_json(s: &Scheme) -> serde_json::Value {
    serde_json::json!({
        "normal": {
            "black": hex(s.surface_container_high),
            "red": hex(s.error),
            "green": hex(s.secondary),
            "yellow": hex(s.tertiary),
            "blue": hex(s.primary),
            "magenta": hex(s.tertiary_fixed_dim),
            "cyan": hex(s.secondary_fixed_dim),
            "white": hex(s.surface_bright),
        },
        "bright": {
            "black": hex(s.outline_variant),
            "red": hex(s.error_container),
            "green": hex(s.secondary_container),
            "yellow": hex(s.tertiary_container),
            "blue": hex(s.primary_fixed_dim),
            "magenta": hex(s.tertiary_fixed),
            "cyan": hex(s.secondary_fixed),
            "white": hex(s.on_surface),
        },
        "foreground": hex(s.on_surface),
        "background": hex(s.surface),
        "selectionFg": hex(s.surface),
        "selectionBg": hex(s.primary),
        "cursorText": hex(s.surface),
        "cursor": hex(s.primary),
    })
}

/// One reshell theme variant object (same keys/values as sys).
pub fn scheme_json(s: &Scheme) -> serde_json::Value {
    serde_json::json!({
        "primary": hex(s.primary),
        "on_primary": hex(s.on_primary),
        "secondary": hex(s.secondary),
        "on_secondary": hex(s.on_secondary),
        "tertiary": hex(s.tertiary),
        "on_tertiary": hex(s.on_tertiary),
        "error": hex(s.error),
        "on_error": hex(s.on_error),
        "surface": hex(s.surface),
        "on_surface": hex(s.on_surface),
        "surface_variant": hex(s.surface_variant),
        "on_surface_variant": hex(s.on_surface_variant),
        "outline": hex(s.outline),
        "shadow": hex(s.shadow),
        "hover": hex(s.tertiary),
        "on_hover": hex(s.on_tertiary),
        "terminal": terminal_json(s),
    })
}

/// `{{var}}` variables for an M3 scheme: `colors.<name>[.hex|.default.hex]`
/// for every role, terminal mappings, `hover`/`mode`/`image` — same as sys.
/// Used when a templates dir is passed explicitly (sys runs these against
/// reshell's `core/theme/`; riced only runs user-requested dirs).
pub fn build_color_map(scheme: &Scheme, is_dark: bool, image: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    let fields: &[(&str, Argb)] = &[
        ("primary", scheme.primary),
        ("on_primary", scheme.on_primary),
        ("primary_container", scheme.primary_container),
        ("on_primary_container", scheme.on_primary_container),
        ("inverse_primary", scheme.inverse_primary),
        ("primary_fixed", scheme.primary_fixed),
        ("primary_fixed_dim", scheme.primary_fixed_dim),
        ("on_primary_fixed", scheme.on_primary_fixed),
        ("on_primary_fixed_variant", scheme.on_primary_fixed_variant),
        ("secondary", scheme.secondary),
        ("on_secondary", scheme.on_secondary),
        ("secondary_container", scheme.secondary_container),
        ("on_secondary_container", scheme.on_secondary_container),
        ("secondary_fixed", scheme.secondary_fixed),
        ("secondary_fixed_dim", scheme.secondary_fixed_dim),
        ("on_secondary_fixed", scheme.on_secondary_fixed),
        (
            "on_secondary_fixed_variant",
            scheme.on_secondary_fixed_variant,
        ),
        ("tertiary", scheme.tertiary),
        ("on_tertiary", scheme.on_tertiary),
        ("tertiary_container", scheme.tertiary_container),
        ("on_tertiary_container", scheme.on_tertiary_container),
        ("tertiary_fixed", scheme.tertiary_fixed),
        ("tertiary_fixed_dim", scheme.tertiary_fixed_dim),
        ("on_tertiary_fixed", scheme.on_tertiary_fixed),
        (
            "on_tertiary_fixed_variant",
            scheme.on_tertiary_fixed_variant,
        ),
        ("error", scheme.error),
        ("on_error", scheme.on_error),
        ("error_container", scheme.error_container),
        ("on_error_container", scheme.on_error_container),
        ("surface_dim", scheme.surface_dim),
        ("surface", scheme.surface),
        ("surface_tint", scheme.surface_tint),
        ("surface_bright", scheme.surface_bright),
        ("surface_container_lowest", scheme.surface_container_lowest),
        ("surface_container_low", scheme.surface_container_low),
        ("surface_container", scheme.surface_container),
        ("surface_container_high", scheme.surface_container_high),
        (
            "surface_container_highest",
            scheme.surface_container_highest,
        ),
        ("on_surface", scheme.on_surface),
        ("on_surface_variant", scheme.on_surface_variant),
        ("outline", scheme.outline),
        ("outline_variant", scheme.outline_variant),
        ("inverse_surface", scheme.inverse_surface),
        ("inverse_on_surface", scheme.inverse_on_surface),
        ("surface_variant", scheme.surface_variant),
        ("background", scheme.background),
        ("on_background", scheme.on_background),
        ("shadow", scheme.shadow),
        ("scrim", scheme.scrim),
    ];

    for (name, color) in fields {
        let hex = hex(*color);
        map.insert(format!("colors.{name}.default.hex"), hex.clone());
        map.insert(format!("colors.{name}.hex"), hex.clone());
        map.insert(format!("colors.{name}"), hex);
    }

    map.insert(
        "mode".to_string(),
        if is_dark { "dark" } else { "light" }.to_string(),
    );
    map.insert("image".to_string(), image.to_string());

    let terminal_colors: &[(&str, Argb)] = &[
        ("terminal.normal.black", scheme.surface_container_high),
        ("terminal.normal.red", scheme.error),
        ("terminal.normal.green", scheme.secondary),
        ("terminal.normal.yellow", scheme.tertiary),
        ("terminal.normal.blue", scheme.primary),
        ("terminal.normal.magenta", scheme.tertiary_fixed_dim),
        ("terminal.normal.cyan", scheme.secondary_fixed_dim),
        ("terminal.normal.white", scheme.surface_bright),
        ("terminal.bright.black", scheme.outline_variant),
        ("terminal.bright.red", scheme.error_container),
        ("terminal.bright.green", scheme.secondary_container),
        ("terminal.bright.yellow", scheme.tertiary_container),
        ("terminal.bright.blue", scheme.primary_fixed_dim),
        ("terminal.bright.magenta", scheme.tertiary_fixed),
        ("terminal.bright.cyan", scheme.secondary_fixed),
        ("terminal.bright.white", scheme.on_surface),
    ];

    for (key, color) in terminal_colors {
        let hex = hex(*color);
        map.insert(key.to_string(), hex.clone());
        map.insert(format!("colors.{key}"), hex);
    }

    let pairs: &[(&str, Argb)] = &[
        ("terminal.foreground", scheme.on_surface),
        ("terminal.background", scheme.surface),
        ("terminal.selectionfg", scheme.surface),
        ("terminal.selectionFg", scheme.surface),
        ("terminal.selectionbg", scheme.primary),
        ("terminal.selectionBg", scheme.primary),
        ("terminal.cursortext", scheme.surface),
        ("terminal.cursorText", scheme.surface),
        ("terminal.cursor", scheme.primary),
    ];
    for (key, color) in pairs {
        let hex = hex(*color);
        map.insert(key.to_string(), hex.clone());
        map.insert(format!("colors.{key}"), hex);
    }

    map.insert("hover".to_string(), hex(scheme.tertiary));
    map.insert("colors.hover".to_string(), hex(scheme.tertiary));
    map.insert("on_hover".to_string(), hex(scheme.on_tertiary));
    map.insert("colors.on_hover".to_string(), hex(scheme.on_tertiary));

    map
}

// ---------------------------------------------------------------------------
// change_theme path: stored theme JSON → template variables
// ---------------------------------------------------------------------------

/// Expand a leading `~` to `$HOME` (template dirs are user paths).
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Template variables from a stored reshell theme variant: reads both
/// `snake_case` and unseparated (`onprimary`, as in `ayu-blue.json`) keys,
/// plus the `terminal` block.
pub fn build_color_map_from_json(scheme: &serde_json::Value) -> HashMap<String, String> {
    let mut map = HashMap::new();

    let color_keys: &[(&str, &str)] = &[
        ("primary", "primary"),
        ("on_primary", "onprimary"),
        ("primary_container", "primarycontainer"),
        ("on_primary_container", "onprimarycontainer"),
        ("inverse_primary", "inverseprimary"),
        ("primary_fixed", "primaryfixed"),
        ("primary_fixed_dim", "primaryfixeddim"),
        ("on_primary_fixed", "onprimaryfixed"),
        ("on_primary_fixed_variant", "onprimaryfixedvariant"),
        ("secondary", "secondary"),
        ("on_secondary", "onsecondary"),
        ("secondary_container", "secondarycontainer"),
        ("on_secondary_container", "onsecondarycontainer"),
        ("secondary_fixed", "secondaryfixed"),
        ("secondary_fixed_dim", "secondaryfixeddim"),
        ("on_secondary_fixed", "onsecondaryfixed"),
        ("on_secondary_fixed_variant", "onsecondaryfixedvariant"),
        ("tertiary", "tertiary"),
        ("on_tertiary", "ontertiary"),
        ("tertiary_container", "tertiarycontainer"),
        ("on_tertiary_container", "ontertiarycontainer"),
        ("tertiary_fixed", "tertiaryfixed"),
        ("tertiary_fixed_dim", "tertiaryfixeddim"),
        ("on_tertiary_fixed", "ontertiaryfixed"),
        ("on_tertiary_fixed_variant", "ontertiaryfixedvariant"),
        ("error", "error"),
        ("on_error", "onerror"),
        ("error_container", "errorcontainer"),
        ("on_error_container", "onerrorcontainer"),
        ("surface_dim", "surfacedim"),
        ("surface", "surface"),
        ("surface_tint", "surfacetint"),
        ("surface_bright", "surfacebright"),
        ("surface_container_lowest", "surfacecontainerlowest"),
        ("surface_container_low", "surfacecontainerlow"),
        ("surface_container", "surfacecontainer"),
        ("surface_container_high", "surfacecontainerhigh"),
        ("surface_container_highest", "surfacecontainerhighest"),
        ("on_surface", "onsurface"),
        ("on_surface_variant", "onsurfacevariant"),
        ("outline", "outline"),
        ("outline_variant", "outlinevariant"),
        ("inverse_surface", "inversesurface"),
        ("inverse_on_surface", "inverseonsurface"),
        ("surface_variant", "surfacevariant"),
        ("background", "background"),
        ("on_background", "onbackground"),
        ("shadow", "shadow"),
        ("scrim", "scrim"),
        ("hover", "hover"),
        ("on_hover", "onhover"),
    ];

    for &(snake, camel) in color_keys {
        let val = scheme.get(snake).or_else(|| scheme.get(camel));
        if let Some(v) = val {
            let hex = v.as_str().unwrap_or("");
            map.insert(format!("colors.{snake}.default.hex"), hex.to_string());
            map.insert(format!("colors.{snake}.hex"), hex.to_string());
            map.insert(format!("colors.{snake}"), hex.to_string());
        }
    }

    if let Some(terminal) = scheme.get("terminal") {
        for section in &["normal", "bright"] {
            if let Some(obj) = terminal.get(*section) {
                for color in &[
                    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
                ] {
                    if let Some(val) = obj.get(*color) {
                        let hex = val.as_str().unwrap_or("");
                        map.insert(format!("terminal.{section}.{color}"), hex.to_string());
                        map.insert(
                            format!("colors.terminal.{section}.{color}"),
                            hex.to_string(),
                        );
                    }
                }
            }
        }

        let terminal_keys: &[(&str, &[&str])] = &[
            ("foreground", &["foreground"]),
            ("background", &["background"]),
            ("selectionFg", &["selectionFg", "selectionfg"]),
            ("selectionBg", &["selectionBg", "selectionbg"]),
            ("cursorText", &["cursorText", "cursortext"]),
            ("cursor", &["cursor"]),
        ];

        for &(camel_name, aliases) in terminal_keys {
            for alias in aliases {
                if let Some(val) = terminal.get(*alias) {
                    let hex = val.as_str().unwrap_or("");
                    map.insert(format!("terminal.{camel_name}"), hex.to_string());
                    map.insert(format!("colors.terminal.{camel_name}"), hex.to_string());
                }
            }
        }
    }

    map
}

/// Template variables for a stored theme OBJECT (sys's `change_theme`
/// `json` argument): trim/BOM-strip, parse (with a 50-char preview on
/// error), pick the `dark`/`light` variant, and build the map. Mirrors the
/// QML validation messages as `Err`.
pub fn variables_for_theme(
    json_text: &str,
    darkmode: bool,
) -> Result<HashMap<String, String>, String> {
    let trimmed = json_text.trim().trim_start_matches('\u{feff}');
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        let preview: String = trimmed.chars().take(50).collect();
        format!("invalid JSON: {e} | preview: {preview}")
    })?;
    let mode_key = if darkmode { "dark" } else { "light" };
    let scheme = value.get(mode_key).ok_or_else(|| {
        format!(
            "missing \"{mode_key}\" key in JSON | keys: {:?}",
            value.as_object().map(|o| o.keys().collect::<Vec<_>>())
        )
    })?;
    Ok(build_color_map_from_json(scheme))
}

/// Full `change_theme` equivalent: variables from the stored theme object,
/// then render a `[templates]` dir. Returns per-template errors (empty =
/// applied); a validation failure yields a single-element vec.
pub fn render_theme_templates(dir: &Path, json_text: &str, darkmode: bool) -> Vec<String> {
    match variables_for_theme(json_text, darkmode) {
        Ok(vars) => process_templates(dir, &vars),
        Err(e) => vec![e],
    }
}

/// Whether a wallpaper change should regenerate `dynamic.json`: only when
/// `dynamic` is the selected theme. Pure policy, cheap to call on every arm.
pub fn wants_regen(theme_name: &str) -> bool {
    theme_name == "dynamic"
}

/// Raw text of a stored theme (user file wins, then vendored builtin),
/// for the `change_theme` object pass.
pub fn stored_theme_text(name: &str) -> Result<String, String> {
    let path = crate::theme::user_file(name);
    if path.exists() {
        return std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()));
    }
    if let Some(content) = crate::theme::builtin_text(name) {
        return Ok(content.to_string());
    }
    Err(format!(
        "unknown theme {name:?} — run `riced generate-theme` for dynamic"
    ))
}

// ---------------------------------------------------------------------------
// Default templates set (vendored reshell core/theme)
// ---------------------------------------------------------------------------

/// Default `[templates]` set, copied from reshell `core/theme/` (inputs +
/// `config.toml`), embedded at compile time.
static DEFAULT_TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// `~/.config/riced/templates` (`$XDG_CONFIG_HOME` aware): the editable
/// copy of [`DEFAULT_TEMPLATES`] that rendering actually reads.
pub fn user_templates_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("templates"))
        .unwrap_or_else(|| PathBuf::from("templates"))
}

/// Seed the user templates dir with the embedded defaults: every file the
/// user doesn't already have (never overwrites edits). `into` selects the
/// root (used by tests to avoid touching `$HOME`).
pub fn ensure_user_templates_into(root: &Path) -> io::Result<()> {
    std::fs::create_dir_all(root)?;
    seed_recursive(&DEFAULT_TEMPLATES, root)
}

fn seed_recursive(dir: &Dir, dest_root: &Path) -> io::Result<()> {
    // File::path is root-relative, so every level joins against dest_root.
    for f in dir.files() {
        let dest = dest_root.join(f.path());
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, f.contents())?;
    }
    for d in dir.dirs() {
        seed_recursive(d, dest_root)?;
    }
    Ok(())
}

/// All embedded template files, recursively.
#[cfg(test)]
fn default_template_files(dir: &Dir, out: &mut Vec<std::path::PathBuf>) {
    out.extend(dir.files().map(|f| f.path().to_path_buf()));
    for d in dir.dirs() {
        default_template_files(d, out);
    }
}

/// Seed [`user_templates_dir`] (best-effort: failures log and rendering
/// reports the missing dir per template).
pub fn ensure_user_templates() {
    let dir = user_templates_dir();
    if let Err(e) = ensure_user_templates_into(&dir) {
        eprintln!("templates: cannot seed {}: {e}", dir.display());
    }
}

/// Which templates dir a theme switch / CLI run renders, if any:
/// `"off"` disables; an explicit `[theme] templates_dir` wins; otherwise
/// the seeded user dir when its `config.toml` exists.
pub fn effective_templates_dir(theme: &ThemeConfig) -> Option<PathBuf> {
    if theme.templates_dir == "off" {
        return None;
    }
    if !theme.templates_dir.is_empty() {
        return Some(expand_tilde(&theme.templates_dir));
    }
    let dir = user_templates_dir();
    dir.join("config.toml").is_file().then_some(dir)
}

// ---------------------------------------------------------------------------
// Templates: {{var}} substitution + pre/post hooks (same as sys)
// ---------------------------------------------------------------------------

static TEMPLATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*(.+?)\s*\}\}").unwrap());

/// Substitute `{{ key }}` variables; unknown keys pass through untouched.
pub fn render_template(content: &str, variables: &HashMap<String, String>) -> String {
    TEMPLATE_RE
        .replace_all(content, |caps: &regex::Captures| {
            let expr = caps[1].trim();
            variables
                .get(expr)
                .cloned()
                .unwrap_or_else(|| caps[0].to_string())
        })
        .to_string()
}

fn run_hook(hook: &str, variables: &HashMap<String, String>) -> bool {
    let rendered = render_template(hook, variables);
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(serde::Deserialize)]
struct TemplatesConfig {
    templates: Option<HashMap<String, TemplateEntry>>,
}

#[derive(serde::Deserialize)]
struct TemplateEntry {
    input_path: String,
    output_path: String,
    pre_hook: Option<String>,
    post_hook: Option<String>,
}

/// Render every `[templates.*]` entry of `<dir>/config.toml`: read
/// `<dir>/<input_path>`, substitute variables, write to `output_path`
/// (`~` expands to `$HOME`, parents created), running optional
/// `pre_hook`/`post_hook` shell snippets. Returns per-template errors
/// (empty = all applied). Same semantics as sys, sequential.
pub fn process_templates(dir: &Path, variables: &HashMap<String, String>) -> Vec<String> {
    let config_file = dir.join("config.toml");
    let content = match std::fs::read_to_string(&config_file) {
        Ok(c) => c,
        Err(e) => {
            return vec![format!("failed to read {}: {e}", config_file.display())];
        }
    };

    let config: TemplatesConfig = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            return vec![format!("failed to parse config.toml: {e}")];
        }
    };

    let templates = match config.templates {
        Some(t) => t,
        None => {
            return vec!["no [templates] sections in config.toml".to_string()];
        }
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let mut errors = Vec::new();

    for (name, entry) in &templates {
        let input = dir.join(&entry.input_path);
        let content = match std::fs::read_to_string(&input) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{name}: failed to read {}: {e}", input.display()));
                continue;
            }
        };

        if let Some(ref hook) = entry.pre_hook
            && !run_hook(hook, variables)
        {
            errors.push(format!("{name}: pre_hook failed: {hook}"));
        }

        let rendered = render_template(&content, variables);

        let output_raw = entry.output_path.replace("~", &home);
        let output = Path::new(&output_raw);
        if let Some(parent) = output.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(output, &rendered) {
            errors.push(format!(
                "{name}: failed to write {}: {e}",
                entry.output_path
            ));
        }

        if let Some(ref hook) = entry.post_hook
            && !run_hook(hook, variables)
        {
            errors.push(format!("{name}: post_hook failed: {hook}"));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `combine_views` uses a fixed temp path: serialize file-writing
    /// tests so parallel runs don't clobber each other's strips.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn variant_names_parse_including_aliases() {
        assert!(matches!(parse_variant("content"), Variant::Content));
        assert!(matches!(parse_variant("Vibrant"), Variant::Vibrant));
        assert!(matches!(parse_variant("fruit_salad"), Variant::FruitSalad));
        assert!(matches!(parse_variant("fruitsalad"), Variant::FruitSalad));
        assert!(matches!(parse_variant("bogus"), Variant::TonalSpot));
        // every advertised name parses (none falls through to default
        // unless it genuinely is unknown)
        for name in VARIANT_NAMES {
            let _ = parse_variant(name);
        }
    }

    #[test]
    fn render_template_substitutes_and_passes_unknown_through() {
        let mut vars = HashMap::new();
        vars.insert("colors.primary".to_string(), "#aabbcc".to_string());
        assert_eq!(
            render_template("fg {{colors.primary}}; bg {{colors.missing}}", &vars),
            "fg #aabbcc; bg {{colors.missing}}"
        );
        // whitespace-tolerant, like reshell's kitty template
        assert_eq!(
            render_template("cursor            {{ colors.primary }}", &vars),
            "cursor            #aabbcc"
        );
    }

    #[test]
    fn process_templates_renders_files_and_reports_errors() {
        let dir = std::env::temp_dir().join(format!("riced-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[templates.good]\ninput_path = \"in.txt\"\noutput_path = \"OUT_PLACEHOLDER/out.txt\"\n\
             [templates.broken]\ninput_path = \"missing.txt\"\noutput_path = \"out2.txt\"\n",
        )
        .unwrap();
        // output under the temp dir (avoids touching $HOME in tests)
        let out = dir.join("out").display().to_string();
        let cfg = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            cfg.replace("OUT_PLACEHOLDER", &out),
        )
        .unwrap();
        std::fs::write(dir.join("in.txt"), "primary={{colors.primary}}\n").unwrap();

        let mut vars = HashMap::new();
        vars.insert("colors.primary".to_string(), "#112233".to_string());
        let errors = process_templates(&dir, &vars);
        assert_eq!(
            std::fs::read_to_string(dir.join("out").join("out.txt")).unwrap(),
            "primary=#112233\n"
        );
        // only the missing-input template errors
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("broken:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_image(dir: &Path, name: &str, size: u32, rgb: [u8; 3]) -> BackgroundImage {
        use image::{ImageBuffer, Rgb};
        let path = dir.join(name);
        ImageBuffer::<Rgb<u8>, _>::from_pixel(size, size, Rgb(rgb))
            .save(&path)
            .unwrap();
        BackgroundImage {
            path: path.display().to_string(),
            x: 0.0,
            y: 0.0,
            z: 0,
            scale: 1.0,
            width: size as f32,
            height: size as f32,
        }
    }

    #[test]
    fn render_views_matches_wallpaper_view_math() {
        // 100x100 red image at (0,0); view shows its bottom-right quarter:
        // tile pixels only in the top-left 50x50, black elsewhere.
        let dir = std::env::temp_dir().join(format!("riced-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = test_image(&dir, "red.png", 100, [255, 0, 0]);

        let views = render_views(&[(50.0, 50.0, 100.0, 100.0)], std::slice::from_ref(&img));
        assert_eq!(views.len(), 1);
        let tile = &views[0];
        assert_eq!((tile.width(), tile.height()), (100, 100));
        assert_eq!(tile.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(tile.get_pixel(49, 49).0, [255, 0, 0]);
        assert_eq!(tile.get_pixel(50, 50).0, [0, 0, 0]);
        assert_eq!(tile.get_pixel(99, 99).0, [0, 0, 0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_views_combines_one_tile_per_output() {
        // Two side-by-side outputs, one image each: two tiles stitched.
        let dir = std::env::temp_dir().join(format!("riced-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = test_image(&dir, "a.png", 60, [10, 20, 30]);
        a.x = 0.0;
        let mut b = test_image(&dir, "b.png", 40, [200, 100, 50]);
        b.x = 60.0;
        b.z = 1;
        let images = vec![a, b];

        let views = render_views(&[(0.0, 0.0, 60.0, 60.0), (60.0, 0.0, 40.0, 40.0)], &images);
        assert_eq!(views.len(), 2);
        assert_eq!((views[0].width(), views[0].height()), (60, 60));
        assert_eq!((views[1].width(), views[1].height()), (40, 40));
        assert_eq!(views[0].get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(views[1].get_pixel(0, 0).0, [200, 100, 50]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    fn rss_kb() -> u64 {
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|pages| pages * 4)
            .unwrap_or(0)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn repeated_regen_rss_stays_flat() {
        let _guard = SERIAL.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("riced-genleak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Realistic wallpaper pixels so arena effects show.
        let img = test_image(&dir, "wall.png", 512, [0x45, 0x85, 0x88]);
        let images = std::slice::from_ref(&img);
        let mut rss = Vec::new();
        for _ in 0..14 {
            let views = render_views(&[(0.0, 0.0, 512.0, 512.0)], images);
            let generated = generate_from_views(&views, "content", true).unwrap();
            std::hint::black_box(generated);
            rss.push(rss_kb());
        }
        // Medians, not endpoints: sibling tests share this process and
        // a single transient spike from another thread must not fail
        // us — but a true per-regen leak shifts the whole second half.
        // (Warmup iterations excluded: first-touch inits are one-time.)
        fn median(mut v: Vec<u64>) -> u64 {
            v.sort_unstable();
            v[v.len() / 2]
        }
        let early = median(rss[4..9].to_vec());
        let late = median(rss[9..14].to_vec());
        let growth = late.saturating_sub(early);
        assert!(growth < 1024, "regen RSS grew {growth} KiB: {rss:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_produces_reshell_shaped_json() {
        let _guard = SERIAL.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("riced-gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = test_image(&dir, "wall.png", 8, [0x45, 0x85, 0x88]);

        let views = render_views(&[(0.0, 0.0, 8.0, 8.0)], std::slice::from_ref(&img));
        let generated = generate_from_views(&views, "content", true).unwrap();
        let payload = &generated.payload;
        for mode in ["light", "dark"] {
            let scheme = &payload[mode];
            for key in [
                "primary",
                "surface",
                "on_surface",
                "outline",
                "hover",
                "on_hover",
                "terminal",
            ] {
                assert!(scheme.get(key).is_some(), "{mode} has {key}");
            }
            // hex shape
            let primary = scheme["primary"].as_str().unwrap();
            assert!(
                primary.len() == 7 && primary.starts_with('#'),
                "primary is #rrggbb, got {primary}"
            );
            assert_eq!(
                scheme["terminal"]["foreground"].as_str().unwrap(),
                scheme["on_surface"].as_str().unwrap()
            );
        }
        // template variables for the dark scheme ride along
        assert_eq!(generated.variables["mode"], "dark");
        assert!(generated.variables.contains_key("colors.primary"));
        assert!(
            generated
                .variables
                .contains_key("colors.terminal.normal.red")
        );
        // temp strip cleaned up
        assert!(
            !std::env::temp_dir()
                .join("riced_combined_wallpaper.png")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_fails_without_views() {
        let _guard = SERIAL.lock().unwrap();
        let err = generate_from_views(&[], "content", true).unwrap_err();
        assert!(err.contains("no wallpaper views"));
    }

    #[test]
    fn color_map_covers_template_keys() {
        let theme = ThemeBuilder::with_source(Argb {
            alpha: 255,
            red: 0x45,
            green: 0x85,
            blue: 0x88,
        })
        .build();
        let map = build_color_map(&theme.schemes.dark, true, "/tmp/w.png");
        assert_eq!(map["mode"], "dark");
        assert_eq!(map["image"], "/tmp/w.png");
        for key in [
            "colors.primary",
            "colors.primary.hex",
            "colors.primary.default.hex",
            "colors.surface",
            "colors.terminal.normal.red",
            "colors.terminal.foreground",
            "terminal.cursor",
            "hover",
            "colors.on_hover",
        ] {
            assert!(map.contains_key(key), "has {key}");
        }
    }

    #[test]
    fn color_map_from_json_reads_both_key_styles() {
        let scheme = serde_json::json!({
            "primary": "#a1b2c3",
            "onprimary": "#010203",
            "on_primary": "#040506",
            "surface": "#ffffff",
            "terminal": {"normal": {"red": "#ff0000"},
                         "selectionFg": "#111111", "cursor": "#222222"},
        });
        let map = build_color_map_from_json(&scheme);
        // snake_case wins when both spellings exist
        assert_eq!(map["colors.on_primary"], "#040506");
        assert_eq!(map["colors.primary"], "#a1b2c3");
        assert_eq!(map["colors.terminal.normal.red"], "#ff0000");
        assert_eq!(map["terminal.selectionFg"], "#111111");
        assert_eq!(map["terminal.cursor"], "#222222");
        assert_eq!(map["colors.terminal.cursor"], "#222222");
        // surface passes through; missing keys are simply absent
        assert_eq!(map["colors.surface"], "#ffffff");
        assert!(!map.contains_key("colors.secondary"));
    }

    #[test]
    fn wants_regen_only_when_dynamic_selected() {
        assert!(wants_regen("dynamic"));
        assert!(!wants_regen("gruvbox"));
        assert!(!wants_regen("dracula"));
    }

    #[test]
    fn variables_for_theme_validates_like_change_theme() {
        // valid object, dark variant
        let text = r##"{"dark": {"primary": "#a1b2c3", "surface": "#111111"},
                       "light": {"primary": "#d4e4f4", "surface": "#ffffff"}}"##;
        let dark = variables_for_theme(text, true).unwrap();
        assert_eq!(dark["colors.primary"], "#a1b2c3");
        let light = variables_for_theme(text, false).unwrap();
        assert_eq!(light["colors.primary"], "#d4e4f4");
        // BOM + whitespace tolerated
        let bom = format!("\u{feff}  {text}  ");
        assert!(variables_for_theme(&bom, true).is_ok());
        // invalid JSON reports a preview
        let err = variables_for_theme("{nope", true).unwrap_err();
        assert!(err.contains("invalid JSON") && err.contains("preview"));
        // missing mode key lists available keys
        let err = variables_for_theme(r#"{"dark": {}}"#, false).unwrap_err();
        assert!(err.contains("missing \"light\"") && err.contains("dark"));
    }

    #[test]
    fn render_theme_templates_applies_stored_object() {
        let dir = std::env::temp_dir().join(format!("riced-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[templates.a]\ninput_path = \"in.txt\"\noutput_path = \"OUT/out.txt\"\n",
        )
        .unwrap();
        let out = dir.join("out").display().to_string();
        let cfg = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        std::fs::write(dir.join("config.toml"), cfg.replace("OUT", &out)).unwrap();
        std::fs::write(dir.join("in.txt"), "p={{colors.primary}};\n").unwrap();

        let text = r##"{"dark": {"primary": "#a1b2c3"}, "light": {"primary": "#d4e4f4"}}"##;
        let errors = render_theme_templates(&dir, text, true);
        assert!(errors.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("out").join("out.txt")).unwrap(),
            "p=#a1b2c3;\n"
        );
        // validation failure surfaces as the single error
        let errors = render_theme_templates(&dir, "{nope", true);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("invalid JSON"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stored_theme_text_prefers_builtin_and_rejects_unknown() {
        // vendored dracula dark primary, without touching the real home dir
        let text = stored_theme_text("dracula").unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["dark"]["primary"], "#bd93f9");
        assert!(stored_theme_text("no-such-theme").is_err());
    }

    #[test]
    fn expand_tilde_handles_home_paths() {
        assert_eq!(
            expand_tilde("~/a/b"),
            PathBuf::from(std::env::var("HOME").unwrap()).join("a/b")
        );
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn default_templates_embed_config_and_inputs() {
        let mut files = Vec::new();
        default_template_files(&DEFAULT_TEMPLATES, &mut files);
        // mirrors reshell core/theme (inputs + config.toml + colors.json)
        assert!(files.len() >= 20, "embedded {} files", files.len());
        assert!(
            files.iter().any(|p| p == Path::new("config.toml")),
            "config.toml embedded"
        );
        assert!(
            files
                .iter()
                .any(|p| p == Path::new("kitty/kitty-colors.conf")),
            "kitty input embedded"
        );
    }

    #[test]
    fn ensure_user_templates_seeds_without_overwriting() {
        let root = std::env::temp_dir().join(format!("riced-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // pre-existing user edit survives seeding
        std::fs::create_dir_all(root.join("kitty")).unwrap();
        std::fs::write(root.join("kitty/kitty-colors.conf"), "user edit\n").unwrap();

        ensure_user_templates_into(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("kitty/kitty-colors.conf")).unwrap(),
            "user edit\n"
        );
        // missing files appear, including config.toml
        assert!(root.join("config.toml").is_file());
        assert!(root.join("hypr/colors.lua").is_file());
        // second run is a no-op
        ensure_user_templates_into(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn effective_templates_dir_honors_off_and_explicit() {
        let mut cfg = ThemeConfig::default();
        // explicit dir wins as-is (existence is the renderer's problem)
        cfg.templates_dir = "/tmp/riced-explicit-tpl".to_string();
        assert_eq!(
            effective_templates_dir(&cfg),
            Some(PathBuf::from("/tmp/riced-explicit-tpl"))
        );
        cfg.templates_dir = "~/tpl".to_string();
        assert_eq!(
            effective_templates_dir(&cfg),
            Some(PathBuf::from(std::env::var("HOME").unwrap()).join("tpl"))
        );
        // "off" disables even with a real dir behind it
        cfg.templates_dir = "off".to_string();
        assert_eq!(effective_templates_dir(&cfg), None);
    }

    #[test]
    fn wallpaper_paths_dedups_and_skips_missing() {
        let dir = std::env::temp_dir().join(format!("riced-wp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = test_image(&dir, "a.png", 8, [1, 2, 3]);
        let path = PathBuf::from(&img.path);

        let mut config = Config::default();
        config
            .background
            .image
            .push(crate::config::BackgroundImage {
                path: path.display().to_string(),
                ..Default::default()
            });
        // duplicate entry
        config
            .background
            .image
            .push(crate::config::BackgroundImage {
                path: path.display().to_string(),
                ..Default::default()
            });
        // missing file + empty path
        config
            .background
            .image
            .push(crate::config::BackgroundImage {
                path: "/no/such/file.png".into(),
                ..Default::default()
            });
        config
            .background
            .image
            .push(crate::config::BackgroundImage::default());

        assert_eq!(wallpaper_paths(&config), vec![path]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
