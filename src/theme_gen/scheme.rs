//! Scheme construction, reshell JSON output, and stored-theme reads.
use super::templates::process_templates;
use super::variant::parse_variant;
use super::views::combine_views;
use material_colors::{
    color::Argb,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

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
#[cfg(test)]
mod tests {
    use super::super::views::{render_views, test_image};
    use super::*;

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(target_os = "linux")]
    fn rss_kb() -> u64 {
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|pages| pages * 4)
            .unwrap_or(0)
    }

    #[test]
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    fn trim_memory_releases_churned_arenas() {
        // Churn ~64MB through small (arena, not mmap) blocks, drop it
        // all, then trim: RSS must fall substantially. Median-of-three
        // samples each side; sibling tests share this process.
        fn median(mut v: Vec<u64>) -> u64 {
            v.sort_unstable();
            v[v.len() / 2]
        }
        let sample = || (0..3).map(|_| rss_kb()).collect::<Vec<_>>();
        let mut held = Vec::new();
        for _ in 0..2048 {
            let mut block = vec![0u8; 32 * 1024];
            // Touch every page: zero-filled allocations fault lazily.
            for byte in block.iter_mut().step_by(4096) {
                *byte = 1;
            }
            std::hint::black_box(block.as_mut_ptr());
            held.push(block);
        }
        let peak = median(sample());
        drop(held);
        trim_memory();
        let after = median(sample());
        let released = peak.saturating_sub(after);
        assert!(
            released > 32 * 1024,
            "trim released only {released} KiB (peak {peak}, after {after})"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn repeated_regen_rss_stays_flat() {
        const CHILD: &str = "RICED_REGEN_RSS_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "theme_gen::scheme::tests::repeated_regen_rss_stays_flat",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated RSS probe failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
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
        // Medians, not endpoints. This subprocess runs only the RSS probe,
        // so renderer and Lua allocation tests cannot pollute its samples.
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
}
