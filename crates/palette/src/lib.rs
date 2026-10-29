//! Material palette generation example. No filesystem or theme side effects.
#![deny(missing_docs)]
use material_colors::{color::Argb, theme::ThemeBuilder};
use riced_extension_sdk::{
    Cancellation, Descriptor, Error, Extension, Manifest, Output, SDK_VERSION, Service,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Register this extension with a host registry.
///
/// Generated shells call this function from their `main`; the reserved list
/// comes from `riced::builtin_service_topics()`.
pub fn register(
    registry: &mut riced_extension_sdk::Registry,
    reserved: &[&str],
) -> Result<(), Error> {
    registry.register(&Palette, reserved)
}

/// Compiled-in palette extension.
pub struct Palette;
impl Extension for Palette {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "palette",
            sdk_version: SDK_VERSION,
        }
    }
    fn services(&self) -> Vec<Arc<dyn Service>> {
        vec![Arc::new(Palette)]
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    seed: String,
    #[serde(default)]
    dark: bool,
}
impl Service for Palette {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: "palette",
            commands: &["generate"],
        }
    }
    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(json!({"ready":false}))
    }
    fn command(&self, method: &str, args: Value, cancel: &Cancellation) -> Result<Output, Error> {
        if method != "generate" {
            return Err(Error::new("command", "unknown palette command"));
        }
        let options: Generate =
            serde_json::from_value(args).map_err(|e| Error::new("options", e.to_string()))?;
        let digits = options
            .seed
            .strip_prefix('#')
            .filter(|s| s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| Error::new("seed", "palette seed must be #rrggbb"))?;
        let rgb = u32::from_str_radix(digits, 16).map_err(|e| Error::new("seed", e.to_string()))?;
        if cancel.is_cancelled() {
            return Err(Error::new("cancelled", "palette generation cancelled"));
        }
        let theme = ThemeBuilder::with_source(Argb::from_u32(0xff000000 | rgb)).build();
        let scheme = if options.dark {
            &theme.schemes.dark
        } else {
            &theme.schemes.light
        };
        let hex = |c: Argb| format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue);
        let data = json!({"ready":true,"seed":options.seed.to_ascii_lowercase(),"dark":options.dark,"colors":{
            "primary":hex(scheme.primary),"on_primary":hex(scheme.on_primary),
            "secondary":hex(scheme.secondary),"on_secondary":hex(scheme.on_secondary),
            "surface":hex(scheme.surface),"on_surface":hex(scheme.on_surface),
            "background":hex(scheme.background),"on_background":hex(scheme.on_background),
            "error":hex(scheme.error),"on_error":hex(scheme.on_error)
        }});
        Ok(Output {
            snapshot: Some(data.clone()),
            data,
        })
    }
}
