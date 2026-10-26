#[path = "../scripts/lucide_codegen.rs"]
mod codegen;

use std::collections::BTreeMap;

fn catalog() -> BTreeMap<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../assets/lucide/icons.json")).unwrap();
    assert_eq!(value["version"], "1.61.0");
    serde_json::from_value(value["icons"].clone()).unwrap()
}

#[test]
fn pinned_catalog_generates_self_contained_svg_bytes() {
    let icons = catalog();
    assert_eq!(icons.len(), 1903);
    let code = codegen::generate(&icons).unwrap();
    assert!(!code.contains("lucide_iced::"));
    assert!(!code.contains("/target/"));
    assert!(code.contains("\"avocado\" =>"));
    assert!(code.contains("\"table\" =>"));
    assert!(code.contains("pub(crate) const LOADER_CIRCLE"));
    assert_eq!(code, codegen::generate(&icons).unwrap());
}

#[test]
fn real_names_win_over_aliases_and_normalized_collisions_are_stable() {
    let mut icons = catalog();
    icons.insert("mem".into(), "<svg>real memory icon</svg>".into());
    icons.insert("a-b".into(), "<svg>first</svg>".into());
    icons.insert("ab".into(), "<svg>second</svg>".into());
    let code = codegen::generate(&icons).unwrap();
    assert!(code.contains("\"mem\" => \"<svg>real memory icon</svg>\".as_bytes()"));
    assert_eq!(code.matches("\"mem\" =>").count(), 1);
    assert!(code.contains("\"ab\" => \"<svg>first</svg>\".as_bytes()"));
    assert_eq!(code.matches("\"ab\" =>").count(), 1);
}

#[test]
fn missing_native_icons_and_alias_targets_are_build_errors() {
    for required in ["plus", "minus", "loader-circle", "memory-stick"] {
        let mut icons = catalog();
        icons.remove(required);
        assert!(codegen::generate(&icons).unwrap_err().contains(required));
    }
}

#[test]
fn malformed_catalog_entries_are_rejected() {
    let mut icons = catalog();
    icons.insert("invalid".into(), "not an SVG".into());
    assert!(codegen::generate(&icons).unwrap_err().contains("invalid"));
}

#[test]
fn every_pinned_icon_is_valid_svg() {
    for (name, svg) in catalog() {
        resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}
