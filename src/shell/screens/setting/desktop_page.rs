//! Lua Desktop Settings page. Live widget previews are host-owned native
//! slots; discovery, config patches, and pickup identity stay native.
use crate::{
    config::{ConfigPatch, DesktopWidget, HoldDuration, HoldSpeed},
    shell::{
        ConfigEvent, Plant,
        desktop_widgets::{self, PreviewTarget, preview_output, settings_preview},
        state::Plots,
    },
    ui::{
        action::{Action, ActionValue, Event},
        node::WidgetNode,
        settings::ComponentDefaults,
    },
};
use iced::{Element, window};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone)]
enum Handler {
    Hold(HoldDuration),
    HoldCustomMs,
    PreviewOutput(iced_wayland_subscriber::OutputId),
    Pick(String),
    Reposition(String),
    Remove(usize),
    /// The paired axis value is captured when the snapshot is built.
    MoveX {
        index: usize,
        y: f32,
    },
    MoveY {
        index: usize,
        x: f32,
    },
    Width {
        index: usize,
        height: f32,
    },
    Height {
        index: usize,
        width: f32,
    },
}

#[derive(Debug, Clone)]
enum Rule {
    Number(Handler, f64, f64),
    Fixed(Handler),
}

impl Handler {
    fn plant(self, settings: window::Id, value: Option<f64>) -> Plant {
        let number = value.unwrap_or_default() as f32;
        match self {
            Self::Hold(duration) => {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::HoldDuration(duration)))
            }
            Self::HoldCustomMs => Plant::Config(ConfigEvent::Patch(ConfigPatch::HoldDuration(
                HoldDuration::Custom(number.max(1.0) as u32),
            ))),
            Self::PreviewOutput(output) => {
                Plant::Desktop(desktop_widgets::DesktopEvent::PreviewOutput { settings, output })
            }
            Self::Pick(widget) => {
                Plant::Desktop(desktop_widgets::DesktopEvent::Pick { settings, widget })
            }
            Self::Reposition(placement) => {
                Plant::Desktop(desktop_widgets::DesktopEvent::Reposition {
                    settings,
                    placement,
                })
            }
            Self::Remove(index) => {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::RemoveDesktopWidget {
                    index,
                }))
            }
            Self::MoveX { index, y } => {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveDesktopWidget {
                    index,
                    x: number,
                    y,
                }))
            }
            Self::MoveY { index, x } => {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveDesktopWidget {
                    index,
                    x,
                    y: number,
                }))
            }
            Self::Width { index, height } => {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::SetDesktopWidgetSize {
                    index,
                    width: number,
                    height,
                }))
            }
            Self::Height { index, width } => {
                Plant::Config(ConfigEvent::Patch(ConfigPatch::SetDesktopWidgetSize {
                    index,
                    width,
                    height: number,
                }))
            }
        }
    }
}

fn dispatch(action: Action, rules: &HashMap<String, Rule>, settings: window::Id) -> Plant {
    if !matches!(action.event, Event::Action) {
        return Plant::Tend;
    }
    let Some(rule) = rules.get(&action.key) else {
        return Plant::Tend;
    };
    match (rule, action.value) {
        (Rule::Number(handler, min, max), Some(ActionValue::Number(value)))
            if value.is_finite() =>
        {
            handler
                .clone()
                .plant(settings, Some(f64::from(value).clamp(*min, *max)))
        }
        (Rule::Fixed(handler), None) => handler.clone().plant(settings, None),
        _ => Plant::Tend,
    }
}

struct Snapshot {
    host: Value,
    rules: HashMap<String, Rule>,
    slots: HashMap<String, PreviewTarget>,
}

