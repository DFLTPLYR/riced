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
//! (sys's `change_theme` re-application path stays with reshell: picking a
//! theme in riced only repaints riced itself via hot-reload.)

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use material_colors::{
    color::Argb,
    dynamic_color::Variant,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use regex::Regex;

use crate::components::display_map::MapLayer;
use crate::config::{BackgroundImage, Config};

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
        use image::GenericImageView;
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
        use image::GenericImageView;
        assert_eq!(views[0].get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(views[1].get_pixel(0, 0).0, [200, 100, 50]);
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
