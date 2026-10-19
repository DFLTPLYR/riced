//! Snapshot-only Settings pages; these do not borrow the shell state.
use super::{hint, section, section_heading, swatch};
use crate::{
    config::{AnimationSpeed, ConfigPatch, ThemeConfig},
    shell::{ConfigEvent, Plant},
    theme,
};
use iced::{
    Element, Length,
    widget::{Space, button, column, container, image, row, text},
};
use std::collections::HashMap;

const PREVIEW_APPS: [(&str, &[&str]); 6] = [
    (
        "Firefox",
        &["firefox", "org.mozilla.firefox", "web-browser"],
    ),
    (
        "Files",
        &["org.gnome.Nautilus", "system-file-manager", "folder"],
    ),
    (
        "Terminal",
        &["org.gnome.Terminal", "utilities-terminal", "terminal"],
    ),
    (
        "Editor",
        &["code", "visual-studio-code", "accessories-text-editor"],
    ),
    (
        "VLC",
        &["vlc", "org.videolan.VLC", "multimedia-video-player"],
    ),
    (
        "Steam",
        &["steam", "com.valvesoftware.Steam", "applications-games"],
    ),
];

pub(super) fn load_icon_previews(current: String) -> super::IconPreviews {
    let mut themes = crate::icons::available_themes();
    if !themes.contains(&current) {
        themes.push(current);
    }
    let mut resolver = crate::icons::Resolver::for_previews();
    let mut decoded = HashMap::new();
    themes
        .into_iter()
        .map(|theme| {
            let icons = PREVIEW_APPS
                .iter()
                .map(|(_, names)| {
                    names.iter().find_map(|name| {
                        let path = resolver.resolve_in_theme(&theme, name, 48, &[])?;
                        decoded
                            .entry(path.clone())
                            .or_insert_with(|| {
                                crate::notify::images::image_file(&path.to_string_lossy())
                            })
                            .clone()
                    })
                })
                .collect();
            (theme, icons)
        })
        .collect()
}

fn icon_theme_card(
    name: String,
    icons: &[Option<image::Handle>],
    selected: bool,
) -> Element<'static, Plant> {
    let mut samples = column![].spacing(10);
    for start in [0, 3] {
        let mut line = row![].spacing(12);
        for index in start..start + 3 {
            let icon: Element<'static, Plant> = match &icons[index] {
                Some(handle) => image(handle.clone()).width(40).height(40).into(),
                None => container(text("?").size(24)).center(40).into(),
            };
            line = line.push(
                column![
                    container(icon).center_x(Length::Fill),
                    container(text(PREVIEW_APPS[index].0).size(10)).center_x(Length::Fill),
                ]
                .spacing(4)
                .width(Length::Fill),
            );
        }
        samples = samples.push(line);
    }
    button(
        column![
            row![
                text(name.clone()).size(13).width(Length::Fill).height(36),
                text(if selected { "Selected" } else { "" }).size(10),
            ]
            .spacing(8),
            samples,
        ]
        .spacing(12),
    )
    .width(Length::Fill)
    .padding(12)
    .style(theme::nav_button(selected))
    .on_press(Plant::Config(ConfigEvent::Patch(ConfigPatch::IconTheme(
        name,
    ))))
    .into()
}

pub(super) fn theme_page(
    current: &ThemeConfig,
    previews: Option<&Result<super::IconPreviews, String>>,
) -> Element<'static, Plant> {
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
        "Icon theme",
        "Used for application, tray, and notification icons. Missing icons fall back to hicolor.",
    ));
    match previews {
        None => list = list.push(text("Loading icon previews…").size(13)),
        Some(Err(error)) => {
            list = list.push(text(format!("Could not load icon previews: {error}")).size(13))
        }
        Some(Ok(previews)) => {
            let cards = previews
                .iter()
                .map(|(name, icons)| {
                    icon_theme_card(name.clone(), icons, name == &current.icon_theme)
                })
                .collect();
            list = list.push(crate::ui::widgets::masonry::masonry(cards, 260.0, 12.0));
        }
    }
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
