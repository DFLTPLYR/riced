//! Snapshot-only Settings pages; these do not borrow the shell state.
use super::{hint, section, section_heading, swatch};
use crate::{
    config::{AnimationSpeed, ConfigPatch, ThemeConfig},
    shell::{ConfigEvent, Plant},
    theme,
};
use iced::{
    Element, Length,
    widget::{Space, button, column, row, text},
};

pub(super) fn theme_page(current: &ThemeConfig) -> Element<'static, Plant> {
    let mut list = column![
        section_heading(
            "Color mode",
            &format!(
                "Current palette: {} · {}",
                current.name,
                if current.darkmode { "Dark" } else { "Light" }
            )
        ),
        row![
            button(text("Dark").size(13).color(theme::text()))
                .width(Length::Fill)
                .on_press(Plant::Config(ConfigEvent::Patch(
                    ConfigPatch::ThemeDarkmode(true)
                )))
                .padding(8)
                .style(theme::nav_button(current.darkmode)),
            button(text("Light").size(13).color(theme::text()))
                .width(Length::Fill)
                .on_press(Plant::Config(ConfigEvent::Patch(
                    ConfigPatch::ThemeDarkmode(false)
                )))
                .padding(8)
                .style(theme::nav_button(!current.darkmode)),
        ]
        .spacing(8),
    ]
    .spacing(16);
    list = list.push(section_heading(
        "Choose a palette",
        "Swatches preview the primary, secondary, and tertiary colors.",
    ));
    for name in theme::available_themes() {
        let selected = name == current.name;
        let preview = theme::preview(&name, current.darkmode);
        list = list.push(
            button(
                row![
                    text(name.clone()).size(13).color(preview.on_surface),
                    text(if selected { "Selected" } else { "" })
                        .size(11)
                        .color(preview.on_surface),
                    Space::new().width(Length::Fill),
                    row![
                        swatch(preview.primary, preview.outline),
                        swatch(preview.secondary, preview.outline),
                        swatch(preview.tertiary, preview.outline)
                    ]
                    .spacing(4),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .on_press(Plant::Config(ConfigEvent::Patch(ConfigPatch::ThemeName(
                name,
            ))))
            .padding(14)
            .style(theme::preview_button(preview, selected)),
        );
    }
    list.width(Length::Fill).into()
}

pub(super) fn animation_page(current: AnimationSpeed) -> Element<'static, Plant> {
    let mut speeds = column![].spacing(10);
    for speed in AnimationSpeed::all() {
        let selected = speed == current;
        speeds = speeds.push(
            button(
                column![
                    row![
                        text(speed.title())
                            .size(14)
                            .color(theme::text())
                            .width(Length::Fill),
                        text(format!(
                            "{} ms{}",
                            speed.duration().as_millis(),
                            if selected { " · Selected" } else { "" }
                        ))
                        .size(12)
                        .color(theme::text())
                    ]
                    .spacing(8),
                    hint(match speed {
                        AnimationSpeed::Fast => "Quick and responsive",
                        AnimationSpeed::Medium => "Balanced everyday transitions",
                        AnimationSpeed::Slow => "Relaxed and more pronounced",
                    }),
                ]
                .spacing(5)
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .on_press(Plant::Config(ConfigEvent::Patch(
                ConfigPatch::AnimationSpeed(speed),
            )))
            .padding(14)
            .style(theme::nav_button(selected)),
        );
    }
    column![
        section(
            "Transition speed",
            "Shorter durations feel snappier; longer durations make changes more gradual.",
            speeds.width(Length::Fill).into()
        ),
        hint("Some Lua listviews define their own durations and override this global setting.")
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}
