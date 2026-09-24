use crate::app::Plant;
use crate::config::MenuConfig;
use iced::widget::{button, column, container, text};
use iced::{Color, Element, Fill, Length};

/// QML `Menu { }` content — the inner styled box only.
/// Dimensions come from `[menu]` in `config.toml` (hot-reloaded).
pub fn content<'a>(menu: &MenuConfig) -> Element<'a, Plant> {
    container(
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
    .padding(8)
    .clip(true)
    .width(Length::Fixed(menu.width))
    .height(Length::Fixed(menu.height))
    .style(|_| container::Style {
        background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
        border: iced::Border {
            color: Color::from_rgb(0.5, 0.5, 0.55),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Placement wrapper — positions the menu at `(lx, ly)` local coords
/// via the outer `Fill + padding` trick.
pub fn menu<'a>(cfg: &MenuConfig, clamped_lx: f32, clamped_ly: f32) -> Element<'a, Plant> {
    container(content(cfg))
        .width(Fill)
        .height(Fill)
        .padding(iced::Padding {
            top: clamped_ly,
            left: clamped_lx,
            right: 0.0,
            bottom: 0.0,
        })
        .into()
}
