//! Wallpaper page. The draft editor is Lua; the interactive map canvas is a
//! host-filled native slot and rendering has no access to unrelated domains.
use super::image_file_name;
use crate::ui::widgets::display_map::{MapLayer, MapView, images_layer, outputs_layer};
use crate::{
    config::{BackgroundImage, ConfigPatch, MonitorWallpaper, WallpaperFit, WallpaperMode},
    shell::{Plant, SettingEvent, state::Plots},
    theme,
    ui::{
        action::{Action, ActionValue},
        node::WidgetNode,
        settings::ComponentDefaults,
    },
};
use iced::{
    Element, Length,
    widget::{container, image::Handle, stack},
    window,
};
use iced_wayland_subscriber::{OutputId, OutputInfo};
use serde_json::{Value, json};
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

/// Numeric image properties. The sibling axis is captured when the snapshot
/// is built so a single spin box never overwrites the other coordinate.
#[derive(Debug, Clone, Copy)]
enum Axis {
    X { index: usize, y: f32 },
    Y { index: usize, x: f32 },
    Z { index: usize },
    Width { index: usize, height: f32 },
    Height { index: usize, width: f32 },
    Scale { index: usize },
}

impl Axis {
    fn patch(self, value: f32) -> ConfigPatch {
        match self {
            Self::X { index, y } => ConfigPatch::MoveImage { index, x: value, y },
            Self::Y { index, x } => ConfigPatch::MoveImage { index, x, y: value },
            Self::Z { index } => ConfigPatch::SetImageZ {
                index,
                z: value as i32,
            },
            Self::Width { index, height } => ConfigPatch::SetImageSize {
                index,
                width: value,
                height,
            },
            Self::Height { index, width } => ConfigPatch::SetImageSize {
                index,
                width,
                height: value,
            },
            Self::Scale { index } => ConfigPatch::SetImageScale {
                index,
                scale: value,
            },
        }
    }
}

#[derive(Debug, Clone)]
enum Handler {
    Patch(ConfigPatch),
    Event(SettingEvent),
}

#[derive(Debug, Clone)]
enum Rule {
    Number(Axis, f64, f64),
    Fixed(Handler),
}

fn dispatch(action: Action, rules: &HashMap<String, Rule>, id: window::Id) -> Plant {
    if !action.is_control_action() {
        return Plant::Tend;
    }
    let Some(rule) = rules.get(&action.key) else {
        return Plant::Tend;
    };
    let patch = match (rule, action.value) {
        (Rule::Number(axis, min, max), Some(ActionValue::Number(value))) if value.is_finite() => {
            Some(axis.patch(f64::from(value).clamp(*min, *max) as f32))
        }
        (Rule::Fixed(Handler::Patch(patch)), None) => {
            return Plant::SettingPlot(SettingEvent::WallpaperPatch(id, patch.clone()));
        }
        (Rule::Fixed(Handler::Event(event)), None) => return Plant::SettingPlot(event.clone()),
        _ => None,
    };
    match patch {
        Some(patch) => Plant::SettingPlot(SettingEvent::WallpaperPatch(id, patch)),
        None => Plant::Tend,
    }
}

struct Snapshot {
    host: Value,
    rules: HashMap<String, Rule>,
}

/// Everything the page needs besides the owning Settings window.
pub(super) struct Context<'a> {
    pub id: window::Id,
    pub plots: &'a Plots,
    pub draft: &'a crate::wallpaper::draft::Draft,
    pub map_view: MapView,
    pub selected_image: Option<usize>,
}

/// One cache per Settings window; the page re-renders only when its snapshot
/// changes, never on ordinary redraws.
#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<Value>,
    node: Option<WidgetNode>,
    rules: HashMap<String, Rule>,
    error: Option<String>,
}

