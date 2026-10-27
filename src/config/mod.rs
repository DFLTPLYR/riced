//! Configuration schema, discovery, migration, installation policy, and I/O.
pub mod discovery;
mod format;
mod format_migration;
pub(crate) mod initialize;
mod io;
pub(crate) mod lua;
pub(crate) use lua::{parse_value as parse_lua_value, to_literal as to_lua_literal};

/// Test-only shims so config-adjacent suites parse Lua without depending on
/// module privacy. The real path is `io::load_from`.
#[cfg(test)]
pub(crate) fn test_parse_config(source: &str) -> Result<Config, String> {
    lua::parse(source, "test")
}
#[cfg(test)]
pub(crate) fn test_parse_defaults(
    source: &str,
) -> Result<crate::ui::settings::ComponentDefaults, String> {
    lua::parse(source, "test")
}
#[cfg(test)]
pub(crate) fn test_format_config(config: &Config) -> Result<String, String> {
    format::to_string(config)
}
pub mod migrate;
mod paths;
pub(crate) use paths::{project_root, set_project_root};
mod schema;
mod seed;
pub mod util;
pub(crate) use io::take_parse_error;
pub use schema::*;
