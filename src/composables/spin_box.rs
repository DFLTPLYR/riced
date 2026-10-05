use crate::app::Plant;
use crate::theme;
use iced::widget::row::Row;
use iced::widget::{button, row, text_input};
use iced::{Alignment, Length};
use std::ops::RangeInclusive;

/// QML-like `SpinBox` — a `[-] type-in [+]` stepper row (label lives with
/// the caller, so rows like `text(..) + spin_box(..)` compose freely).
///
/// Typing live-commits whenever it parses (clamped); unparsable keystrokes
/// re-commit the current value, so the field snaps back instead of holding
/// invalid text — no draft state, no new events. Steppers commit a single
/// clamped step per press.
///
/// ```ignore
/// row![
///     text("Margin top (px)").width(Length::FillPortion(7)),
///     spin_box(
///         top.local.margins.top as f64,
///         0.0..=256.0,
///         1.0,
///         0,
///         move |v| Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(wid, Edge::Top, v as i32))),
///     )
///     .width(Length::FillPortion(2)),
/// ]
/// ```
pub fn spin_box(
    value: f64,
    range: RangeInclusive<f64>,
    step: f64,
    decimals: usize,
    on_commit: impl Fn(f64) -> Plant + Clone + 'static,
) -> Row<'static, Plant> {
    let (min, max) = (*range.start(), *range.end());
    let shown = format!("{:.*}", decimals, value.clamp(min, max));

    let commit_input = on_commit.clone();
    let input = text_input("", &shown)
        .size(12)
        .width(Length::Fill)
        .padding(6)
        .style(input_style)
        .on_input(move |typed| match typed.trim().parse::<f64>() {
            Ok(v) => commit_input(v.clamp(min, max)),
            Err(_) => commit_input(value),
        });

    // Icons inherit the button text color and recolor with its states.
    let commit_down = on_commit.clone();
    let down = button(lucide_iced::themed_icon(lucide_iced::bytes::MINUS, 8.0))
        .width(Length::Fixed(30.0))
        .height(Length::Fixed(30.0))
        .padding(4)
        .style(theme::menu_button(theme::RADIUS))
        .on_press(commit_down((value - step).clamp(min, max)));
    let up = button(lucide_iced::themed_icon(lucide_iced::bytes::PLUS, 8.0))
        .width(Length::Fixed(30.0))
        .height(Length::Fixed(30.0))
        .padding(4)
        .style(theme::menu_button(theme::RADIUS))
        .on_press(on_commit((value + step).clamp(min, max)));

    row![down, input, up]
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill)
}

/// Settings-surface input: card background, hairline border, theme text.
fn input_style(_: &iced::Theme, _: text_input::Status) -> text_input::Style {
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Element;

    #[test]
    fn builds_with_commit_closure() {
        let _: Element<'_, Plant> = spin_box(8.0, 0.0..=256.0, 1.0, 0, |_| Plant::Tend).into();
    }
}