fn snapshot(
    id: window::Id,
    draft: &crate::wallpaper::draft::Draft,
    selected_image: Option<usize>,
    outputs: &[(OutputId, crate::wallpaper::OutputSpec)],
) -> Snapshot {
    let mut rules = HashMap::new();
    let per_monitor = draft.recipe.mode == WallpaperMode::PerMonitor;
    let modes = [
        (WallpaperMode::FreeForm, "Free Form", "mode:freeform"),
        (WallpaperMode::PerMonitor, "Per Monitor", "mode:permonitor"),
    ]
    .into_iter()
    .map(|(mode, label, action)| {
        rules.insert(
            action.into(),
            Rule::Fixed(Handler::Patch(ConfigPatch::WallpaperMode(mode))),
        );
        json!({"label": label, "action": action, "selected": draft.recipe.mode == mode})
    })
    .collect::<Vec<_>>();
    // `Add wallpaper…` and `Apply` are inert when their action cannot run.
    let add = if per_monitor {
        rules.insert(
            "add".into(),
            Rule::Fixed(Handler::Patch(ConfigPatch::WallpaperMode(
                WallpaperMode::FreeForm,
            ))),
        );
        "add"
    } else {
        rules.insert(
            "add".into(),
            Rule::Fixed(Handler::Event(SettingEvent::PickWallpaper(id))),
        );
        "add"
    };
    rules.insert(
        "discard".into(),
        Rule::Fixed(Handler::Event(SettingEvent::DiscardWallpaper(id))),
    );
    let apply = if draft.loading || draft.applying {
        rules.insert(
            "apply".into(),
            Rule::Fixed(Handler::Patch(ConfigPatch::WallpaperMode(
                draft.recipe.mode,
            ))),
        );
        "apply"
    } else {
        rules.insert(
            "apply".into(),
            Rule::Fixed(Handler::Event(SettingEvent::ApplyWallpaper(id))),
        );
        "apply"
    };
    let selected_monitor = selected_monitor(draft, outputs);
    let monitors = outputs
        .iter()
        .map(|(_, spec)| {
            let action = format!("monitor:{}", spec.key);
            rules.insert(action.clone(), Rule::Fixed(Handler::Event(SettingEvent::SelectMonitor(id, spec.key.clone()))));
            json!({"key": spec.key, "action": action, "selected": selected_monitor.as_deref() == Some(spec.key.as_str())})
        })
        .collect::<Vec<_>>();
    let monitor = selected_monitor.as_deref().and_then(|selected| {
        let spec = outputs.iter().find(|(_, spec)| spec.key == selected).map(|(_, spec)| spec)?;
        let wallpaper = draft
            .recipe
            .monitor
            .iter()
            .find(|m| m.output == selected)
            .cloned()
            .unwrap_or(MonitorWallpaper { output: selected.to_owned(), ..Default::default() });
        rules.insert("pick-monitor".into(), Rule::Fixed(Handler::Event(SettingEvent::PickMonitorWallpaper(id, selected.to_owned()))));
        let mut cleared = wallpaper.clone();
        cleared.path.clear();
        rules.insert("clear-monitor".into(), Rule::Fixed(Handler::Patch(ConfigPatch::MonitorWallpaper(cleared))));
        let fits = [
            (WallpaperFit::Fit, "Fit", "fit:fit"),
            (WallpaperFit::Expand, "Expand", "fit:expand"),
            (WallpaperFit::Stretch, "Stretch", "fit:stretch"),
            (WallpaperFit::Center, "Center", "fit:center"),
        ]
        .into_iter()
        .map(|(fit, label, action)| {
            let mut next = wallpaper.clone();
            next.fit = fit;
            rules.insert(action.into(), Rule::Fixed(Handler::Patch(ConfigPatch::MonitorWallpaper(next))));
            json!({"label": label, "action": action, "selected": wallpaper.fit == fit})
        })
        .collect::<Vec<_>>();
        let disconnected = draft
            .recipe
            .monitor
            .iter()
            .filter(|m| !outputs.iter().any(|(_, spec)| spec.key == m.output))
            .count();
        Some(json!({
            "caption": format!("{selected} · {} × {} px", spec.pixels.0, spec.pixels.1),
            "path_label": if wallpaper.path.is_empty() { "No wallpaper assigned (black background)".to_owned() } else { wallpaper.path.clone() },
            "fits": fits,
            "disconnected_hint": format!("{disconnected} disconnected display assignment(s) retained."),
        }))
    });
    let images: Vec<_> = draft
        .recipe
        .image
        .iter()
        .enumerate()
        .map(|(index, image)| {
            let action = format!("select-image:{index}");
            rules.insert(action.clone(), Rule::Fixed(Handler::Event(SettingEvent::SelectImage(id, index))));
            json!({"label": format!("{} · {}", index + 1, image_file_name(image)), "action": action, "selected": false})
        })
        .collect::<Vec<_>>();
    let selected = selected_image
        .filter(|index| *index < draft.recipe.image.len())
        .unwrap_or(0);
    let image = draft.recipe.image.get(selected).map(|image| {
        let (fw, fh) = draft
            .sources
            .get(&image.local_path())
            .map(|source| (source.width as f32, source.height as f32))
            .unwrap_or((0.0, 0.0));
        let (shown_width, shown_height) = MapLayer::size_with_file(image, (fw, fh));
        let shown_scale = image.scale.max(0.01);
        rules.insert(
            "remove-image".into(),
            Rule::Fixed(Handler::Patch(ConfigPatch::RemoveImage { index: selected })),
        );
        let fields = [
            (
                "X (px)",
                f64::from(image.x),
                -20000.0,
                20000.0,
                1.0,
                1,
                0.0_f32,
                Axis::X {
                    index: selected,
                    y: image.y,
                },
            ),
            (
                "Y (px)",
                f64::from(image.y),
                -20000.0,
                20000.0,
                1.0,
                1,
                0.0_f32,
                Axis::Y {
                    index: selected,
                    x: image.x,
                },
            ),
            (
                "Z (stack)",
                f64::from(image.z),
                -100.0,
                100.0,
                1.0,
                0,
                0.0_f32,
                Axis::Z { index: selected },
            ),
            (
                "Width (px)",
                f64::from(shown_width),
                0.0,
                16000.0_f64.max(f64::from(shown_width)),
                1.0,
                0,
                fw,
                Axis::Width {
                    index: selected,
                    height: image.height,
                },
            ),
            (
                "Height (px)",
                f64::from(shown_height),
                0.0,
                16000.0_f64.max(f64::from(shown_height)),
                1.0,
                0,
                fh,
                Axis::Height {
                    index: selected,
                    width: image.width,
                },
            ),
            (
                "Scale (×)",
                f64::from(shown_scale),
                0.01,
                10.0_f64.max(f64::from(shown_scale)),
                0.1,
                2,
                1.0_f32,
                Axis::Scale { index: selected },
            ),
        ]
        .into_iter()
        .enumerate()
        .map(
            |(slot, (label, value, min, max, step, decimals, reset, axis))| {
                let action = format!("image:{slot}");
                let reset_action = format!("reset:{slot}");
                rules.insert(action.clone(), Rule::Number(axis, min, max));
                rules.insert(
                    reset_action.clone(),
                    Rule::Fixed(Handler::Patch(axis.patch(reset))),
                );
                json!({
                    "label": label, "value": value, "min": min, "max": max,
                    "step": step, "decimals": decimals,
                    "action": action, "reset": reset_action,
                })
            },
        )
        .collect::<Vec<_>>();
        json!({
            "caption": format!("Image {} · {}", selected + 1, image_file_name(image)),
            "resolution": format!("Source resolution: {fw:.0} × {fh:.0} px"),
            "fields": fields,
        })
    });
    let mut host = serde_json::Map::new();
    host.insert("modes".into(), json!(modes));
    host.insert("per_monitor".into(), json!(per_monitor));
    host.insert("heading_title".into(), json!("Display layout"));
    host.insert("heading_description".into(), json!(if per_monitor {
        "Select a display, choose its wallpaper and content fit, then Apply. Unassigned displays stay black."
    } else {
        "Arrange the original images in the preview. Apply saves lossless, native-resolution wallpapers for each display."
    }));
    host.insert("add".into(), json!(add));
    host.insert("apply".into(), json!(apply));
    host.insert(
        "apply_label".into(),
        json!(if draft.applying {
            "Applying…"
        } else {
            "Apply"
        }),
    );
    host.insert(
        "status".into(),
        json!(if draft.dirty() {
            "Unapplied changes"
        } else {
            "Applied layout"
        }),
    );
    host.insert("monitors".into(), json!(monitors));
    host.insert("images".into(), json!(images));
    host.insert("loading".into(), json!(draft.loading));
    if let Some(monitor) = monitor {
        host.insert("monitor".into(), monitor);
    }
    if let Some(image) = image {
        host.insert("image".into(), image);
    }
    if let Some(error) = &draft.error {
        host.insert("error".into(), json!(error));
    }
    Snapshot {
        host: Value::Object(host),
        rules,
    }
}

