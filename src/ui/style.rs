//! Shared editor styling; inputs do not define independent palettes.
use crate::theme;
use iced::widget::text_input;
pub(crate) fn input_style(_: &iced::Theme, _: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: iced::Background::Color(theme::card()),
        border: iced::Border {
            color: theme::border_color(),
            width: theme::BORDER_WIDTH,
            radius: theme::RADIUS.into(),
        },
        icon: theme::text_dim(),
        placeholder: theme::text_dim(),
        value: theme::text(),
        selection: theme::text().scale_alpha(0.3),
    }
}
