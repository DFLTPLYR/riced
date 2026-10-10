//! Freedesktop icon-theme lookup with a bounded memo cache.
use super::images::image_file;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Icon theme search roots: `$XDG_DATA_HOME/icons`, each
/// `$XDG_DATA_DIRS/icons`, legacy `~/.icons`, legacy
/// `/usr/share/pixmaps` (flat files, no theme subdirs).
fn theme_base_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut bases = Vec::new();
    if let Some(data_home) = dirs::data_dir() {
        bases.push(data_home.join("icons"));
    }
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    bases.extend(
        data_dirs
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(s).join("icons")),
    );
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".icons"));
    }
    bases.push(PathBuf::from("/usr/share/pixmaps"));
    bases
}
/// Candidate size dirs per theme, closest to notification thumbnail
/// size first; contexts limited to what notifications actually use.
const THEME_SIZES: [&str; 9] = [
    "48x48", "32x32", "64x64", "24x24", "22x22", "16x16", "128x128", "256x256", "512x512",
];
const THEME_CONTEXTS: [&str; 2] = ["apps", "mimetypes"];
/// PNG first (exact pixels, cheapest), then SVG (rasterized below),
/// then legacy XPM — `image_file`/`rasterize_svg` decide per format,
/// so anything undecodable just misses.
const THEME_EXTS: [&str; 3] = ["png", "svg", "xpm"];
/// Resolve a freedesktop icon theme *name* (e.g. `firefox` — never a
/// path) to a file: flat legacy files first, then every theme with
/// `hicolor` last as the mandated fallback. Pure over `bases` so tests
/// can point it at a fake tree.
pub(crate) fn theme_icon_path_in(
    name: &str,
    bases: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.contains("..") || name.bytes().any(|b| b == b'/' || b == b'\\') {
        return None;
    }
    for base in bases {
        for ext in THEME_EXTS {
            let direct = base.join(format!("{name}.{ext}"));
            if direct.is_file() {
                return Some(direct);
            }
        }
        let Ok(themes) = std::fs::read_dir(base) else {
            continue;
        };
        let mut themes: Vec<_> = themes
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        themes.sort();
        // hicolor is the mandated fallback: check it last (stable sort
        // keeps the rest alphabetical).
        themes.sort_by_key(|path| path.file_name().is_some_and(|name| name == "hicolor"));
        for theme in &themes {
            for size in THEME_SIZES {
                for context in THEME_CONTEXTS {
                    for ext in THEME_EXTS {
                        let candidate =
                            theme.join(size).join(context).join(format!("{name}.{ext}"));
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }
    None
}
/// Memoized theme lookups: icon names are low-cardinality, but each
/// miss walks the theme tree. Cleared past a bound so a pathological
/// sender can't grow it without limit.
static THEME_CACHE: LazyLock<Mutex<HashMap<String, Option<std::path::PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const THEME_CACHE_CAP: usize = 1024;
/// Decode a theme icon name via lookup + the shared file decoder
/// (which also enforces the pixel cap).
pub(crate) fn theme_icon_file(name: &str) -> Option<iced::widget::image::Handle> {
    let hit = THEME_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(name).cloned());
    let path = match hit {
        Some(found) => found?,
        None => {
            let found = theme_icon_path_in(name, &theme_base_dirs());
            if let Ok(mut cache) = THEME_CACHE.lock() {
                if cache.len() >= THEME_CACHE_CAP {
                    cache.clear();
                }
                cache.insert(name.to_string(), found.clone());
            }
            found?
        }
    };
    image_file(&path.to_string_lossy())
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Fake icon tree for lookup tests: `$base/<theme>/48x48/apps/<name>.png`
    /// plus a legacy `$base/pixmaps.png`, all real decodable PNGs, plus
    /// an SVG-only icon to prove the rasterize chain.
    /// One tree per test — the suite runs tests in parallel.
    fn theme_fixture(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("riced-icons-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for theme in ["custom", "hicolor"] {
            let dir = base.join(theme).join("48x48").join("apps");
            std::fs::create_dir_all(&dir).unwrap();
            image::RgbImage::new(8, 8)
                .save(dir.join("testicon.png"))
                .unwrap();
        }
        // An icon present as both PNG and SVG resolves to the PNG
        // (exact pixels beat re-rasterized vectors).
        image::RgbImage::new(8, 8)
            .save(
                base.join("custom")
                    .join("48x48")
                    .join("apps")
                    .join("vectors.png"),
            )
            .unwrap();
        std::fs::write(
            base
                .join("custom")
                .join("48x48")
                .join("apps")
                .join("vectors.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect width="16" height="16" fill="red"/></svg>"#,
        )
        .unwrap();
        image::RgbImage::new(8, 8)
            .save(base.join("legacy.png"))
            .unwrap();
        base
    }
    #[test]
    fn theme_lookup_finds_themed_and_legacy_icons() {
        let base = theme_fixture("hits");
        let bases = [base.clone()];
        // Themed hit resolves to a file that decodes.
        let found = theme_icon_path_in("testicon", &bases).expect("themed hit");
        assert!(image_file(&found.to_string_lossy()).is_some());
        // Legacy flat layout resolves too.
        assert!(theme_icon_path_in("legacy", &bases).is_some());
        // Theme-specific hits win over hicolor.
        assert!(found.starts_with(base.join("custom")));
        // PNG wins over SVG for the same icon name.
        let vectors = theme_icon_path_in("vectors", &bases).expect("vectors hit");
        assert_eq!(
            vectors.extension().and_then(|ext| ext.to_str()),
            Some("png")
        );
        // SVG-only icons resolve and rasterize end to end.
        std::fs::remove_file(
            base.join("custom")
                .join("48x48")
                .join("apps")
                .join("vectors.png"),
        )
        .unwrap();
        let svg_only = theme_icon_path_in("vectors", &bases).expect("svg hit");
        assert_eq!(
            svg_only.extension().and_then(|ext| ext.to_str()),
            Some("svg")
        );
        assert!(image_file(&svg_only.to_string_lossy()).is_some());
        let _ = std::fs::remove_dir_all(&base);
    }
    #[test]
    fn theme_lookup_rejects_names_and_misses() {
        let base = theme_fixture("misses");
        let bases = [base.clone()];
        // Path traversal and absolute-looking names never resolve.
        assert!(theme_icon_path_in("../x", &bases).is_none());
        assert!(theme_icon_path_in("a/b", &bases).is_none());
        assert!(theme_icon_path_in("", &bases).is_none());
        // Unknown names and missing trees miss cleanly.
        assert!(theme_icon_path_in("nope", &bases).is_none());
        assert!(
            theme_icon_path_in(
                "testicon",
                &[std::path::PathBuf::from("/definitely/not/here")]
            )
            .is_none()
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
