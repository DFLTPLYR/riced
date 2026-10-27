//! Commands returned by any Lua host share one delivery boundary.
use super::{Plant, Plots};
use crate::lua::shell::ShellCommand;

pub(crate) fn command_from_action(value: &mlua::Value, plots: &mut Plots) -> iced::Task<Plant> {
    if let Err(error) = crate::lua::tray::dispatch(value, &plots.services.tray) {
        tracing::warn!(%error, "Lua tray command failed");
    }
    match crate::lua::shell::parse(value) {
        Ok(Some(ShellCommand::OpenSettings)) => {
            return iced::Task::done(Plant::Sprout);
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "Lua shell command failed"),
    }
    super::screens::notification::command_from_action(value, plots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::shell::install;
    use iced::futures::StreamExt;
    use mlua::Lua;

    fn plots() -> Plots {
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        Plots::new(receiver)
    }

    /// Plants emitted by one Lua action result. The VM stays alive for the
    /// call because the action value borrows it.
    fn plants(source: &str) -> Vec<Plant> {
        let lua = Lua::new();
        install(&lua).unwrap();
        let value: mlua::Value = lua.load(source).eval().unwrap();
        let mut plots = plots();
        iced_runtime::task::into_stream(command_from_action(&value, &mut plots))
            .map(|stream| {
                iced::futures::executor::block_on(stream.collect::<Vec<_>>())
                    .into_iter()
                    .map(|action| match action {
                        iced_runtime::Action::Output(plant) => plant,
                        other => panic!("unexpected action {other:?}"),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn shell_commands_map_to_host_messages_and_others_do_not() {
        assert!(matches!(
            plants("return shell.open_settings()").as_slice(),
            [Plant::Sprout]
        ));
        assert!(plants("return {other=true}").is_empty());
        assert!(plants("return nil").is_empty());
    }
}
