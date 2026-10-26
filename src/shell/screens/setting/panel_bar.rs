//! Lua bar-level Panel controls. Native code owns the snapshot, bounds,
//! bar identity, and every persisted command.
use crate::{
    config::ThemeConfig,
    shell::{
        BarEvent, Corner, Edge, Plant, SettingEvent, StyleEvent, TopEvent,
        screens::top::{SlotAlign, Top, TopLocal},
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

/// Which half of the bar page to render. The native arrangement preview and the
/// Lua placement editor sit between the two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Layout,
    Appearance,
}

#[derive(Debug, Clone, Copy)]
enum Handler {
    Length,
    Thickness,
    Slots,
    SlotPadding,
    SlotSpacing,
    Opacity(f32),
    Align(SlotAlign),
    SelectSlot(usize),
    Floating,
    Margin(Edge),
    Radius(Corner),
    Remove,
}

#[derive(Debug, Clone, Copy)]
enum Rule {
    /// Clamp to `min..=max` before dispatch.
    Number(Handler, f64, f64),
    Boolean(Handler),
    /// Button-style actions carry no value.
    Fixed(Handler),
}

impl Handler {
    fn plant(
        self,
        setting: window::Id,
        bar: window::Id,
        selected_slot: usize,
        value: Option<f64>,
        flag: bool,
    ) -> Plant {
        let number = value.unwrap_or_default() as f32;
        match self {
            Self::Length => Plant::TopPlot(TopEvent::Bar(BarEvent::Length(bar, number))),
            Self::Thickness => Plant::TopPlot(TopEvent::Bar(BarEvent::Thickness(bar, number))),
            Self::Slots => Plant::TopPlot(TopEvent::Bar(BarEvent::Slots(bar, number as u32))),
            Self::SlotPadding => Plant::TopPlot(TopEvent::Bar(BarEvent::SlotPadding(bar, number))),
            Self::SlotSpacing => Plant::TopPlot(TopEvent::Bar(BarEvent::SlotSpacing(bar, number))),
            Self::Opacity(step) => Plant::TopPlot(TopEvent::Style(StyleEvent::Opacity(bar, step))),
            Self::Align(align) => Plant::TopPlot(TopEvent::Bar(BarEvent::SlotAlign(
                bar,
                selected_slot,
                align,
            ))),
            Self::SelectSlot(position) => {
                Plant::SettingPlot(SettingEvent::SelectSlot(setting, position))
            }
            Self::Floating => Plant::TopPlot(TopEvent::Style(StyleEvent::Floating(bar, flag))),
            Self::Margin(edge) => Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(
                bar,
                edge,
                number as i32,
            ))),
            Self::Radius(corner) => {
                Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(bar, corner, number)))
            }
            Self::Remove => Plant::TopPlot(TopEvent::Remove(bar)),
        }
    }
}

fn dispatch(
    action: Action,
    rules: &HashMap<String, Rule>,
    setting: window::Id,
    bar: window::Id,
    selected_slot: usize,
) -> Plant {
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
            handler.plant(
                setting,
                bar,
                selected_slot,
                Some(f64::from(value).clamp(*min, *max)),
                false,
            )
        }
        (Rule::Boolean(handler), Some(ActionValue::Boolean(value))) => {
            handler.plant(setting, bar, selected_slot, None, value)
        }
        (Rule::Fixed(handler), None) => handler.plant(setting, bar, selected_slot, None, false),
        _ => Plant::Tend,
    }
}

struct Snapshot {
    host: Value,
    rules: HashMap<String, Rule>,
}

#[derive(Debug, Default)]
pub(super) struct Page {
    layout_input: Option<Value>,
    layout: Option<WidgetNode>,
    layout_rules: HashMap<String, Rule>,
    appearance_input: Option<Value>,
    appearance: Option<WidgetNode>,
    appearance_rules: HashMap<String, Rule>,
    error: Option<String>,
}

pub(super) struct Context<'a> {
    pub setting: window::Id,
    pub bar: window::Id,
    pub top: &'a Top,
    pub thick_max: f64,
    pub selected_slot: usize,
    pub theme: &'a ThemeConfig,
    pub defaults: &'a ComponentDefaults,
}