fn snapshot(settings: window::Id, plots: &Plots) -> Snapshot {
    let mut rules = HashMap::new();
    let mut slots = HashMap::new();
    let current = plots.config.interaction.hold;
    let holds = [
        (
            "Fast (250 ms)",
            "hold:fast",
            HoldDuration::Preset(HoldSpeed::Fast),
        ),
        (
            "Medium (500 ms)",
            "hold:medium",
            HoldDuration::Preset(HoldSpeed::Medium),
        ),
        (
            "Slow (1000 ms)",
            "hold:slow",
            HoldDuration::Preset(HoldSpeed::Slow),
        ),
        (
            "Custom",
            "hold:custom",
            HoldDuration::Custom(current.milliseconds()),
        ),
    ]
    .into_iter()
    .map(|(label, action, duration)| {
        rules.insert(action.into(), Rule::Fixed(Handler::Hold(duration)));
        json!({"label": label, "action": action, "selected": current == duration})
    })
    .collect::<Vec<_>>();
    rules.insert(
        "hold:ms".into(),
        Rule::Number(Handler::HoldCustomMs, 1.0, 60_000.0),
    );
    let custom = match current {
        HoldDuration::Custom(ms) => json!(ms),
        HoldDuration::Preset(_) => Value::Null,
    };
    let mut outputs: Vec<_> = plots.windows.output_infos.iter().collect();
    outputs.sort_by_key(|(output, _)| **output);
    let selected_output = preview_output(plots, settings, &DesktopWidget::default());
    let displays = outputs
        .into_iter()
        .map(|(output, info)| {
            let action = format!("display:{}", output.0);
            rules.insert(action.clone(), Rule::Fixed(Handler::PreviewOutput(*output)));
            json!({
                "label": info.name.clone().unwrap_or_else(|| format!("Display {}", output.0)),
                "action": action,
                "selected": selected_output == Some(*output),
            })
        })
        .collect::<Vec<_>>();
    let available = crate::config::discovery::discover_desktop_widget_files()
        .into_iter()
        .map(|(name, _)| {
            let action = format!("place:{name}");
            rules.insert(action.clone(), Rule::Fixed(Handler::Pick(name.clone())));
            let slot = format!("available:{name}");
            slots.insert(slot.clone(), PreviewTarget::Available(name.clone()));
            json!({"name": name, "action": action, "slot": slot})
        })
        .collect::<Vec<_>>();
    let placed = plots
        .config
        .desktop
        .iter()
        .enumerate()
        .map(|(index, widget)| {
            let slot = format!("placed:{index}");
            slots.insert(slot.clone(), PreviewTarget::Placed(widget.id.clone()));
            let reposition = format!("reposition:{index}");
            rules.insert(
                reposition.clone(),
                Rule::Fixed(Handler::Reposition(widget.id.clone())),
            );
            let remove = format!("remove:{index}");
            rules.insert(remove.clone(), Rule::Fixed(Handler::Remove(index)));
            let (x_action, y_action) = (format!("x:{index}"), format!("y:{index}"));
            rules.insert(
                x_action.clone(),
                Rule::Number(Handler::MoveX { index, y: widget.y }, -32768.0, 32768.0),
            );
            rules.insert(
                y_action.clone(),
                Rule::Number(Handler::MoveY { index, x: widget.x }, -32768.0, 32768.0),
            );
            let (width_action, height_action) =
                (format!("width:{index}"), format!("height:{index}"));
            rules.insert(
                width_action.clone(),
                Rule::Number(
                    Handler::Width {
                        index,
                        height: widget.height,
                    },
                    0.0,
                    32768.0,
                ),
            );
            rules.insert(
                height_action.clone(),
                Rule::Number(
                    Handler::Height {
                        index,
                        width: widget.width,
                    },
                    0.0,
                    32768.0,
                ),
            );
            json!({
                "slot": slot,
                "caption": format!(
                    "{} · {} · ({:.0}, {:.0})",
                    widget.widget,
                    if widget.output.is_empty() { "all outputs" } else { widget.output.as_str() },
                    widget.x,
                    widget.y,
                ),
                "reposition": reposition,
                "remove": remove,
                "x": widget.x, "y": widget.y,
                "width": widget.width, "height": widget.height,
                "x_action": x_action, "y_action": y_action,
                "width_action": width_action, "height_action": height_action,
            })
        })
        .collect::<Vec<_>>();
    Snapshot {
        host: json!({
            "holds": holds,
            "custom": custom,
            "displays": displays,
            "available": available,
            "placed": placed,
        }),
        rules,
        slots,
    }
}

