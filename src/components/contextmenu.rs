use crate::app::Plant;
use crate::composables::menu::menu;
use iced::widget::{button, column, text};
use iced::{Color, Element, Fill};

/// Right-click context menu overlay (the "Add Top" menu).
/// `width` comes from `[context_menu]`; position is pre-clamped local coords.
pub fn contextmenu<'a>(width: f32, clamped_lx: f32, clamped_ly: f32) -> Element<'a, Plant> {
    menu()
        .content(
            column![
                button(text("Add Top").size(12).color(Color::WHITE))
                    .on_press(Plant::AddTop)
                    .padding(2)
                    .style(|_, _| button::Style {
                        background: Some(Color::from_rgb(0.25, 0.25, 0.28).into()),
                        text_color: Color::WHITE,
                        border: iced::Border {
                            color: Color::from_rgb(0.5, 0.5, 0.55),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    })
                    .width(Fill)
            ]
            .spacing(8)
            .width(Fill),
        )
        .padding(4.0)
        .width(width)
        .position(clamped_lx, clamped_ly)
        .into()
}
