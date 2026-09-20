use crate::app::Message;
use iced::widget::{container, Space};
use iced::window;
use iced::window::Id;
use iced::{Element, Fill};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};

#[derive(Debug)]
pub struct Overlay;

impl Overlay {
    pub fn open(&self, output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: Anchor::Top,
            layer: Layer::Overlay,
            exclusive_zone: Some(0),
            output_option: OutputOption::GlobalName(output),
            namespace: Some("Riced - Overlay".to_string()),
            ..Default::default()
        };

        (id, settings)
    }

    pub fn view(&self) -> Element<'_, Message> {
        container(Space::new())
            .width(Fill)
            .height(Fill)
            .style(|_| container::Style {
                background: Some(iced::Color::TRANSPARENT.into()),
                ..Default::default()
            })
            .into()
    }
}
