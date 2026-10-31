//! Scheme construction, reshell JSON output, and stored-theme reads.
//!
//! The pure scheme and color-map logic lives in `riced-extension-theme-gen`
//! and is re-exported here; this module keeps wallpaper sampling and the
//! theme-file writes.
use super::templates::process_templates;
use material_colors::image::{Image, ImageReader};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) use riced_extension_theme_gen::{Generated, generate_from_seed, variables_for_theme};

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
        // SAFETY: glibc's malloc_trim takes a size_t padding value, requires no
        // caller-owned pointers, and is thread-safe. This is best effort only.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// Build the dynamic theme from rasterized wallpaper views (see
/// [`render_views`]): downscale the stitched strip to 128x128, extract the
/// seed color, and delegate the M3 scheme build to the theme-gen extension.
pub fn generate_from_views(
    views: &[image::RgbImage],
    variant: &str,
    darkmode: bool,
) -> Result<Generated, String> {
    let sample = super::views::sample_views(views)
        .ok_or_else(|| "no wallpaper views to combine".to_string())?;
    generate_from_sample(sample, variant, darkmode)
}

pub(crate) fn generate_from_pixels(
    views: &[super::views::PixelView],
    variant: &str,
    darkmode: bool,
) -> Result<Generated, String> {
    let sample = super::views::sample_pixels(views)
        .ok_or_else(|| "no wallpaper views to combine".to_string())?;
    generate_from_sample(sample, variant, darkmode)
}

fn generate_from_sample(
    sample: image::RgbImage,
    variant: &str,
    darkmode: bool,
) -> Result<Generated, String> {
    let rgba = image::RgbaImage::from_fn(sample.width(), sample.height(), |x, y| {
        let pixel = sample.get_pixel(x, y);
        image::Rgba([pixel[0], pixel[1], pixel[2], 255])
    });
    let data = Image::new(rgba);
    let seed = ImageReader::extract_color(&data);
    // Preserve the historical image label without encoding and re-decoding a
    // full wallpaper strip; that temporary file was already removed before use.
    let image = std::env::temp_dir()
        .join("riced_combined_wallpaper.png")
        .display()
        .to_string();
    Ok(generate_from_seed(seed, variant, darkmode, &image))
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

/// Expand a leading `~` to `$HOME` (template dirs are user paths).
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
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
    use material_colors::{color::Argb, theme::ThemeBuilder};
    use riced_extension_theme_gen::{build_color_map, build_color_map_from_json};

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
        let mut rss = Vec::new();
        for _ in 0..14 {
            let img = img.clone();
            let generated = super::super::worker::run(move || {
                let views = render_views(&[(0.0, 0.0, 512.0, 512.0)], &[img]);
                generate_from_views(&views, "content", true)
            })
            .unwrap()
            .unwrap();
            drop(std::hint::black_box(generated));
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
