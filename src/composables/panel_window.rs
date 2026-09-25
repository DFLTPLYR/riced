use crate::app::{BackgroundEvent, Plant, TopEvent};
use iced::mouse::Button;
use iced::widget::{Space, container, mouse_area};
use iced::window;
use iced::{Element, Fill};

/// QML-like `PanelWindow { }` — Fill-sized `mouse_area` every layer builds its
/// view on, so press/release stop living in per-layer `handle_graft`.
///
/// ```ignore
/// panel_window()
///     .content(text("TOP BAR"))
///     .on_press(Plant::TopPlot(TopEvent::Pressed(id, Button::Left)))
///     .on_release(Plant::TopPlot(TopEvent::Released(id, Button::Left)))
///     .into()
/// ```
pub struct PanelWindow<'a> {
    content: Option<Element<'a, Plant>>,
    on_press: Option<Plant>,
    on_release: Option<Plant>,
    on_right_press: Option<Plant>,
    on_right_release: Option<Plant>,
    on_middle_press: Option<Plant>,
    on_middle_release: Option<Plant>,
}

impl<'a> Default for PanelWindow<'a> {
    fn default() -> Self {
        PanelWindow {
            content: None,
            on_press: None,
            on_release: None,
            on_right_press: None,
            on_right_release: None,
            on_middle_press: None,
            on_middle_release: None,
        }
    }
}

pub fn panel_window<'a>() -> PanelWindow<'a> {
    PanelWindow {
        ..Default::default()
    }
}

/// Pre-wired shell for Top bars: all six press/release handlers emit
/// `TopEvent::Pressed/Released(id, button)`. Layer only provides content:
///
/// ```ignore
/// top_window(id).content(text("TOP BAR")).into()
/// ```
pub fn top_window<'a>(id: window::Id) -> PanelWindow<'a> {
    panel_window()
        .on_press(Plant::TopPlot(TopEvent::Pressed(id, Button::Left)))
        .on_release(Plant::TopPlot(TopEvent::Released(id, Button::Left)))
        .on_right_press(Plant::TopPlot(TopEvent::Pressed(id, Button::Right)))
        .on_right_release(Plant::TopPlot(TopEvent::Released(id, Button::Right)))
        .on_middle_press(Plant::TopPlot(TopEvent::Pressed(id, Button::Middle)))
        .on_middle_release(Plant::TopPlot(TopEvent::Released(id, Button::Middle)))
}

/// Pre-wired shell for Background windows: all six press/release handlers
/// emit `BackgroundEvent::Pressed/Released(id, button)`.
///
/// ```ignore
/// background_window(id).content(stack(vec![bg, selection])).into()
/// ```
pub fn background_window<'a>(id: window::Id) -> PanelWindow<'a> {
    panel_window()
        .on_press(Plant::BackgroundPlot(BackgroundEvent::Pressed(
            id,
            Button::Left,
        )))
        .on_release(Plant::BackgroundPlot(BackgroundEvent::Released(
            id,
            Button::Left,
        )))
        .on_right_press(Plant::BackgroundPlot(BackgroundEvent::Pressed(
            id,
            Button::Right,
        )))
        .on_right_release(Plant::BackgroundPlot(BackgroundEvent::Released(
            id,
            Button::Right,
        )))
        .on_middle_press(Plant::BackgroundPlot(BackgroundEvent::Pressed(
            id,
            Button::Middle,
        )))
        .on_middle_release(Plant::BackgroundPlot(BackgroundEvent::Released(
            id,
            Button::Middle,
        )))
}

impl<'a> PanelWindow<'a> {
    pub fn content(mut self, content: impl Into<Element<'a, Plant>>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn on_press(mut self, message: Plant) -> Self {
        self.on_press = Some(message);
        self
    }

    pub fn on_release(mut self, message: Plant) -> Self {
        self.on_release = Some(message);
        self
    }

    pub fn on_right_press(mut self, message: Plant) -> Self {
        self.on_right_press = Some(message);
        self
    }

    pub fn on_right_release(mut self, message: Plant) -> Self {
        self.on_right_release = Some(message);
        self
    }

    pub fn on_middle_press(mut self, message: Plant) -> Self {
        self.on_middle_press = Some(message);
        self
    }

    pub fn on_middle_release(mut self, message: Plant) -> Self {
        self.on_middle_release = Some(message);
        self
    }

    pub fn view(self) -> Element<'a, Plant> {
        let content = self
            .content
            .unwrap_or_else(|| Space::new().width(0).height(0).into());
        let mut area = mouse_area(container(content).width(Fill).height(Fill));
        if let Some(m) = self.on_press {
            area = area.on_press(m);
        }
        if let Some(m) = self.on_release {
            area = area.on_release(m);
        }
        if let Some(m) = self.on_right_press {
            area = area.on_right_press(m);
        }
        if let Some(m) = self.on_right_release {
            area = area.on_right_release(m);
        }
        if let Some(m) = self.on_middle_press {
            area = area.on_middle_press(m);
        }
        if let Some(m) = self.on_middle_release {
            area = area.on_middle_release(m);
        }
        area.into()
    }
}

impl<'a> From<PanelWindow<'a>> for Element<'a, Plant> {
    fn from(panel: PanelWindow<'a>) -> Self {
        panel.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;
    use iced::window;

    #[test]
    fn press_release_chain_compiles() {
        let id = window::Id::unique();
        let _: Element<'_, Plant> = panel_window()
            .content(text("hi"))
            .on_press(Plant::Tend)
            .on_release(Plant::Uproot(id))
            .on_right_press(Plant::Tend)
            .into();
    }
}
