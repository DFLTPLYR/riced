//! Material You theme generation: reshell `{light, dark}` scheme JSON and
//! `{{var}}` template variables from a seed color.
//!
//! This crate owns the pure part of the former in-tree `theme_gen` pipeline.
//! It never samples wallpaper pixels or writes files; the host keeps sampling
//! (which needs image decoding) and persistence, and calls this crate for the
//! scheme itself. The same code is exposed as the `theme_gen` service so Lua
//! and other extensions can generate a theme from a seed.
#![deny(missing_docs)]
use material_colors::{color::Argb, dynamic_color::Variant, scheme::Scheme, theme::ThemeBuilder};
use riced_extension_sdk::{
    Cancellation, Descriptor, Error, Extension, Manifest, Output, SDK_VERSION, Service,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

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

/// Parse a variant name (case-insensitive, `-`/`_` interchangeable);
/// unknown names fall back to `TonalSpot`, like sys.
pub fn parse_variant(input: &str) -> Variant {
    const NAMES: &[(&str, Variant)] = &[
        ("content", Variant::Content),
        ("tonalspot", Variant::TonalSpot),
        ("monochrome", Variant::Monochrome),
        ("neutral", Variant::Neutral),
        ("vibrant", Variant::Vibrant),
        ("expressive", Variant::Expressive),
        ("fidelity", Variant::Fidelity),
        ("rainbow", Variant::Rainbow),
        ("fruit_salad", Variant::FruitSalad),
        ("fruitsalad", Variant::FruitSalad),
    ];
    let normalized = input.trim().to_lowercase().replace('-', "_");
    NAMES
        .iter()
        .find(|(name, _)| *name == normalized)
        .map(|(_, value)| value.clone())
        .unwrap_or(Variant::TonalSpot)
}

// ---------------------------------------------------------------------------
// Scheme construction and reshell JSON
// ---------------------------------------------------------------------------

/// Reshell-format `{light, dark}` JSON plus template variables.
#[derive(Debug)]
pub struct Generated {
    /// Reshell-format `{light, dark}` JSON (`scheme_json`, same as sys).
    pub payload: Value,
    /// Template variables for the active scheme (`build_color_map`).
    pub variables: HashMap<String, String>,
}

/// Build both M3 schemes from a seed color. `image` is the label exposed to
/// templates as the `{{image}}` variable (cosmetic; never read).
pub fn generate_from_seed(seed: Argb, variant: &str, darkmode: bool, image: &str) -> Generated {
    let theme = ThemeBuilder::with_source(seed)
        .variant(parse_variant(variant))
        .build();
    let scheme = if darkmode {
        &theme.schemes.dark
    } else {
        &theme.schemes.light
    };
    Generated {
        payload: json!({
            "light": scheme_json(&theme.schemes.light),
            "dark": scheme_json(&theme.schemes.dark),
        }),
        variables: build_color_map(scheme, darkmode, image),
    }
}

fn hex(color: Argb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
}

fn terminal_json(s: &Scheme) -> Value {
    json!({
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
pub fn scheme_json(s: &Scheme) -> Value {
    json!({
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

/// Template variables from a stored reshell theme variant: reads both
/// `snake_case` and unseparated (`onprimary`, as in `ayu-blue.json`) keys,
/// plus the `terminal` block.
pub fn build_color_map_from_json(scheme: &Value) -> HashMap<String, String> {
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
    let value: Value = serde_json::from_str(trimmed).map_err(|e| {
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

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Register this extension with a host registry.
///
/// Generated shells call this function from their `main`; the reserved list
/// comes from `riced::builtin_service_topics()`.
pub fn register(
    registry: &mut riced_extension_sdk::Registry,
    reserved: &[&str],
) -> Result<(), Error> {
    registry.register(&ThemeGen, reserved)
}

/// Compiled-in theme-generation extension.
pub struct ThemeGen;
impl Extension for ThemeGen {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "theme_gen",
            sdk_version: SDK_VERSION,
        }
    }
    fn services(&self) -> Vec<Arc<dyn Service>> {
        vec![Arc::new(ThemeGen)]
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    seed: String,
    #[serde(default)]
    variant: Option<String>,
    #[serde(default)]
    darkmode: bool,
    #[serde(default)]
    image: Option<String>,
}

/// The `{{image}}` label the host uses for generated schemes.
fn default_image() -> String {
    std::env::temp_dir()
        .join("riced_combined_wallpaper.png")
        .display()
        .to_string()
}

fn parse_seed(seed: &str) -> Result<Argb, Error> {
    let digits = seed
        .strip_prefix('#')
        .filter(|s| s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| Error::new("seed", "theme seed must be #rrggbb"))?;
    let rgb = u32::from_str_radix(digits, 16).map_err(|e| Error::new("seed", e.to_string()))?;
    Ok(Argb::from_u32(0xff000000 | rgb))
}

impl Service for ThemeGen {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: "theme_gen",
            commands: &["generate"],
        }
    }
    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(json!({"ready": false}))
    }
    fn command(&self, method: &str, args: Value, cancel: &Cancellation) -> Result<Output, Error> {
        if method != "generate" {
            return Err(Error::new("command", "unknown theme_gen command"));
        }
        let options: Generate =
            serde_json::from_value(args).map_err(|e| Error::new("options", e.to_string()))?;
        let seed = parse_seed(&options.seed)?;
        if cancel.is_cancelled() {
            return Err(Error::new("cancelled", "theme generation cancelled"));
        }
        let variant = options.variant.unwrap_or_else(|| "tonalspot".to_string());
        let image = options.image.unwrap_or_else(default_image);
        let generated = generate_from_seed(seed, &variant, options.darkmode, &image);
        let snapshot = json!({
            "ready": true,
            "seed": options.seed.to_ascii_lowercase(),
            "variant": variant,
            "darkmode": options.darkmode,
        });
        Ok(Output {
            data: json!({"payload": generated.payload, "variables": generated.variables}),
            snapshot: Some(snapshot),
        })
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
        for name in VARIANT_NAMES {
            let _ = parse_variant(name);
        }
    }

    #[test]
    fn generate_command_returns_reshell_payload_and_snapshot() {
        let output = ThemeGen
            .command(
                "generate",
                json!({"seed": "#89b4fa", "variant": "content", "darkmode": true}),
                &Cancellation::default(),
            )
            .unwrap();
        for mode in ["light", "dark"] {
            let scheme = &output.data["payload"][mode];
            assert!(scheme["primary"].as_str().unwrap().starts_with('#'));
            assert_eq!(
                scheme["terminal"]["foreground"].as_str().unwrap(),
                scheme["on_surface"].as_str().unwrap()
            );
        }
        assert_eq!(output.data["variables"]["mode"], "dark");
        assert!(output.data["variables"]["colors.primary"].is_string());
        assert!(output.data["variables"]["colors.terminal.normal.red"].is_string());
        let snapshot = output.snapshot.unwrap();
        assert_eq!(snapshot["ready"], true);
        assert_eq!(snapshot["variant"], "content");
    }

    #[test]
    fn generate_command_rejects_bad_seeds_and_unknown_options() {
        let error = ThemeGen
            .command(
                "generate",
                json!({"seed": "not-a-color"}),
                &Cancellation::default(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, "seed");
        let error = ThemeGen
            .command(
                "generate",
                json!({"seed": "#89b4fa", "extra": 1}),
                &Cancellation::default(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, "options");
        assert_eq!(
            ThemeGen
                .command("bogus", json!({}), &Cancellation::default())
                .err()
                .unwrap()
                .code,
            "command"
        );
    }

    #[test]
    fn variables_for_theme_validates_like_change_theme() {
        let text = r##"{"dark": {"primary": "#a1b2c3", "surface": "#111111"},
                       "light": {"primary": "#d4e4f4", "surface": "#ffffff"}}"##;
        assert_eq!(
            variables_for_theme(text, true).unwrap()["colors.primary"],
            "#a1b2c3"
        );
        assert_eq!(
            variables_for_theme(text, false).unwrap()["colors.primary"],
            "#d4e4f4"
        );
        let err = variables_for_theme("{nope", true).unwrap_err();
        assert!(err.contains("invalid JSON") && err.contains("preview"));
        let err = variables_for_theme(r#"{"dark": {}}"#, false).unwrap_err();
        assert!(err.contains("missing \"light\"") && err.contains("dark"));
    }
}
