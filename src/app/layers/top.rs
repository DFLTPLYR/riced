use crate::app::Plant;
use iced::widget::{container, text};
use iced::window;
use iced::{Element, Fill};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};

#[derive(Debug)]
pub struct Top {
    thickness: u32,
    anchor: Anchor,
}

impl Top {
    pub fn new() -> Self {
        Self {
            thickness: 50,
            anchor: Anchor::Top,
        }
    }

    pub fn with_anchor(anchor: Anchor) -> Self {
        Self {
            thickness: 50,
            anchor,
        }
    }

    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    fn layer_size(&self) -> LayerSize {
        // Top/Bottom span width (fill_width), Left/Right span height (fill_height)
        if self.anchor == Anchor::Left || self.anchor == Anchor::Right {
            LayerSize::fill_height(self.thickness)
        } else {
            LayerSize::fill_width(self.thickness)
        }
    }

    fn anchor_label(&self) -> &'static str {
        if self.anchor == Anchor::Top {
            "TOP"
        } else if self.anchor == Anchor::Bottom {
            "BOTTOM"
        } else if self.anchor == Anchor::Left {
            "LEFT"
        } else if self.anchor == Anchor::Right {
            "RIGHT"
        } else {
            "BAR"
        }
    }

    /// Open a Top bar for a specific output (GlobalName).
    /// Called on `WayEvent::OutputInsert` which fires at startup for each
    /// active output when `StartMode::AllScreens`.
    pub fn open(&self, output: u32) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(self.thickness as i32),
            size: self.layer_size(),
            output_option: OutputOption::GlobalName(output),
            namespace: Some(format!("Riced - {} {}", self.anchor_label(), output)),
            blur_option: BlurOption::FullRegion,
            ..Default::default()
        };

        (id, settings)
    }

    /// Fallback for startup when no OutputId is known yet (uses Active output)
    pub fn open_active(&self) -> (window::Id, NewLayerShellSettings) {
        let id = window::Id::unique();

        let settings = NewLayerShellSettings {
            anchor: self.anchor,
            layer: Layer::Top,
            exclusive_zone: Some(self.thickness as i32),
            size: self.layer_size(),
            output_option: OutputOption::Active,
            namespace: Some(format!("Riced - {} Active", self.anchor_label())),
            blur_option: BlurOption::FullRegion,
            ..Default::default()
        };

        (id, settings)
    }

    pub fn view(&self) -> Element<'_, Plant> {
        container(text(format!("{} BAR TEST", self.anchor_label())).size(30))
            .width(Fill)
            .height(Fill)
            .into()
    }
}
