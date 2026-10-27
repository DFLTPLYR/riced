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
        // Bars are config, not app state: Settings edits the same `config.bar`
        // entries, so there is one list rather than two.
        Ok(Some(ShellCommand::AddBar {
            anchor,
            output,
            thickness,
        })) => return add_bar(plots, &anchor, &output, thickness),
        Ok(Some(ShellCommand::RemoveBar { anchor, output })) => {
            return remove_bar(plots, &anchor, &output);
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "Lua shell command failed"),
    }
    super::screens::notification::command_from_action(value, plots)
}

/// Append a bar placement, reusing the legacy defaults so Settings shows it.
fn add_bar(plots: &mut Plots, anchor: &str, output: &str, thickness: u32) -> iced::Task<Plant> {
    let taken = plots.config.bar.iter().any(|bar| {
        bar.anchor.eq_ignore_ascii_case(anchor)
            && bar.output.trim().eq_ignore_ascii_case(output.trim())
    });
    if taken {
        tracing::warn!(%anchor, %output, "shell.add_bar: that edge already has a bar");
        return iced::Task::none();
    }
    plots.config.bar.push(crate::config::TopConfig {
        anchor: anchor.to_owned(),
        output: output.to_owned(),
        thickness: thickness as f32,
        ..Default::default()
    });
    persist(plots)
}

fn remove_bar(plots: &mut Plots, anchor: &str, output: &str) -> iced::Task<Plant> {
    let index = plots.config.bar.iter().position(|bar| {
        bar.anchor.eq_ignore_ascii_case(anchor)
            && bar.output.trim().eq_ignore_ascii_case(output.trim())
    });
    let Some(index) = index else {
        tracing::warn!(%anchor, %output, "shell.remove_bar: no matching bar");
        return iced::Task::none();
    };
    super::screens::Top::remove_configured(plots, index)
}

/// Stage the write like every other config edit and re-render the app that
/// draws `config.bar`.
fn persist(plots: &mut Plots) -> iced::Task<Plant> {
    plots.config_jobs.dirty = true;
    let save = plots.arm_config_save();
    let outputs: Vec<_> = plots.windows.output_infos.keys().copied().collect();
    let mut commands = vec![save, super::apps::reload(plots)];
    commands.extend(
        outputs
            .into_iter()
            .map(|output| super::screens::Top::spawn_configured(plots, output)),
    );
    iced::Task::batch(commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::shell::install;
    use iced::futures::StreamExt;
    use mlua::Lua;

    fn plots() -> Plots {
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots::new(receiver);
        plots.config.bar.clear();
        plots
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

    #[test]
    fn add_bar_appends_one_config_entry_per_edge_and_rejects_duplicates() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut plots = plots();
                let _ = add_bar(&mut plots, "top", "DP-1", 34);
                assert_eq!(plots.config.bar.len(), 1);
                let bar = &plots.config.bar[0];
                assert_eq!((bar.anchor.as_str(), bar.output.as_str()), ("top", "DP-1"));
                assert_eq!(bar.thickness, 34.0);
                assert!(plots.config_jobs.dirty);
                // The same edge twice is a no-op, not a second bar.
                let _ = add_bar(&mut plots, "top", "DP-1", 34);
                assert_eq!(plots.config.bar.len(), 1);
                let _ = add_bar(&mut plots, "bottom", "DP-1", 28);
                assert_eq!(plots.config.bar.len(), 2);
            });
    }

    #[test]
    fn remove_bar_drops_only_the_matching_entry() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut plots = plots();
                let _ = add_bar(&mut plots, "top", "DP-1", 34);
                let _ = add_bar(&mut plots, "bottom", "DP-1", 34);
                let _ = remove_bar(&mut plots, "top", "DP-1");
                assert_eq!(plots.config.bar.len(), 1);
                assert_eq!(plots.config.bar[0].anchor, "bottom");
                let _ = remove_bar(&mut plots, "top", "DP-1");
                assert_eq!(plots.config.bar.len(), 1);
            });
    }

    #[test]
    fn remove_closes_all_recipe_surfaces_and_preserves_other_bar_identity() {
        use super::super::screens::Top;
        use super::super::windows::PlotInfo;
        use iced::window;
        use iced_exwlshell::reexport::Anchor;
        use iced_wayland_subscriber::OutputId;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut plots = plots();
                plots.config.bar = vec![
                    crate::config::TopConfig::default(),
                    crate::config::TopConfig {
                        anchor: "bottom".into(),
                        ..Default::default()
                    },
                ];
                let first = window::Id::unique();
                let second = window::Id::unique();
                let survivor = window::Id::unique();
                for (id, output, index, anchor) in [
                    (first, OutputId(1), 0, Anchor::Top),
                    (second, OutputId(2), 0, Anchor::Top),
                    (survivor, OutputId(1), 1, Anchor::Bottom),
                ] {
                    plots.windows.tops.insert(
                        id,
                        Top::with_config(index, anchor, (&plots.config.bar[index]).into()),
                    );
                    plots.windows.ids.insert(id, PlotInfo::Top(output));
                }
                let local = plots.windows.tops[&survivor].local.clone();
                let _ = remove_bar(&mut plots, "top", "");
                assert!(!plots.windows.ids.contains_key(&first));
                assert!(!plots.windows.ids.contains_key(&second));
                assert_eq!(plots.windows.tops.len(), 1);
                assert_eq!(plots.windows.tops[&survivor].bar_index, 0);
                assert_eq!(
                    plots.windows.tops[&survivor].local.thickness_px,
                    local.thickness_px
                );
                assert_eq!(plots.config.bar[0].anchor, "bottom");
                assert!(plots.config_jobs.dirty);
            });
    }
}