fn layout_snapshot(context: &Context<'_>) -> Snapshot {
    let local = &context.top.local;
    let length = local.length_pct.clamp(1.0, 100.0);
    let thickness_max = context.thick_max.max(1.0);
    let thickness = f64::from(local.thickness_px.clamp(1.0, thickness_max as f32));
    // `iced.slider` requires `min < max`; a 1px-thick display is degenerate.
    let slider_max = if thickness_max > 1.0 {
        thickness_max
    } else {
        thickness + 1.0
    };
    let mut rules = HashMap::new();
    rules.insert("length".into(), Rule::Number(Handler::Length, 1.0, 100.0));
    rules.insert(
        "thickness".into(),
        Rule::Number(Handler::Thickness, 1.0, slider_max),
    );
    let current_opacity = TopLocal::snap_opacity(local.opacity);
    let opacity_steps = TopLocal::OPACITY_STEPS
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let action = format!("opacity:{index}");
            rules.insert(action.clone(), Rule::Fixed(Handler::Opacity(value)));
            json!({"value": value, "action": action, "selected": value == current_opacity})
        })
        .collect::<Vec<_>>();
    let slots = local.slots.clamp(1, TopLocal::MAX_SLOTS);
    rules.insert(
        "slots".into(),
        Rule::Number(Handler::Slots, 1.0, f64::from(TopLocal::MAX_SLOTS)),
    );
    let slot_padding = local.slot_padding.clamp(0.0, TopLocal::MAX_SLOT_GAP);
    let slot_spacing = local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
    rules.insert(
        "slot_padding".into(),
        Rule::Number(Handler::SlotPadding, 0.0, 32.0),
    );
    rules.insert(
        "slot_spacing".into(),
        Rule::Number(Handler::SlotSpacing, 0.0, 32.0),
    );
    let count = slots as usize;
    let selected_slot = context.selected_slot.min(count.saturating_sub(1));
    let slots_picker = (0..count)
        .map(|position| {
            let action = format!("slot:{position}");
            rules.insert(action.clone(), Rule::Fixed(Handler::SelectSlot(position)));
            json!({"label": format!("Slot {}", position + 1), "action": action, "selected": position == selected_slot, "padding": 6})
        })
        .collect::<Vec<_>>();
    let current_align = local.align_at(selected_slot);
    let aligns = [SlotAlign::Start, SlotAlign::Center, SlotAlign::End]
        .into_iter()
        .map(|align| {
            let action = format!("align:{}", align.as_str());
            rules.insert(action.clone(), Rule::Fixed(Handler::Align(align)));
            json!({"label": align.as_str(), "action": action, "selected": align == current_align, "padding": 6})
        })
        .collect::<Vec<_>>();
    Snapshot {
        host: json!({
            "section": "layout",
            "length": length, "thickness": thickness, "thickness_max": slider_max,
            "opacity_steps": opacity_steps,
            "slots": slots, "max_slots": TopLocal::MAX_SLOTS,
            "slot_padding": slot_padding, "slot_spacing": slot_spacing,
            "slots_picker": slots_picker, "aligns": aligns,
        }),
        rules,
    }
}

fn appearance_snapshot(context: &Context<'_>) -> Snapshot {
    let local = &context.top.local;
    let mut rules = HashMap::new();
    rules.insert("floating".into(), Rule::Boolean(Handler::Floating));
    let margins = [
        (
            "Margin top (px)",
            "margin:top",
            Edge::Top,
            local.margins.top,
        ),
        (
            "Margin bottom (px)",
            "margin:bottom",
            Edge::Bottom,
            local.margins.bottom,
        ),
        (
            "Margin right (px)",
            "margin:right",
            Edge::Right,
            local.margins.right,
        ),
        (
            "Margin left (px)",
            "margin:left",
            Edge::Left,
            local.margins.left,
        ),
    ]
    .into_iter()
    .map(|(label, action, edge, value)| {
        rules.insert(
            action.into(),
            Rule::Number(Handler::Margin(edge), 0.0, 256.0),
        );
        json!({"label": label, "action": action, "value": value})
    })
    .collect::<Vec<_>>();
    let corners = [
        (
            "Round top-left",
            "radius:top_left",
            Corner::TopLeft,
            local.radius.top_left,
        ),
        (
            "Round top-right",
            "radius:top_right",
            Corner::TopRight,
            local.radius.top_right,
        ),
        (
            "Round bottom-left",
            "radius:bottom_left",
            Corner::BottomLeft,
            local.radius.bottom_left,
        ),
        (
            "Round bottom-right",
            "radius:bottom_right",
            Corner::BottomRight,
            local.radius.bottom_right,
        ),
    ]
    .into_iter()
    .map(|(label, action, corner, value)| {
        rules.insert(
            action.into(),
            Rule::Number(Handler::Radius(corner), 0.0, 32.0),
        );
        json!({"label": label, "action": action, "value": value})
    })
    .collect::<Vec<_>>();
    rules.insert("remove".into(), Rule::Fixed(Handler::Remove));
    Snapshot {
        host: json!({
            "section": "appearance",
            "floating": local.floating,
            "margins": margins,
            "corners": corners,
            "remove_label": format!("Remove {} bar", context.top.anchor_label()),
        }),
        rules,
    }
}

