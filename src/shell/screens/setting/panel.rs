//! Read-only inputs required by the Panel page and placement editor.
use crate::{
    config::WidgetDefinition,
    shell::{
        screens::Top,
        state::{PlotInfo, Plots},
    },
};
use iced::window;
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

pub(super) struct PanelContext<'a> {
    pub ids: &'a HashMap<window::Id, PlotInfo>,
    pub tops: &'a HashMap<window::Id, Top>,
    pub output_infos: &'a HashMap<OutputId, OutputInfo>,
    pub widgets: &'a [WidgetDefinition],
}

impl<'a> From<&'a Plots> for PanelContext<'a> {
    fn from(plots: &'a Plots) -> Self {
        Self {
            ids: &plots.ids,
            tops: &plots.tops,
            output_infos: &plots.output_infos,
            widgets: &plots.widgets,
        }
    }
}

impl PanelContext<'_> {
    pub fn output_name(&self, bar: window::Id) -> String {
        let Some(PlotInfo::Top(output)) = self.ids.get(&bar) else {
            return String::new();
        };
        self.output_infos
            .get(output)
            .and_then(|info| info.name.clone())
            .unwrap_or_default()
    }
}
