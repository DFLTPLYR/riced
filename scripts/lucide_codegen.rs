//! Pure, deterministic generation shared by build.rs and regression tests.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

const ALIASES: &[(&str, &str)] = &[
    ("robot", "bot"),
    ("mem", "memory-stick"),
    ("ram", "memory-stick"),
    ("temp", "thermometer"),
    ("disk", "hard-drive"),
    ("chart", "chart-line"),
    ("vol", "volume-2"),
    ("mute", "volume-x"),
    ("up", "arrow-up"),
    ("down", "arrow-down"),
];

fn normalize(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

pub fn generate(icons: &BTreeMap<String, String>) -> Result<String, String> {
    let mut code = String::from("// @generated from assets/lucide/icons.json; do not edit.\n");
    // Native controls share the same pinned SVGs as the dynamic name lookup.
    for (constant, name) in [
        ("MINUS", "minus"),
        ("PLUS", "plus"),
        ("LOADER_CIRCLE", "loader-circle"),
    ] {
        let svg = icons
            .get(name)
            .ok_or_else(|| format!("missing required icon {name}"))?;
        writeln!(
            code,
            "pub(crate) const {constant}: &[u8] = {svg:?}.as_bytes();"
        )
        .map_err(|e| e.to_string())?;
    }
    code.push_str("pub(crate) fn generated_icon_bytes(name: &str) -> Option<&'static [u8]> {\nSome(match name {\n");
    let mut seen = BTreeSet::new();
    for (name, svg) in icons {
        if !svg.contains("<svg") {
            return Err(format!("invalid SVG for {name}"));
        }
        let key = normalize(name);
        if key.is_empty() {
            return Err("empty normalized icon name".into());
        }
        if seen.insert(key.clone()) {
            writeln!(code, "{key:?} => {svg:?}.as_bytes(),").map_err(|e| e.to_string())?;
        }
    }
    // Real icon names take precedence over aliases, as before.
    for (alias, target) in ALIASES {
        let svg = icons
            .get(*target)
            .ok_or_else(|| format!("missing alias target {target}"))?;
        if seen.insert((*alias).into()) {
            writeln!(code, "{alias:?} => {svg:?}.as_bytes(),").map_err(|e| e.to_string())?;
        }
    }
    code.push_str("_ => return None,\n})\n}\n");
    Ok(code)
}
