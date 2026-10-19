//! Commands returned by any Lua host share one delivery boundary.
use super::{Plant, Plots};

pub(crate) fn command_from_action(value: &mlua::Value, plots: &mut Plots) -> iced::Task<Plant> {
    if let Err(error) = crate::lua::tray::dispatch(value, &plots.services.tray) {
        tracing::warn!(%error, "Lua tray command failed");
    }
    super::screens::notification::command_from_action(value, plots)
}