impl Page {
    fn render(
        &mut self,
        section: Section,
        snapshot: Snapshot,
        identity: &str,
        theme: &ThemeConfig,
        defaults: &ComponentDefaults,
    ) {
        let input = json!({"host": snapshot.host, "identity": identity, "source": super::sources::revision("panel_bar.lua"), "palette": crate::theme::lua_palette(theme), "defaults": defaults});
        let (slot_input, slot_node, slot_rules) = match section {
            Section::Layout => (
                &mut self.layout_input,
                &mut self.layout,
                &mut self.layout_rules,
            ),
            Section::Appearance => (
                &mut self.appearance_input,
                &mut self.appearance,
                &mut self.appearance_rules,
            ),
        };
        if slot_input.as_ref() == Some(&input) {
            return;
        }
        let result = super::sources::render("panel_bar.lua", &snapshot.host, theme, defaults, &[]);
        match result {
            Ok(node) => {
                *slot_node = Some(node);
                *slot_rules = snapshot.rules;
                self.error = None;
            }
            Err(error) => {
                // A failed render must not reuse another bar's controls.
                *slot_node = None;
                slot_rules.clear();
                crate::lua::error::report_once(
                    &mut self.error,
                    "Panel Settings: ",
                    error.to_string(),
                );
            }
        }
        *slot_input = Some(input);
    }

