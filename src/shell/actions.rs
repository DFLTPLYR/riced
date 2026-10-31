//! Commands returned by any Lua host share one delivery boundary.
use super::{Plant, Plots};
use crate::theme::palette::PaletteOverrides;

pub(crate) fn command_from_action(value: &mlua::Value, plots: &mut Plots) -> iced::Task<Plant> {
    if matches!(value, mlua::Value::UserData(value) if value.is::<crate::theme_gen::GenerateTheme>())
    {
        plots.theme_jobs.requested_theme = Some(plots.config.theme.name.clone());
        plots.theme_jobs.dirty = true;
        return plots.fire_regen_theme();
    }
    if let mlua::Value::UserData(value) = value
        && value.is::<PaletteOverrides>()
    {
        match value.borrow::<PaletteOverrides>() {
            Ok(patch) if !plots.config.theme.palette.contains_patch(&patch) => {
                return iced::Task::done(Plant::Config(super::ConfigEvent::Patch(
                    crate::config::ConfigPatch::ThemePalette(patch.clone()),
                )));
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "Lua palette value could not be read"),
        }
        return iced::Task::none();
    }
    if let Err(error) = crate::lua::tray::dispatch(value, &plots.services.tray) {
        tracing::warn!(%error, "Lua tray command failed");
    }
    super::screens::notification::command_from_action(value, plots)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
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
    fn theme_generation_requires_an_explicit_request_not_a_special_theme_name() {
        let lua = Lua::new();
        let mut plots = plots();
        plots.config.theme.name = "user-defined-theme".into();
        let raw: mlua::Value = lua.load("return {generate_theme=true}").eval().unwrap();
        assert!(iced_runtime::task::into_stream(command_from_action(&raw, &mut plots)).is_none());
        assert!(!plots.theme_jobs.running);

        let request = mlua::Value::UserData(
            lua.create_userdata(crate::theme_gen::GenerateTheme)
                .unwrap(),
        );
        let _ = command_from_action(&request, &mut plots);
        assert!(plots.theme_jobs.running);
        assert_eq!(
            plots.theme_jobs.requested_theme.as_deref(),
            Some("user-defined-theme")
        );
        let _ = command_from_action(&request, &mut plots);
        assert!(
            plots.theme_jobs.dirty,
            "in-flight requests must coalesce, not disappear"
        );

        plots.theme_jobs.running = false;
        plots.config.theme.name = "another-theme".into();
        assert!(iced_runtime::task::into_stream(plots.fire_regen_theme()).is_none());
        assert!(
            !plots.theme_jobs.dirty,
            "do not run a queued request after its theme changes"
        );
    }

    #[test]
    fn palette_setters_only_produce_commands_when_returned() {
        assert_eq!(plants("return iced.palette({primary='#89b4fa'})").len(), 1);
        assert!(plants("iced.palette({primary='#89b4fa'}); return nil").is_empty());
        assert!(plants("return iced.palette({})").is_empty());
    }

    #[test]
    fn native_palette_owns_validated_colors_after_input_changes_and_vm_drop() {
        let messages = plants(
            "local colors = {primary='#010203'}; local patch = iced.palette(colors); colors.primary = 'invalid'; return patch",
        );
        let [
            Plant::Config(super::super::ConfigEvent::Patch(
                crate::config::ConfigPatch::ThemePalette(patch),
            )),
        ] = messages.as_slice()
        else {
            panic!("expected one palette update");
        };
        assert_eq!(
            serde_json::to_value(patch).unwrap(),
            serde_json::json!({"primary":"#010203"})
        );
    }

    #[test]
    fn palette_updates_are_idempotent_and_do_not_render_external_templates() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let root = tempfile::tempdir_in("/tmp/opencode").unwrap();
                let output = root.path().join("external-app-colors");
                std::fs::write(root.path().join("input"), "{{colors.primary}}").unwrap();
                std::fs::write(
                    root.path().join("config.toml"),
                    format!(
                        "[templates.probe]\ninput_path = 'input'\noutput_path = {:?}\n",
                        output.to_str().unwrap()
                    ),
                )
                .unwrap();
                let mut plots = plots();
                plots.config = crate::config::Config::default();
                plots.config.theme.templates_dir = root.path().to_string_lossy().into_owned();
                let colors: crate::theme::palette::PaletteOverrides =
                    serde_json::from_value(serde_json::json!({"primary":"#010203"})).unwrap();
                let patch = || {
                    Plant::Config(super::super::ConfigEvent::Patch(
                        crate::config::ConfigPatch::ThemePalette(colors.clone()),
                    ))
                };
                let first = plots.update(patch());
                let sequence = plots.config_jobs.sequence;
                let results = iced_runtime::task::into_stream(first)
                    .unwrap()
                    .collect::<Vec<_>>()
                    .await;
                assert!(!results.iter().any(|action| matches!(
                    action,
                    iced_runtime::Action::Output(Plant::Config(
                        super::super::ConfigEvent::TemplatesDone(_)
                    ))
                )));
                assert!(
                    !output.exists(),
                    "palette overrides ran external app templates"
                );
                assert!(iced_runtime::task::into_stream(plots.update(patch())).is_none());
                assert_eq!(plots.config_jobs.sequence, sequence);
                let lua = Lua::new();
                crate::ui::dsl::inject_ui_base(&lua).unwrap();
                let repeated = lua
                    .load("return iced.palette({primary='#010203'})")
                    .eval()
                    .unwrap();
                assert!(
                    iced_runtime::task::into_stream(command_from_action(&repeated, &mut plots))
                        .is_none()
                );
                let empty = Plant::Config(super::super::ConfigEvent::Patch(
                    crate::config::ConfigPatch::ThemePalette(Default::default()),
                ));
                assert!(iced_runtime::task::into_stream(plots.update(empty)).is_none());
                assert_eq!(plots.config_jobs.sequence, sequence);
            });
    }

    #[test]
    fn unrelated_values_and_legacy_shell_commands_are_inert() {
        assert!(plants("assert(shell == nil); return {shell={action='open-settings'}}").is_empty());
        assert!(plants("return {other=true}").is_empty());
        assert!(plants("return nil").is_empty());
    }
}