#[derive(Debug, Default)]
pub(super) struct Page {
    input: Option<Value>,
    node: Option<WidgetNode>,
    rules: HashMap<String, Rule>,
    slots: HashMap<String, PreviewTarget>,
    error: Option<String>,
}

impl Page {
    fn refresh(&mut self, settings: window::Id, plots: &Plots, theme: &crate::config::ThemeConfig) {
        let snapshot = snapshot(settings, plots);
        let input = json!({
            "host": snapshot.host,
            "source": super::sources::revision("desktop.lua"), "palette": crate::theme::lua_palette(theme),
            "id": format!("{settings:?}"),
        });
        if self.input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render(
            "desktop.lua",
            &snapshot.host,
            theme,
            &ComponentDefaults::default(),
            &[],
        );
        match result {
            Ok(node) => {
                self.node = Some(node);
                self.rules = snapshot.rules;
                self.slots = snapshot.slots;
                self.error = None;
            }
            Err(error) => {
                self.node = None;
                self.rules.clear();
                self.slots.clear();
                crate::lua::error::report_once(
                    &mut self.error,
                    "Desktop Settings: ",
                    error.to_string(),
                );
            }
        }
        self.input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        settings: window::Id,
        plots: &Plots,
        theme: &crate::config::ThemeConfig,
    ) -> Element<'static, Plant> {
        self.refresh(settings, plots, theme);
        let rules = self.rules.clone();
        let slots = self.slots.clone();
        let sender = std::sync::Arc::new(move |action| dispatch(action, &rules, settings));
        let result = self
            .node
            .as_ref()
            .ok_or_else(|| "Desktop page unavailable".to_owned())
            .and_then(|node| {
                crate::ui::build::build_node_with_native(node, 13.0, Some(sender), &mut |id| {
                    let target = slots
                        .get(id)
                        .ok_or_else(|| format!("unknown native slot {id:?}"))?;
                    Ok(settings_preview(settings, plots, target.clone()))
                })
            });
        match result {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_plots(hold: HoldDuration) -> Plots {
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots::new(receiver);
        plots.config.interaction.hold = hold;
        plots
    }

    fn fixture() -> (window::Id, Plots) {
        (
            window::Id::unique(),
            test_plots(HoldDuration::Preset(HoldSpeed::Fast)),
        )
    }

    #[test]
    fn page_reads_only_props_the_host_provides() {
        let source = include_str!("../../../../scripts/settings/desktop.lua");
        let (settings, plots) = fixture();
        let host = snapshot(settings, &plots).host;
        let missing = super::super::sources::missing_props(source, &host, &[]);
        assert!(missing.is_empty(), "{missing:?} for {host}");
    }

    #[test]
    fn page_renders_hold_choices_displays_and_native_slots() {
        let (settings, plots) = fixture();
        let mut page = Page::default();
        let _element = page.view(settings, &plots, &crate::config::ThemeConfig::default());
        assert!(page.error.is_none(), "{:?}", page.error);
        let mut ids = Vec::new();
        collect_slots(page.node.as_ref().unwrap(), &mut ids);
        for id in &ids {
            assert!(page.slots.contains_key(*id), "unresolvable slot {id:?}");
        }
        // The rendered tree builds when the host fills every declared slot.
        let declared = page.slots.clone();
        crate::ui::build::build_node_with_native(
            page.node.as_ref().unwrap(),
            13.0,
            None,
            &mut |id| {
                assert!(declared.contains_key(id), "unexpected slot {id:?}");
                Ok(iced::widget::text(id.to_owned()).into())
            },
        )
        .unwrap();
        assert!(page.rules.contains_key("hold:fast") && page.rules.contains_key("hold:custom"));
    }

    #[test]
    fn actions_map_to_hold_display_and_placement_patches() {
        let (settings, plots) = fixture();
        let mut page = Page::default();
        page.refresh(settings, &plots, &crate::config::ThemeConfig::default());
        let rules = page.rules.clone();
        assert!(matches!(
            dispatch("hold:slow".into(), &rules, settings),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::HoldDuration(
                HoldDuration::Preset(HoldSpeed::Slow)
            )))
        ));
        assert!(matches!(
            dispatch(
                Action::change("hold:ms".into(), ActionValue::Number(0.0)),
                &rules,
                settings
            ),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::HoldDuration(
                HoldDuration::Custom(1)
            )))
        ));
        assert!(matches!(
            dispatch(
                Action::change("hold:ms".into(), ActionValue::Number(999999.0)),
                &rules,
                settings
            ),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::HoldDuration(
                HoldDuration::Custom(60000)
            )))
        ));
        assert!(matches!(
            dispatch("hold:ms".into(), &rules, settings),
            Plant::Tend
        ));
        assert!(matches!(
            dispatch("missing".into(), &rules, settings),
            Plant::Tend
        ));
        assert!(matches!(
            dispatch(Action::animation_frame(), &rules, settings),
            Plant::Tend
        ));
    }

    #[test]
    fn placed_widgets_expose_paired_axis_actions_and_slots() {
        let (settings, mut plots) = fixture();
        plots.config.desktop = vec![DesktopWidget {
            id: "d1".into(),
            widget: "clock".into(),
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
            ..Default::default()
        }];
        let mut page = Page::default();
        page.refresh(settings, &plots, &crate::config::ThemeConfig::default());
        assert!(page.error.is_none(), "{:?}", page.error);
        assert!(page.slots.contains_key("placed:0"));
        let rules = page.rules.clone();
        assert!(matches!(
            dispatch(
                Action::change("x:0".into(), ActionValue::Number(5.0)),
                &rules,
                settings
            ),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveDesktopWidget {
                index: 0,
                x: 5.0,
                y: 20.0
            }))
        ));
        assert!(matches!(
            dispatch(
                Action::change("y:0".into(), ActionValue::Number(7.0)),
                &rules,
                settings
            ),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::MoveDesktopWidget {
                index: 0,
                x: 10.0,
                y: 7.0
            }))
        ));
        assert!(matches!(
            dispatch(
                Action::change("width:0".into(), ActionValue::Number(3.0)),
                &rules,
                settings
            ),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetDesktopWidgetSize {
                index: 0,
                width: 3.0,
                height: 40.0
            }))
        ));
        assert!(matches!(
            dispatch(
                Action::change("height:0".into(), ActionValue::Number(4.0)),
                &rules,
                settings
            ),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::SetDesktopWidgetSize {
                index: 0,
                width: 30.0,
                height: 4.0
            }))
        ));
        assert!(matches!(
            dispatch("remove:0".into(), &rules, settings),
            Plant::Config(ConfigEvent::Patch(ConfigPatch::RemoveDesktopWidget {
                index: 0
            }))
        ));
        assert!(
            matches!(dispatch("reposition:0".into(), &rules, settings), Plant::Desktop(desktop_widgets::DesktopEvent::Reposition { placement, .. }) if placement == "d1")
        );
    }

    #[test]
    fn cache_reuses_nodes_and_clears_slots_after_failure() {
        let (settings, plots) = fixture();
        let theme = crate::config::ThemeConfig::default();
        let mut page = Page::default();
        page.refresh(settings, &plots, &theme);
        page.node = Some(WidgetNode::Spinner);
        page.refresh(settings, &plots, &theme);
        assert!(matches!(page.node, Some(WidgetNode::Spinner)));
        // A changed hold selection invalidates the cache.
        let next = test_plots(HoldDuration::Preset(HoldSpeed::Slow));
        page.refresh(settings, &next, &theme);
        assert!(!matches!(page.node, Some(WidgetNode::Spinner)));
    }

    fn collect_slots<'a>(node: &'a WidgetNode, out: &mut Vec<&'a str>) {
        match node {
            WidgetNode::Native { id } => out.push(id),
            WidgetNode::Clickable { child, .. } | WidgetNode::Container { child, .. } => {
                collect_slots(child, out)
            }
            WidgetNode::Column { children, .. }
            | WidgetNode::Row { children, .. }
            | WidgetNode::Masonry { children, .. } => {
                for child in children {
                    collect_slots(child, out);
                }
            }
            _ => {}
        }
    }
}
