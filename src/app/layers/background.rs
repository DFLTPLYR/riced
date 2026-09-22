use crate::app::Plant;
use iced::widget::{Space, container};
use iced::window;
use iced::{Element, Fill};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};

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

    pub fn open_active(&self) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();
        let settings = NewLayerShellSettings {
            anchor: Anchor::all(),
            layer: Layer::Background,
            exclusive_zone: Some(0),
            size: LayerSize::FILL,
            output_option: OutputOption::Active,
            namespace: Some("Riced - Background".to_string()),
            ..Default::default()
        };
        (id, settings)
    }

    pub fn view(&self) -> Element<'_, Plant> {
        // Background itself is transparent; selectionRect is drawn by Plots view, not here
        container(Space::new()).width(Fill).height(Fill).into()
    }
}
