use crate::app::Plant;
use crate::app::app::PlotInfo;
use iced::widget::{Space, container};
use iced::window;
use iced::{Element, Fill, Task as Command};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};
use iced_wayland_subscriber::OutputId;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Background;

impl Background {
    pub fn open(&self, output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();
        let settings = NewLayerShellSettings {
            anchor: Anchor::all(),
            layer: Layer::Background,
            exclusive_zone: Some(0),
            size: LayerSize::FILL,
            output_option: OutputOption::GlobalName(output),
            namespace: Some("Riced - Background".to_string()),
            blur_option: BlurOption::None,
            ..Default::default()
        };
        (id, settings)
    }

    pub fn view(&self) -> Element<'_, Plant> {
        // Background itself is transparent; selectionRect is drawn by Plots view, not here
        container(Space::new()).width(Fill).height(Fill).into()
    }

    /// Ensure a fullscreen Background exists for `output_id` (called on `LandEvent::OutputAdded`).
    /// Returns a `NewLayerShell` command if a new background was created.
    pub(crate) fn ensure_for_output(
        backgrounds: &mut HashMap<OutputId, Background>,
        background_ids: &mut HashMap<OutputId, window::Id>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Option<Command<Plant>> {
        if backgrounds.contains_key(&output_id) {
            return None;
        }
        let bg = Background;
        let (id, settings) = bg.open(output_id.0);
        backgrounds.insert(output_id, bg);
        background_ids.insert(output_id, id);
        ids.insert(id, PlotInfo::Background(output_id));
        Some(Command::done(Plant::NewLayerShell { settings, id }))
    }

    /// Remove the Background for `output_id` (called on `LandEvent::OutputRemoved`).
    /// Returns the closed window `Id` if one existed.
    pub(crate) fn remove_for_output(
        backgrounds: &mut HashMap<OutputId, Background>,
        background_ids: &mut HashMap<OutputId, window::Id>,
        ids: &mut HashMap<window::Id, PlotInfo>,
        output_id: OutputId,
    ) -> Option<window::Id> {
        let id = background_ids.remove(&output_id)?;
        backgrounds.remove(&output_id);
        ids.remove(&id);
        Some(id)
    }
}
