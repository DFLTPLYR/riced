//! `riced.root(app, options)`: the shell entry's startup contract.
//!
//! Options are read once before the Wayland connection; the entry file is
//! loaded again for each output, so only the daemon bootstrap consumes them.
use mlua::{Lua, LuaSerdeExt, Value};

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct Settings {
    /// `RUST_LOG`-style filter; `None` keeps the environment/default.
    pub tracing: Option<String>,
    pub connection: Connection,
    #[serde(default)]
    pub settings: Daemon,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct Connection {
    /// Compositor application id; empty means unset.
    pub id: String,
    pub antialiasing: bool,
    pub default_text_size: f32,
    pub keep_compositor_alive: bool,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            id: String::new(),
            antialiasing: false,
            default_text_size: 16.0,
            keep_compositor_alive: true,
        }
    }
}

/// Settings for the daemon's own host surface. Bars and windows are Lua layers.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct Daemon {
    pub start_mode: StartMode,
    pub keyboard_interactivity: Keyboard,
}

#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StartMode {
    #[default]
    Background,
    AllScreens,
}

#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Keyboard {
    #[default]
    None,
    OnDemand,
    Exclusive,
}

impl Daemon {
    pub(crate) fn layer(&self) -> iced_exwlshell::settings::LayerShellSettings {
        use iced_exwlshell::reexport::KeyboardInteractivity;
        use iced_exwlshell::settings::StartMode as IcedStartMode;
        iced_exwlshell::settings::LayerShellSettings {
            start_mode: match self.start_mode {
                StartMode::Background => IcedStartMode::Background,
                StartMode::AllScreens => IcedStartMode::AllScreens,
            },
            keyboard_interactivity: match self.keyboard_interactivity {
                Keyboard::None => KeyboardInteractivity::None,
                Keyboard::OnDemand => KeyboardInteractivity::OnDemand,
                Keyboard::Exclusive => KeyboardInteractivity::Exclusive,
            },
            ..Default::default()
        }
    }
}

pub(crate) fn parse(lua: &Lua, value: Value) -> Result<Settings, String> {
    if value.is_nil() {
        return Ok(Settings::default());
    }
    lua.from_value(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_options_parse_and_unknown_fields_are_rejected() {
        let lua = Lua::new();
        let value: Value = lua
            .load(
                r#"return {
                    tracing = "riced=debug",
                    connection = { keep_compositor_alive = false },
                    settings = { start_mode = "all_screens", keyboard_interactivity = "exclusive" },
                }"#,
            )
            .eval()
            .unwrap();
        let settings = parse(&lua, value).unwrap();
        assert_eq!(settings.tracing.as_deref(), Some("riced=debug"));
        assert!(!settings.connection.keep_compositor_alive);
        assert!(matches!(
            settings.settings.start_mode,
            StartMode::AllScreens
        ));
        assert!(matches!(
            settings.settings.keyboard_interactivity,
            Keyboard::Exclusive
        ));

        let bad: Value = lua
            .load("return { start_mode = 'sideways' }")
            .eval()
            .unwrap();
        assert!(parse(&lua, bad).is_err());
    }

    #[test]
    fn riced_root_returns_the_app_and_captures_options() {
        let mut runtime = crate::lua::LuaRuntime::new().unwrap();
        runtime
            .load(
                r#"return riced.root(
                    { view = function() return iced.text("shell") end },
                    { tracing = "riced=debug", settings = { start_mode = "all_screens" } }
                )"#,
            )
            .unwrap();
        let settings = runtime.root_settings().unwrap();
        assert_eq!(settings.tracing.as_deref(), Some("riced=debug"));
        assert!(matches!(
            settings.settings.start_mode,
            StartMode::AllScreens
        ));
    }
}
