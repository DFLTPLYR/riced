//! Reshell-compatible theme-file schema; serialization is independent of paint.
use serde::Deserialize;
#[derive(Debug, Clone, Deserialize)]
pub(super) struct VariantRaw {
    pub primary: String,
    #[serde(alias = "onprimary")]
    pub on_primary: String,
    pub secondary: String,
    #[serde(alias = "onsecondary")]
    pub on_secondary: String,
    pub tertiary: String,
    #[serde(alias = "ontertiary")]
    pub on_tertiary: String,
    pub error: String,
    #[serde(alias = "onerror")]
    pub on_error: String,
    pub surface: String,
    #[serde(alias = "onsurface")]
    pub on_surface: String,
    #[serde(alias = "surfacevariant")]
    pub surface_variant: String,
    #[serde(alias = "onsurfacevariant")]
    pub on_surface_variant: String,
    pub outline: String,
    pub shadow: String,
    pub hover: String,
    #[serde(alias = "onhover")]
    pub on_hover: String,
}
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ThemeFile {
    pub dark: VariantRaw,
    pub light: VariantRaw,
}
