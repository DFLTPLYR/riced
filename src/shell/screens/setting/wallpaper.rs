//! Wallpaper page snapshots; rendering has no access to unrelated shell domains.
use super::editors::image_spin_row;
use super::{Setting, hint, image_file_name, section, section_heading};
use crate::ui::widgets::display_map::{MapLayer, MapView, images_layer, outputs_layer};
use crate::{
    config::{BackgroundImage, ConfigPatch},
    shell::{Plant, SettingEvent, state::Plots},
    theme,
};
use iced::{
    Element, Length,
    widget::{Space, button, column, container, image::Handle, row, scrollable, stack, text},
    window,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

pub(super) struct WallpaperContext<'a> {
    pub images: &'a Vec<BackgroundImage>,
    pub output_infos: &'a HashMap<OutputId, OutputInfo>,
    pub sources: &'a crate::wallpaper::raster::Sources,
}

impl WallpaperContext<'_> {
    pub fn wallpaper_handle(&self, image: &BackgroundImage) -> Option<Handle> {
        self.sources
            .get(&image.local_path())
            .map(|source| source.handle.clone())
    }
}

pub(super) fn view<'a>(setting: &'a Setting, id: window::Id, plots: &Plots) -> Element<'a, Plant> {
    let Some(draft) = &setting.wallpaper_draft else {
        return hint("Open the Wallpaper page to load the editor.");
    };
    let context = WallpaperContext {
        images: &draft.images,
        output_infos: &plots.windows.output_infos,
        sources: &draft.sources,
    };
    let mut content = column![
        section_heading("Display layout", "Arrange the original images in the preview. Apply saves lossless, native-resolution wallpapers for each display."),
        row![button(text("Add wallpaper…").size(13).color(theme::button_text()))
            .on_press(Plant::SettingPlot(SettingEvent::PickWallpaper(id))).padding(8).style(theme::menu_button(theme::RADIUS)),
            Space::new().width(Length::Fill),
            hint(if draft.dirty() {"Unapplied changes"} else {"Applied layout"}),
            button(text("Discard").size(13)).on_press(Plant::SettingPlot(SettingEvent::DiscardWallpaper(id))).padding(8).style(theme::menu_button(theme::RADIUS)),
            button(text(if draft.applying {"Applying…"} else {"Apply"}).size(13))
                .on_press_maybe((!draft.loading && !draft.applying).then_some(Plant::SettingPlot(SettingEvent::ApplyWallpaper(id))))
                .padding(8).style(theme::menu_button(theme::RADIUS))].spacing(8).align_y(iced::Alignment::Center),
        grid(id, &context, setting.map_view),
        section("Image properties", "These controls change only the draft. Reset restores that property's default.", image_controls(id, &context, setting.selected_image)),
    ].spacing(16);
    if draft.loading {
        content = content.push(hint("Loading full-resolution originals…"));
    }
    if let Some(error) = &draft.error {
        content = content.push(text(error.clone()).size(12).color(theme::active().error));
    }
    content.into()
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
    let sizes: Vec<_> = images
        .iter()
        .map(|image| {
            context
                .sources
                .get(&image.local_path())
                .map(|source| (source.width as f32, source.height as f32))
        })
        .collect();
    container(stack![
        images_layer(id, outputs.clone(), images.clone(), handles.clone(), view)
            .with_file_sizes(sizes.clone()),
        outputs_layer(id, outputs, images, handles, view).with_file_sizes(sizes)
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
    let (fw, fh) = context
        .sources
        .get(&image.local_path())
        .map(|source| (source.width as f32, source.height as f32))
        .unwrap_or((0.0, 0.0));
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
                .on_press(change(id, ConfigPatch::RemoveImage { index: sel }))
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
        "Placement",
        "X/Y use desktop coordinates. Higher Z paints in front; scale multiplies width and height.",
    ));
    let make = move |property, value: f32| match property {
        Property::X => ConfigPatch::MoveImage {
            index: sel,
            x: value,
            y: iy,
        },
        Property::Y => ConfigPatch::MoveImage {
            index: sel,
            x: ix,
            y: value,
        },
        Property::Z => ConfigPatch::SetImageZ {
            index: sel,
            z: value as i32,
        },
        Property::Width => ConfigPatch::SetImageSize {
            index: sel,
            width: value,
            height: ih,
        },
        Property::Height => ConfigPatch::SetImageSize {
            index: sel,
            width: iw,
            height: value,
        },
        Property::Scale => ConfigPatch::SetImageScale {
            index: sel,
            scale: value,
        },
    };
    for (label, value, bounds, step, decimals, reset, property) in [
        (
            "X (px)",
            ix as f64,
            -20000.0..=20000.0,
            1.0,
            1,
            0.0,
            Property::X,
        ),
        (
            "Y (px)",
            iy as f64,
            -20000.0..=20000.0,
            1.0,
            1,
            0.0,
            Property::Y,
        ),
        (
            "Z (stack)",
            iz as f64,
            -100.0..=100.0,
            1.0,
            0,
            0.0,
            Property::Z,
        ),
        (
            "Width (px)",
            shown_width as f64,
            0.0..=16000.0_f64.max(shown_width as f64),
            1.0,
            0,
            fw,
            Property::Width,
        ),
        (
            "Height (px)",
            shown_height as f64,
            0.0..=16000.0_f64.max(shown_height as f64),
            1.0,
            0,
            fh,
            Property::Height,
        ),
        (
            "Scale (×)",
            shown_scale as f64,
            0.01..=10.0_f64.max(shown_scale as f64),
            0.1,
            2,
            1.0,
            Property::Scale,
        ),
    ] {
        col = col.push(image_spin_row(
            label,
            value,
            bounds,
            step,
            decimals,
            move |value| change(id, make(property, value as f32)),
            change(id, make(property, reset)),
        ));
    }
    col.spacing(8).width(Length::Fill).into()
}

#[derive(Clone, Copy)]
enum Property {
    X,
    Y,
    Z,
    Width,
    Height,
    Scale,
}
fn change(id: window::Id, patch: ConfigPatch) -> Plant {
    Plant::SettingPlot(SettingEvent::WallpaperPatch(id, patch))
}
