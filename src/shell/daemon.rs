//! Native adapters for the Lua daemon callbacks.
use super::{apps, Plots};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

pub(crate) fn redraw(
    callback: &std::sync::Mutex<Option<crate::lua::DaemonRedraw>>,
    message: &super::Plant,
) -> iced_exwlshell::redraw::Scope {
    use iced_exwlshell::redraw::Scope;
    let fallback = super::redraw_scope(message);
    let guard = callback.lock().unwrap_or_else(|error| error.into_inner());
    let Some(callback) = guard.as_ref() else {
        return fallback;
    };
    let (scope, surface) = match fallback {
        Scope::All => ("all", None),
        Scope::None => ("none", None),
        Scope::Window(id) => ("window", Some(id.to_string())),
    };
    let kind = match message {
        super::Plant::App(_) => "app",
        super::Plant::Wayland(_) => "wayland",
        super::Plant::Config(_) => "config",
        super::Plant::Graft(_, _) => "input",
        super::Plant::Notify(_) => "notification",
        super::Plant::Ipc { .. } | super::Plant::IpcPoll => "ipc",
        _ => "native",
    };
    match callback.call(&json!({"kind":kind,"scope":scope,"surface":surface})) {
        Ok(scope) if scope == "all" => Scope::All,
        Ok(scope) if scope == "none" => Scope::None,
        Ok(scope) if scope == "window" && surface.is_some() => fallback,
        Ok(_) => {
            tracing::error!(
                "daemon.redraw_scope must return all, none, or window for a surface event"
            );
            fallback
        }
        Err(error) => {
            tracing::error!(%error, "daemon.redraw_scope failed");
            fallback
        }
    }
}

pub(super) fn callback<T: DeserializeOwned>(
    plots: &Plots,
    name: &str,
    argument: &Value,
) -> Option<T> {
    match plots.apps.daemon_runtime()?.daemon_callback(name, argument) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, callback = name, "Lua daemon callback failed");
            None
        }
    }
}

pub(crate) fn title(plots: &Plots, surface: iced::window::Id) -> Option<String> {
    let title = apps::title(plots, surface);
    callback::<Option<String>>(
        plots,
        "title",
        &json!({"id":surface.to_string(), "title":title}),
    )
    .unwrap_or(title)
}

pub(crate) fn theme(plots: &Plots, surface: iced::window::Id) -> iced::Theme {
    let config = callback(
        plots,
        "theme",
        &json!({"id":surface.to_string(), "theme":plots.config.theme}),
    )
    .unwrap_or_else(|| plots.config.theme.clone());
    crate::theme::theme_for(&config)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Style {
    background_color: String,
    text_color: String,
}

pub(crate) fn style(plots: &Plots, theme: &iced::Theme) -> iced::theme::Style {
    let fallback = crate::theme::app_style(plots, theme);
    let color = fallback.text_color;
    let text = format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        (color.a * 255.0) as u8
    );
    let Some(style) = callback::<Style>(plots, "style", &json!({"text_color":text})) else {
        return fallback;
    };
    match (
        crate::theme::parse_hex(&style.background_color),
        crate::theme::parse_hex(&style.text_color),
    ) {
        (Some(background_color), Some(text_color)) => iced::theme::Style {
            background_color,
            text_color,
        },
        _ => {
            tracing::error!("daemon.style colors must be #rrggbb or #rrggbbaa");
            fallback
        }
    }
}
