//! A Settings page that is a real, persistent Lua app.
//!
//! Ordinary Settings pages are rendered as one-shot trees with props supplied
//! by Rust. A Lua page keeps its VM alive so it can own `riced.file` handles,
//! receive its own actions (`settings:lua:<key>`), and pump managed work.
use crate::{
    framework::tasks::{Delivery, DeliveryKind, Owner},
    lua::{AppEvent, LuaRuntime},
    services::ServiceCtx,
    shell::{Plant, state::Plots},
    ui::action::{Action, Event},
};
use iced::window;
use serde_json::json;

pub(super) struct LuaPage {
    runtime: LuaRuntime,
    owner: Owner,
    mounted: bool,
    error: Option<String>,
}

impl std::fmt::Debug for LuaPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LuaPage").finish_non_exhaustive()
    }
}

impl LuaPage {
    pub(super) fn new(source: &str, label: &str) -> Result<Self, String> {
        let root = crate::config::settings_dir()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let mut runtime =
            LuaRuntime::with_components(Vec::new()).map_err(|error| error.to_string())?;
        runtime
            .set_app_root(&root)
            .map_err(|error| error.to_string())?;
        runtime
            .load_named(source, label)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            runtime,
            owner: Owner::default(),
            mounted: false,
            error: None,
        })
    }

    fn services<'a>(plots: &'a Plots) -> ServiceCtx<'a> {
        ServiceCtx::from_plots(plots, plots.services.gpu())
    }

    fn ensure(&mut self, plots: &Plots) {
        if self.mounted {
            return;
        }
        self.mounted = true;
        let services = Self::services(plots);
        let result = self
            .runtime
            .app_event(&services, "", &json!({}), &json!({}), AppEvent::Mount);
        if let Err(error) = result {
            crate::lua::error::report_once(&mut self.error, "settings page: ", error.to_string());
        }
    }

    pub(super) fn view(&mut self, plots: &Plots) -> Result<crate::ui::node::WidgetNode, String> {
        self.ensure(plots);
        let services = Self::services(plots);
        let _ = services;
        self.runtime
            .component_view_with_images(&json!({}), &json!({}), &plots.config.theme, false, &[])
            .map(|(node, _)| node)
            .map_err(|error| error.to_string())
    }

    pub(super) fn action(&mut self, plots: &Plots, key: &str) -> Result<mlua::Value, String> {
        self.ensure(plots);
        let services = Self::services(plots);
        let action = Action {
            key: key.to_owned(),
            value: None,
            event: Event::Action,
        };
        self.runtime
            .app_event(
                &services,
                "",
                &json!({}),
                &json!({}),
                AppEvent::Action(&action),
            )
            .map_err(|error| error.to_string())
    }

    pub(super) fn managed_work(&mut self, plots: &Plots, id: window::Id) -> Vec<iced::Task<Plant>> {
        let services = Self::services(plots);
        let Some(generation) = self.runtime.managed_generation() else {
            return Vec::new();
        };
        self.owner.reset(generation);
        self.owner.apply(self.runtime.managed_commands());
        let mut tasks: Vec<_> = self
            .owner
            .jobs()
            .into_iter()
            .map(|job| {
                iced::Task::perform(job.run(), move |delivery| {
                    Plant::SettingPlot(crate::shell::SettingEvent::LuaManaged { id, delivery })
                })
            })
            .collect();
        match self.runtime.managed_services(&services) {
            Ok(deliveries) => tasks.extend(deliveries.into_iter().map(move |delivery| {
                iced::Task::done(Plant::SettingPlot(crate::shell::SettingEvent::LuaManaged {
                    id,
                    delivery,
                }))
            })),
            Err(error) => crate::lua::error::report_once(
                &mut self.error,
                "settings page service: ",
                error.to_string(),
            ),
        }
        tasks
    }

    pub(super) fn deliver(
        &mut self,
        plots: &Plots,
        delivery: &Delivery,
    ) -> Option<Result<mlua::Value, String>> {
        if !self.runtime.accepts_managed(delivery) {
            return None;
        }
        if delivery.kind == DeliveryKind::Job && !self.owner.complete(delivery) {
            return None;
        }
        let services = Self::services(plots);
        Some(
            self.runtime
                .app_event(
                    &services,
                    "",
                    &json!({}),
                    &json!({}),
                    AppEvent::Managed(delivery),
                )
                .map_err(|error| error.to_string()),
        )
    }
}
