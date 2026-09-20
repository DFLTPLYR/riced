use crate::app::Message;
use iced::widget::{container, text};
use iced::window;
use iced::window::Id;
use iced::{Element, Fill};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};

#[derive(Debug)]
pub struct Top {
    thickness: u32,
}

impl Top {
    pub fn new() -> Self {
        Self { thickness: 50 }
    }

    pub fn open(&self, output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: Anchor::Top,
            layer: Layer::Top,
            exclusive_zone: Some(self.thickness as i32),
            size: LayerSize::fill_width(self.thickness),
            output_option: OutputOption::GlobalName(output),
            namespace: Some("Riced - Top".to_string()),
            blur_option: BlurOption::FullRegion,
            ..Default::default()
        };

        (id, settings)
    }

    pub fn view(&self) -> Element<'_, Message> {
        container(text("TOP BAR TEST").size(30))
            .width(Fill)
            .height(Fill)
            .into()
    }
}
