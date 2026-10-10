//! Native notification creation takes only output/window, size, and config state.
use crate::{
    config::NotificationConfig,
    shell::{
        Plant,
        windows::{PlotInfo, WindowState},
    },
};
use iced::{Task, window};
use iced_exwlshell::reexport::{
    BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

pub(super) struct OutputContext<'a> {
    pub outputs: &'a HashMap<OutputId, OutputInfo>,
    pub cursor: Option<iced::Point>,
}

impl OutputContext<'_> {
    pub fn resolve(&self, config: &NotificationConfig) -> Option<OutputId> {
        if config.output != "mouse" {
            let wanted = config.output.to_lowercase();
            if let Some((id, _)) = self.outputs.iter().find(|(_, info)| {
                info.name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase() == wanted)
            }) {
                return Some(*id);
            }
        }
        if let Some(cursor) = self.cursor {
            let geometry: Vec<_> = self
                .outputs
                .iter()
                .map(|(id, info)| (*id, crate::shared::geometry::output_geometry(info)))
                .collect();
            if let Some(output) = super::output_at(&geometry, cursor) {
                return Some(output);
            }
        }
        self.outputs.keys().copied().next()
    }
}

pub(super) struct SurfaceContext<'a> {
    pub windows: &'a mut WindowState,
    pub sizes: &'a mut HashMap<OutputId, u32>,
    pub config: &'a NotificationConfig,
}

impl SurfaceContext<'_> {
    pub fn ensure(&mut self, output: OutputId) -> Option<Task<Plant>> {
        if self.windows.notifications.contains_key(&output) {
            return None;
        }
        let id = window::Id::unique();
        let (anchor, margin) = super::placement(self.config);
        let height = self
            .windows
            .output_infos
            .get(&output)
            .map(|info| crate::shared::geometry::output_geometry(info).3)
            .unwrap_or(1080.0);
        let height = super::window_height_for_output(height);
        let settings = NewLayerShellSettings {
            anchor,
            layer: Layer::Overlay,
            exclusive_zone: None,
            size: LayerSize::px(self.config.width.max(200.0) as u32, height),
            output_option: OutputOption::GlobalName(output.0),
            margin: Some(margin),
            namespace: Some(format!("Riced - Notifications {output:?}")),
            keyboard_interactivity: KeyboardInteractivity::None,
            events_transparent: true,
            blur_option: BlurOption::None,
        };
        self.windows.notifications.insert(output, id);
        self.windows.ids.insert(id, PlotInfo::Notification(output));
        self.sizes.insert(output, height);
        Some(Task::done(Plant::NewLayerShell { settings, id }))
    }
}