impl Page {
    fn refresh(
        &mut self,
        id: window::Id,
        draft: &crate::wallpaper::draft::Draft,
        selected_image: Option<usize>,
        outputs: &[(OutputId, crate::wallpaper::OutputSpec)],
        theme: &crate::config::ThemeConfig,
    ) {
        let snapshot = snapshot(id, draft, selected_image, outputs);
        let input = json!({
            "host": snapshot.host,
            "source": super::sources::revision("wallpaper.lua"), "palette": crate::theme::lua_palette(theme),
            "map": draft.revision,
            "assets": draft.assets_revision,
            "id": format!("{id:?}"),
        });
        if self.input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render(
            "wallpaper.lua",
            &snapshot.host,
            theme,
            &ComponentDefaults::default(),
            &[],
        );
        match result {
            Ok(node) => {
                self.node = Some(node);
                self.rules = snapshot.rules;
                self.error = None;
            }
            Err(error) => {
                self.node = None;
                self.rules.clear();
                crate::lua::error::report_once(
                    &mut self.error,
                    "Wallpaper Settings: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        context: Context<'_>,
        theme: &crate::config::ThemeConfig,
    ) -> Element<'static, Plant> {
        let Context {
            id,
            plots,
            draft,
            map_view,
            selected_image,
        } = context;
        let outputs = crate::wallpaper::outputs(&plots.windows.output_infos);
        self.refresh(id, draft, selected_image, &outputs, theme);
        let rules = self.rules.clone();
        let sender = std::sync::Arc::new(move |action| dispatch(action, &rules, id));
        let result = self
            .node
            .as_ref()
            .ok_or_else(|| "Wallpaper page unavailable".to_owned())
            .and_then(|node| {
                crate::ui::build::build_node_with_native(
                    node,
                    13.0,
                    Some(sender),
                    &mut |slot, _| {
                        let context = WallpaperContext {
                            images: &draft.recipe.image,
                            output_infos: &plots.windows.output_infos,
                            sources: &draft.sources,
                        };
                        if slot != "wallpaper-map" {
                            return Err(format!("unknown native slot {slot:?}"));
                        }
                        Ok(if draft.recipe.mode == WallpaperMode::PerMonitor {
                            monitor_grid(id, &context, draft, map_view)
                        } else {
                            grid(id, &context, map_view)
                        })
                    },
                )
            });
        match result {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }
}

fn monitor_grid(
    id: window::Id,
    context: &WallpaperContext<'_>,
    draft: &crate::wallpaper::draft::Draft,
    view: MapView,
) -> Element<'static, Plant> {
    let outputs = crate::wallpaper::outputs(context.output_infos);
    let selected = selected_monitor(draft, &outputs);
    let names: Vec<_> = outputs.iter().map(|(_, spec)| spec.key.clone()).collect();
    let rects: Vec<_> = outputs.iter().map(|(_, spec)| spec.rect).collect();
    let mut images = Vec::new();
    let mut handles = Vec::new();
    let mut sizes = Vec::new();
    for (_, spec) in &outputs {
        let wallpaper = draft.recipe.monitor.iter().find(|m| m.output == spec.key);
        let source = wallpaper.and_then(|m| {
            context.sources.get(
                &BackgroundImage {
                    path: m.path.clone(),
                    ..Default::default()
                }
                .local_path(),
            )
        });
        match (wallpaper, source) {
            (Some(wallpaper), Some(source)) => {
                images.push(crate::wallpaper::raster::monitor_image(
                    spec, wallpaper, source,
                ));
                handles.push(Some(source.handle.clone()));
                sizes.push(Some((source.width as f32, source.height as f32)));
            }
            _ => {
                images.push(BackgroundImage::default());
                handles.push(None);
                sizes.push(None);
            }
        }
    }
    let images_canvas = MapLayer::images(id, rects.clone(), images.clone(), handles.clone(), view)
        .with_monitors(names.clone(), selected.clone())
        .with_file_sizes(sizes.clone());
    let outputs_canvas = MapLayer::outputs(id, rects, images, handles, view)
        .with_monitors(names, selected)
        .with_file_sizes(sizes);
    container(stack![
        iced::widget::canvas(images_canvas)
            .width(Length::Fill)
            .height(Length::Fill),
        iced::widget::canvas(outputs_canvas)
            .width(Length::Fill)
            .height(Length::Fill)
    ])
    .style(theme::menu_box)
    .height(360)
    .width(Length::Fill)
    .clip(true)
    .into()
}

fn selected_monitor(
    draft: &crate::wallpaper::draft::Draft,
    outputs: &[(OutputId, crate::wallpaper::OutputSpec)],
) -> Option<String> {
    draft
        .selected_output
        .as_ref()
        .filter(|name| outputs.iter().any(|(_, spec)| &spec.key == *name))
        .cloned()
        .or_else(|| outputs.first().map(|(_, spec)| spec.key.clone()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{BackgroundConfig, BackgroundImage},
        wallpaper::{OutputSpec, draft::Draft},
    };

    /// Button action keys anywhere in a widget tree.
    fn buttons(node: &WidgetNode, out: &mut Vec<String>) {
        match node {
            WidgetNode::Button { action, .. } => out.push(action.clone()),
            WidgetNode::Column { children, .. }
            | WidgetNode::Row { children, .. }
            | WidgetNode::Stack { children, .. }
            | WidgetNode::Masonry { children, .. } => {
                for child in children {
                    buttons(child, out);
                }
            }
            WidgetNode::Container { child, .. }
            | WidgetNode::Scrollable { child, .. }
            | WidgetNode::Clickable { child, .. }
            | WidgetNode::Anchored { child, .. }
            | WidgetNode::Aligned { child, .. }
            | WidgetNode::Animated { child, .. } => buttons(child, out),
            _ => {}
        }
    }

    fn outputs() -> Vec<(OutputId, OutputSpec)> {
        vec![
            (
                OutputId(1),
                OutputSpec {
                    key: "DP-1".into(),
                    rect: (0.0, 0.0, 100.0, 100.0),
                    pixels: (100, 100),
                },
            ),
            (
                OutputId(2),
                OutputSpec {
                    key: "DP-2".into(),
                    rect: (100.0, 0.0, 100.0, 100.0),
                    pixels: (100, 100),
                },
            ),
        ]
    }

    fn per_monitor_base() -> BackgroundConfig {
        BackgroundConfig {
            mode: WallpaperMode::PerMonitor,
            monitor: vec![
                MonitorWallpaper {
                    output: "DP-1".into(),
                    path: "one.png".into(),
                    fit: WallpaperFit::Expand,
                },
                MonitorWallpaper {
                    output: "DP-2".into(),
                    path: "two.png".into(),
                    fit: WallpaperFit::Expand,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn per_monitor_actions_target_the_selected_output_and_preserve_other_assignments() {
        let id = window::Id::unique();
        let outputs = outputs();
        let base = per_monitor_base();
        for selected in [0, 1] {
            let mut draft = Draft::new(&base);
            draft.selected_output = Some(base.monitor[selected].output.clone());
            for (action, fit) in [
                ("fit:fit", WallpaperFit::Fit),
                ("fit:expand", WallpaperFit::Expand),
                ("fit:stretch", WallpaperFit::Stretch),
                ("fit:center", WallpaperFit::Center),
            ] {
                // The page re-snapshots on every render, so rebuild per action.
                let rules = snapshot(id, &draft, None, &outputs).rules;
                let Plant::SettingPlot(SettingEvent::WallpaperPatch(
                    target,
                    ConfigPatch::MonitorWallpaper(assignment),
                )) = dispatch(action.into(), &rules, id)
                else {
                    panic!("fit patch expected for {action}");
                };
                assert_eq!(target, id);
                assert_eq!(assignment.output, base.monitor[selected].output);
                assert_eq!(assignment.path, base.monitor[selected].path);
                assert_eq!(assignment.fit, fit);
                draft.patch(ConfigPatch::MonitorWallpaper(assignment));
                assert_eq!(
                    draft.recipe.monitor[1 - selected],
                    base.monitor[1 - selected]
                );
            }
            let rules = snapshot(id, &draft, None, &outputs).rules;
            assert!(matches!(
                dispatch("pick-monitor".into(), &rules, id),
                Plant::SettingPlot(SettingEvent::PickMonitorWallpaper(target, output))
                    if target == id && output == base.monitor[selected].output
            ));
            let Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::MonitorWallpaper(cleared),
            )) = dispatch("clear-monitor".into(), &rules, id)
            else {
                panic!("clear patch expected");
            };
            assert_eq!(cleared.output, base.monitor[selected].output);
            assert!(cleared.path.is_empty());
            assert_eq!(cleared.fit, WallpaperFit::Center);
            assert_eq!(draft.base, base, "edits must stay in the draft");
        }
    }

    #[test]
    fn free_form_image_fields_carry_the_sibling_axis_and_reset_defaults() {
        let id = window::Id::unique();
        let base = BackgroundConfig {
            mode: WallpaperMode::FreeForm,
            image: vec![BackgroundImage {
                path: "one.png".into(),
                x: 10.0,
                y: 20.0,
                z: 3,
                width: 30.0,
                height: 40.0,
                scale: 2.0,
            }],
            ..Default::default()
        };
        let draft = Draft::new(&base);
        let rules = snapshot(id, &draft, Some(0), &outputs()).rules;
        assert!(matches!(
            dispatch(
                Action::change("image:0".into(), ActionValue::Number(5.0)),
                &rules,
                id
            ),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::MoveImage {
                    index: 0,
                    x: 5.0,
                    y: 20.0
                }
            ))
        ));
        assert!(matches!(
            dispatch(
                Action::change("image:1".into(), ActionValue::Number(7.0)),
                &rules,
                id
            ),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::MoveImage {
                    index: 0,
                    x: 10.0,
                    y: 7.0
                }
            ))
        ));
        assert!(matches!(
            dispatch(
                Action::change("image:2".into(), ActionValue::Number(-9.0)),
                &rules,
                id
            ),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::SetImageZ { index: 0, z: -9 }
            ))
        ));
        assert!(matches!(
            dispatch(
                Action::change("image:5".into(), ActionValue::Number(5.0)),
                &rules,
                id
            ),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::SetImageScale {
                    index: 0,
                    scale: 5.0
                }
            ))
        ));
        assert!(matches!(
            dispatch("remove-image".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::RemoveImage { index: 0 }
            ))
        ));
        // Numeric resets use the source resolution / defaults, not zero.
        assert!(matches!(
            dispatch("reset:3".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::SetImageSize {
                    index: 0,
                    width: 0.0,
                    ..
                }
            ))
        ));
        assert!(matches!(
            dispatch("reset:5".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::SetImageScale {
                    index: 0,
                    scale: 1.0
                }
            ))
        ));
        assert!(matches!(
            dispatch("image:0".into(), &rules, id),
            Plant::Tend
        ));
        assert!(matches!(
            dispatch("missing".into(), &rules, id),
            Plant::Tend
        ));
    }

    #[test]
    fn modes_and_header_actions_map_to_existing_events() {
        let id = window::Id::unique();
        let base = BackgroundConfig::default();
        let mut draft = Draft::new(&base);
        // While the draft is loading, Apply must not start a second bake.
        let rules = snapshot(id, &draft, None, &outputs()).rules;
        assert!(!matches!(
            dispatch("apply".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::ApplyWallpaper(_))
        ));
        draft.loading = false;
        let rules = snapshot(id, &draft, None, &outputs()).rules;
        assert!(matches!(
            dispatch("mode:permonitor".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::WallpaperPatch(
                _,
                ConfigPatch::WallpaperMode(WallpaperMode::PerMonitor)
            ))
        ));
        assert!(matches!(
            dispatch("add".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::PickWallpaper(target)) if target == id
        ));
        assert!(matches!(
            dispatch("discard".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::DiscardWallpaper(target)) if target == id
        ));
        assert!(matches!(
            dispatch("apply".into(), &rules, id),
            Plant::SettingPlot(SettingEvent::ApplyWallpaper(target)) if target == id
        ));
    }

    #[test]
    fn page_renders_both_modes_with_a_host_filled_map_slot() {
        let theme = crate::config::ThemeConfig::default();
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let plots = Plots::new(receiver);
        let id = window::Id::unique();
        for base in [BackgroundConfig::default(), per_monitor_base()] {
            let draft = Draft::new(&base);
            let mut page = Page::default();
            let _element = page.view(
                Context {
                    id,
                    plots: &plots,
                    draft: &draft,
                    map_view: MapView::default(),
                    selected_image: None,
                },
                &theme,
            );
            assert!(page.error.is_none(), "{:?}", page.error);
            let node = page.node.as_ref().unwrap();
            let mut slots = Vec::new();
            collect_slots(node, &mut slots);
            assert_eq!(slots, ["wallpaper-map"]);
            crate::ui::build::build_node_with_native(node, 13.0, None, &mut |_, _| {
                Ok(iced::widget::text("map").into())
            })
            .unwrap();
        }
    }

    #[test]
    fn page_reads_only_props_the_host_provides() {
        // Static contract check: every `props.*` path the page reads must exist
        // in the snapshot whenever its parent does. This catches a page reading
        // a nested field the host keeps somewhere else, regardless of which
        // branch a render test happens to exercise.
        let source = include_str!("../../../../scripts/settings/wallpaper.lua");
        let id = window::Id::unique();
        let free_form = BackgroundConfig {
            image: vec![BackgroundImage {
                path: "one.png".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        for base in [BackgroundConfig::default(), free_form, per_monitor_base()] {
            let draft = Draft::new(&base);
            for selected in [None, Some(0)] {
                let host = snapshot(id, &draft, selected, &outputs()).host;
                let missing = super::super::sources::missing_props(source, &host, &[]);
                assert!(missing.is_empty(), "{missing:?} for {host}");
            }
        }
    }

    #[test]
    fn free_form_controls_render_for_the_selected_image() {
        // Regression: with one placed image the page reads that image's
        // placement fields. `page.view` needs a live widget library for the
        // preview, so this drives `refresh` with a real draft instead.
        let theme = crate::config::ThemeConfig::default();
        let id = window::Id::unique();
        let base = BackgroundConfig {
            image: vec![BackgroundImage {
                path: "one.png".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let draft = Draft::new(&base);
        let mut page = Page::default();
        page.refresh(id, &draft, None, &outputs(), &theme);
        assert!(page.error.is_none(), "{:?}", page.error);
        let mut keys = Vec::new();
        buttons(page.node.as_ref().unwrap(), &mut keys);
        // Six placement fields (X, Y, Z, width, height, scale) plus the image
        // controls prove the `props.image.fields` branch ran.
        for key in [
            "reset:0", "reset:1", "reset:2", "reset:3", "reset:4", "reset:5",
        ] {
            assert!(keys.iter().any(|entry| entry == key), "{key}: {keys:?}");
        }
        assert!(keys.iter().any(|entry| entry == "remove-image"), "{keys:?}");
        assert!(
            keys.iter().any(|entry| entry == "select-image:0"),
            "{keys:?}"
        );
        assert!(keys.iter().any(|entry| entry == "add"), "{keys:?}");
    }

    #[test]
    fn per_monitor_controls_render_for_a_selected_display() {
        // Regression: with a connected output the page reads the selected
        // monitor's content-fit choices. `page.view` derives outputs from the
        // live plots, so this drives `refresh` with real outputs instead.
        let theme = crate::config::ThemeConfig::default();
        let id = window::Id::unique();
        let draft = Draft::new(&per_monitor_base());
        let mut page = Page::default();
        page.refresh(id, &draft, None, &outputs(), &theme);
        assert!(page.error.is_none(), "{:?}", page.error);
        let mut keys = Vec::new();
        buttons(page.node.as_ref().unwrap(), &mut keys);
        for fit in ["fit:fit", "fit:expand", "fit:stretch", "fit:center"] {
            assert!(keys.iter().any(|key| key == fit), "{fit} missing: {keys:?}");
        }
        assert!(keys.iter().any(|key| key == "monitor:DP-1"));
        assert!(keys.iter().any(|key| key == "pick-monitor"));
    }

    fn collect_slots<'a>(node: &'a WidgetNode, out: &mut Vec<&'a str>) {
        match node {
            WidgetNode::Native { id, .. } => out.push(id),
            WidgetNode::Clickable { child, .. } | WidgetNode::Container { child, .. } => {
                collect_slots(child, out)
            }
            WidgetNode::Column { children, .. } | WidgetNode::Row { children, .. } => {
                for child in children {
                    collect_slots(child, out);
                }
            }
            _ => {}
        }
    }
}
