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
//! Switching themes re-renders the templates dir like sys's `change_theme`
//! (see `render_theme_templates`); generate with
//! `riced generate-theme`, apply a stored theme with
//! `riced apply-templates`.

pub mod scheme;
pub mod templates;
pub mod variant;
pub mod views;

pub(crate) use scheme::trim_memory;
pub use scheme::{
    generate_from_views, render_theme_templates, stored_theme_text, wants_regen,
    write_dynamic_theme,
};
pub use templates::{effective_templates_dir, ensure_user_templates, process_templates};
pub use variant::VARIANT_NAMES;
pub use views::{render_views, wallpaper_paths};