    pub(super) fn view(
        &mut self,
        section: Section,
        context: Context<'_>,
    ) -> Element<'static, Plant> {
        let Context {
            setting,
            bar,
            top,
            thick_max,
            selected_slot,
            theme,
            defaults,
        } = context;
        let snapshot = match section {
            Section::Layout => layout_snapshot(&Context {
                setting,
                bar,
                top,
                thick_max,
                selected_slot,
                theme,
                defaults,
            }),
            Section::Appearance => appearance_snapshot(&Context {
                setting,
                bar,
                top,
                thick_max,
                selected_slot,
                theme,
                defaults,
            }),
        };
        self.render(section, snapshot, &format!("{bar:?}"), theme, defaults);
        let (node, rules) = match section {
            Section::Layout => (self.layout.as_ref(), self.layout_rules.clone()),
            Section::Appearance => (self.appearance.as_ref(), self.appearance_rules.clone()),
        };
        let sender = std::sync::Arc::new(move |action| {
            dispatch(action, &rules, setting, bar, selected_slot)
        });
        match node
            .ok_or_else(|| "Panel controls unavailable".to_owned())
            .and_then(|node| crate::ui::build::build_node(node, 13.0, Some(sender)))
        {
            Ok(element) => element,
            Err(error) => iced::widget::text(error).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(slots: u32) -> Top {
        let mut top = Top::new();
        top.local.length_pct = 80.0;
        top.local.thickness_px = 40.0;
        top.local.slots = slots;
        top.local.slot_padding = 4.0;
        top.local.slot_spacing = 6.0;
        top.local.opacity = 0.5;
        top.local.floating = true;
        top.local.margins.top = 8;
        top.local.radius.top_left = 12.0;
        top.local.ensure_aligns();
        top
    }

    fn context<'a>(
        setting: window::Id,
        bar: window::Id,
        top: &'a Top,
        theme: &'a ThemeConfig,
        defaults: &'a ComponentDefaults,
    ) -> Context<'a> {
        Context {
            setting,
            bar,
            top,
            thick_max: 1080.0,
            selected_slot: 1,
            theme,
            defaults,
        }
    }

    fn collect<'a>(
        node: &'a WidgetNode,
        sliders: &mut Vec<&'a str>,
        buttons: &mut Vec<&'a str>,
        checkboxes: &mut Vec<&'a str>,
        spins: &mut Vec<&'a str>,
    ) {
        match node {
            WidgetNode::Slider { on_change, .. } => sliders.extend(on_change.as_deref()),
            WidgetNode::Button { action, .. } => buttons.push(action),
            WidgetNode::Checkbox { on_toggle, .. } => checkboxes.extend(on_toggle.as_deref()),
            WidgetNode::SpinBox { on_change, .. } => spins.extend(on_change.as_deref()),
            WidgetNode::Clickable { child, .. } => {
                collect(child, sliders, buttons, checkboxes, spins)
            }
            WidgetNode::Container { child, .. } | WidgetNode::Aligned { child, .. } => {
                collect(child, sliders, buttons, checkboxes, spins)
            }
            WidgetNode::Column { children, .. }
            | WidgetNode::Row { children, .. }
            | WidgetNode::Masonry { children, .. } => {
                for child in children {
                    collect(child, sliders, buttons, checkboxes, spins);
                }
            }
            _ => {}
        }
    }

    fn keys(node: &WidgetNode) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
        let (mut sliders, mut buttons, mut checkboxes, mut spins) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        collect(
            node,
            &mut sliders,
            &mut buttons,
            &mut checkboxes,
            &mut spins,
        );
        (
            sliders.into_iter().map(String::from).collect(),
            buttons.into_iter().map(String::from).collect(),
            checkboxes.into_iter().map(String::from).collect(),
            spins.into_iter().map(String::from).collect(),
        )
    }

    #[test]
    fn layout_controls_emit_clamped_bar_events() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let top = fixture(3);
        let (setting, bar) = (window::Id::unique(), window::Id::unique());
        let mut page = Page::default();
        let _element = page.view(
            Section::Layout,
            context(setting, bar, &top, &theme, &defaults),
        );
        assert!(page.error.is_none(), "{:?}", page.error);
        crate::ui::build::build_node(page.layout.as_ref().unwrap(), 13.0, None).unwrap();
        let (sliders, buttons, checkboxes, spins) = keys(page.layout.as_ref().unwrap());
        assert_eq!(
            sliders,
            ["length", "thickness", "slot_padding", "slot_spacing"]
        );
        assert_eq!(spins, ["slots"]);
        assert!(checkboxes.is_empty());
        assert!(buttons.contains(&"opacity:2".to_owned()));
        assert!(buttons.contains(&"slot:0".to_owned()));
        assert!(buttons.contains(&"align:center".to_owned()));
        let rules = page.layout_rules.clone();
        assert!(
            matches!(dispatch(Action::change("length".into(), ActionValue::Number(999.0)), &rules, setting, bar, 1), Plant::TopPlot(TopEvent::Bar(BarEvent::Length(id, 100.0))) if id == bar)
        );
        assert!(matches!(
            dispatch(
                Action::change("thickness".into(), ActionValue::Number(0.0)),
                &rules,
                setting,
                bar,
                1
            ),
            Plant::TopPlot(TopEvent::Bar(BarEvent::Thickness(_, 1.0)))
        ));
        assert!(matches!(
            dispatch(
                Action::change("slots".into(), ActionValue::Number(5.0)),
                &rules,
                setting,
                bar,
                1
            ),
            Plant::TopPlot(TopEvent::Bar(BarEvent::Slots(_, 5)))
        ));
        assert!(matches!(
            dispatch("opacity:2".into(), &rules, setting, bar, 1),
            Plant::TopPlot(TopEvent::Style(StyleEvent::Opacity(_, 0.5)))
        ));
        assert!(
            matches!(dispatch("slot:2".into(), &rules, setting, bar, 1), Plant::SettingPlot(SettingEvent::SelectSlot(id, 2)) if id == setting)
        );
        assert!(matches!(
            dispatch("align:end".into(), &rules, setting, bar, 1),
            Plant::TopPlot(TopEvent::Bar(BarEvent::SlotAlign(_, 1, SlotAlign::End)))
        ));
    }

    #[test]
    fn appearance_controls_toggle_floating_and_edit_edges_and_corners() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let floating = fixture(1);
        let (setting, bar) = (window::Id::unique(), window::Id::unique());
        let mut page = Page::default();
        let _element = page.view(
            Section::Appearance,
            context(setting, bar, &floating, &theme, &defaults),
        );
        assert!(page.error.is_none(), "{:?}", page.error);
        let (sliders, buttons, checkboxes, spins) = keys(page.appearance.as_ref().unwrap());
        assert_eq!(sliders.len(), 4);
        assert_eq!(checkboxes, ["floating"]);
        assert_eq!(
            spins,
            ["margin:top", "margin:bottom", "margin:right", "margin:left"]
        );
        assert_eq!(buttons, ["remove"]);
        let rules = page.appearance_rules.clone();
        assert!(matches!(
            dispatch(
                Action::change("floating".into(), ActionValue::Boolean(false)),
                &rules,
                setting,
                bar,
                0
            ),
            Plant::TopPlot(TopEvent::Style(StyleEvent::Floating(_, false)))
        ));
        assert!(matches!(
            dispatch(
                Action::change("margin:top".into(), ActionValue::Number(500.0)),
                &rules,
                setting,
                bar,
                0
            ),
            Plant::TopPlot(TopEvent::Style(StyleEvent::Margin(_, Edge::Top, 256)))
        ));
        assert!(matches!(
            dispatch(
                Action::change("radius:top_left".into(), ActionValue::Number(-4.0)),
                &rules,
                setting,
                bar,
                0
            ),
            Plant::TopPlot(TopEvent::Style(StyleEvent::Radius(_, Corner::TopLeft, 0.0)))
        ));
        assert!(
            matches!(dispatch("remove".into(), &rules, setting, bar, 0), Plant::TopPlot(TopEvent::Remove(id)) if id == bar)
        );

        let mut docked = fixture(1);
        docked.local.floating = false;
        page.view(
            Section::Appearance,
            context(setting, bar, &docked, &theme, &defaults),
        );
        let (_, _, _, spins) = keys(page.appearance.as_ref().unwrap());
        assert!(spins.is_empty(), "margins hide while the bar is docked");
    }

    #[test]
    fn wrong_types_unknown_keys_and_animation_frames_are_inert() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let top = fixture(1);
        let (setting, bar) = (window::Id::unique(), window::Id::unique());
        let mut page = Page::default();
        page.view(
            Section::Layout,
            context(setting, bar, &top, &theme, &defaults),
        );
        let rules = page.layout_rules.clone();
        assert!(matches!(
            dispatch(
                Action::change("length".into(), ActionValue::Text("5".into())),
                &rules,
                setting,
                bar,
                0
            ),
            Plant::Tend
        ));
        assert!(matches!(
            dispatch(
                Action::change("length".into(), ActionValue::Number(f32::NAN)),
                &rules,
                setting,
                bar,
                0
            ),
            Plant::Tend
        ));
        assert!(matches!(
            dispatch(
                Action::change("missing".into(), ActionValue::Number(1.0)),
                &rules,
                setting,
                bar,
                0
            ),
            Plant::Tend
        ));
        assert!(
            matches!(
                dispatch("length".into(), &rules, setting, bar, 0),
                Plant::Tend
            ),
            "sliders need a value"
        );
        assert!(matches!(
            dispatch(Action::animation_frame(), &rules, setting, bar, 0),
            Plant::Tend
        ));
    }

    #[test]
    fn selected_slot_and_rule_keys_follow_the_bar_snapshot() {
        let theme = ThemeConfig::default();
        let defaults = ComponentDefaults::default();
        let top = fixture(2);
        let (setting, bar) = (window::Id::unique(), window::Id::unique());
        let mut page = Page::default();
        page.view(
            Section::Layout,
            context(setting, bar, &top, &theme, &defaults),
        );
        let (_, buttons, _, _) = keys(page.layout.as_ref().unwrap());
        assert_eq!(
            buttons
                .iter()
                .filter(|key| key.starts_with("slot:"))
                .count(),
            2
        );
        // Re-selecting the same bar reuses the cached tree.
        page.layout = Some(WidgetNode::Spinner);
        page.view(
            Section::Layout,
            context(setting, bar, &top, &theme, &defaults),
        );
        assert!(matches!(page.layout, Some(WidgetNode::Spinner)));
        // A different bar snapshot replaces it.
        let other = fixture(4);
        let other_bar = window::Id::unique();
        page.view(
            Section::Layout,
            context(setting, other_bar, &other, &theme, &defaults),
        );
        assert!(!matches!(page.layout, Some(WidgetNode::Spinner)));
        assert!(page.error.is_none(), "{:?}", page.error);
    }
}
