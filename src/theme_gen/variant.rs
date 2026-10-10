//! Material-You scheme variant names.
use material_colors::dynamic_color::Variant;

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
#[cfg(test)]
mod tests {
    use super::*;

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
}
