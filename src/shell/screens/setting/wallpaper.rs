//! Wallpaper page snapshots; rendering has no access to unrelated shell domains.
use super::editors::image_spin_row;
use super::{Setting, hint, image_file_name, section, section_heading};
use crate::ui::widgets::display_map::{MapLayer, MapView, images_layer, outputs_layer};
use crate::{
    config::{BackgroundImage, ConfigPatch},
    shell::{BackgroundEvent, ConfigEvent, Plant, SettingEvent, state::Plots},
    theme,
};
use iced::{
    Element, Length,
    widget::{Space, button, column, container, image::Handle, row, scrollable, stack, text},
    window,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::{collections::HashMap, path::PathBuf};

pub(super) struct WallpaperContext<'a> {
    pub images: &'a Vec<BackgroundImage>,
    pub output_infos: &'a HashMap<OutputId, OutputInfo>,
    pub handles: &'a HashMap<PathBuf, Handle>,
}

impl<'a> From<&'a Plots> for WallpaperContext<'a> {
    fn from(plots: &'a Plots) -> Self {
        Self {
            images: &plots.config.background.image,
            output_infos: &plots.windows.output_infos,
            handles: &plots.desktop.wallpapers,
        }
    }
}

impl WallpaperContext<'_> {
    pub fn wallpaper_handle(&self, image: &BackgroundImage) -> Option<Handle> {
        self.handles.get(&image.local_path()).cloned()
    }
}

pub(super) fn view<'a>(
    setting: &'a Setting,
    id: window::Id,
    context: &WallpaperContext<'_>,
) -> Element<'a, Plant> {
    column![
        section_heading("Display layout", "Drag images to position them. Use the map to see how they overlap your displays."),
        row![button(text("Add wallpaper…").size(13).color(theme::button_text()))
            .on_press(Plant::BackgroundPlot(BackgroundEvent::PickWallpaper)).padding(8).style(theme::menu_button(theme::RADIUS)),
            Space::new().width(Length::Fill)].spacing(8),
        grid(id, context, setting.map_view),
        section("Image properties", "Choose an image below to adjust its placement. Reset restores that property's default.", image_controls(id, context, setting.selected_image)),
    ].spacing(16).into()
}

fn grid(id: window::Id, context: &WallpaperContext<'_>, view: MapView) -> Element<'static, Plant> {
    let mut outputs: Vec<_> = context
        .output_infos
        .values()
        .map(crate::shared::geometry::output_geometry)
        .collect();
    outputs.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let images = context.images.clone();
    let handles: Vec<_> = images
        .iter()
        .map(|image| context.wallpaper_handle(image))
        .collect();
    container(stack![
        images_layer(id, outputs.clone(), images.clone(), handles.clone(), view),
        outputs_layer(id, outputs, images, handles, view)
    ])
    .style(theme::menu_box)
    .height(Length::Fixed(360.0))
    .clip(true)
    .width(Length::Fill)
    .into()
}

fn image_controls(
    id: window::Id,
    context: &WallpaperContext<'_>,
    selected: Option<usize>,
) -> Element<'static, Plant> {
    let images = context.images;
    if images.is_empty() {
        return column![
            text("No images yet.").size(13).color(theme::text()),
            text("Add wallpaper… to place the first one.").size(11)
        ]
        .spacing(4)
        .into();
    }
    let sel = selected.filter(|index| *index < images.len()).unwrap_or(0);
    let mut picker = row![].spacing(8);
    for (index, image) in images.iter().enumerate() {
        picker = picker.push(
            button(
                text(format!("{} · {}", index + 1, image_file_name(image)))
                    .size(12)
                    .color(theme::text()),
            )
            .on_press(Plant::SettingPlot(SettingEvent::SelectImage(id, index)))
            .padding(8)
            .style(theme::nav_button(index == sel)),
        );
    }
    let image = &images[sel];
    let name = image_file_name(image);
    let (ix, iy, iz, iw, ih, scale) = (
        image.x,
        image.y,
        image.z,
        image.width,
        image.height,
        image.scale,
    );
    let (fw, fh) = MapLayer::file_dimensions(image).unwrap_or((0.0, 0.0));
    let (shown_width, shown_height) = MapLayer::size_with_file(image, (fw, fh));
    let shown_scale = scale.max(0.01);
    let mut col = column![
        scrollable(picker).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default()
        ))
    ]
    .spacing(16);
    col = col.push(
        row![
            text(format!("Image {} · {name}", sel + 1))
                .size(13)
                .color(theme::text())
                .width(Length::Fill),
            button(text("Remove image").size(12).color(theme::active().error))
                .on_press(Plant::Config(ConfigEvent::Patch(
                    ConfigPatch::RemoveImage { index: sel }
                )))
                .padding(6)
                .style(theme::menu_button_tinted(
                    theme::RADIUS,
                    theme::active().error
                ))
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    );
    col = col.push(hint(format!("Source resolution: {fw:.0} × {fh:.0} px")));
    col = col.push(section_heading(
        "Position & stacking",
        "X and Y are desktop coordinates. Higher Z values place the image in front.",
    ));
    col = col.push(image_spin_row(
        "X (px)",
        ix as f64,
        -20000.0..=20000.0,
        1.0,
        1,
        move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
                index: sel,
                x: value as f32,
                y: iy,
            }))
        },
        Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
            index: sel,
            x: 0.0,
            y: iy,
        })),
    ));
    col = col.push(image_spin_row(
        "Y (px)",
        iy as f64,
        -20000.0..=20000.0,
        1.0,
        1,
        move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
                index: sel,
                x: ix,
                y: value as f32,
            }))
        },
        Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveImage {
            index: sel,
            x: ix,
            y: 0.0,
        })),
    ));
    col = col.push(image_spin_row(
        "Z (stack)",
        iz as f64,
        -100.0..=100.0,
        1.0,
        0,
        move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageZ {
                index: sel,
                z: value as i32,
            }))
        },
        Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageZ {
            index: sel,
            z: 0,
        })),
    ));
    col = col.push(section_heading(
        "Dimensions & scale",
        "Width and height define the base size; scale multiplies both dimensions.",
    ));
    col = col.push(image_spin_row(
        "Width (px)",
        shown_width as f64,
        0.0..=16000.0_f64.max(shown_width as f64),
        1.0,
        0,
        move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
                index: sel,
                width: value as f32,
                height: ih,
            }))
        },
        Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
            index: sel,
            width: fw,
            height: ih,
        })),
    ));
    col = col.push(image_spin_row(
        "Height (px)",
        shown_height as f64,
        0.0..=16000.0_f64.max(shown_height as f64),
        1.0,
        0,
        move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
                index: sel,
                width: iw,
                height: value as f32,
            }))
        },
        Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageSize {
            index: sel,
            width: iw,
            height: fh,
        })),
    ));
    col = col.push(image_spin_row(
        "Scale (×)",
        shown_scale as f64,
        0.01..=10.0_f64.max(shown_scale as f64),
        0.1,
        2,
        move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageScale {
                index: sel,
                scale: value as f32,
            }))
        },
        Plant::Config(ConfigEvent::Patch(ConfigPatch::SetImageScale {
            index: sel,
            scale: 1.0,
        })),
    ));
    col.spacing(8).width(Length::Fill).into()
}
