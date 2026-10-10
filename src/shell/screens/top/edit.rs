//! Live bar edits and persistence snapshots over explicit host inputs.
use super::{Top, TopLocal};
use crate::{
    config::{SlotWidgets, TopConfig, WidgetDefinition, WidgetPlacement},
    shell::{Plant, windows::PlotInfo},
};
use iced::{Task, window};
use iced_exwlshell::reexport::LayerSize;
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

pub(super) struct EditContext<'a> {
    pub bars: &'a mut HashMap<window::Id, Top>,
    pub identities: &'a HashMap<window::Id, PlotInfo>,
    pub outputs: &'a HashMap<OutputId, OutputInfo>,
    pub catalog: &'a [WidgetDefinition],
    pub saved: &'a mut Vec<TopConfig>,
}

pub(super) fn serialize_slots(
    catalog: &[WidgetDefinition],
    widgets: &[Vec<WidgetPlacement>],
) -> Vec<SlotWidgets> {
    widgets
        .iter()
        .map(|slot| {
            let placements: Vec<_> = slot
                .iter()
                .map(|placement| {
                    catalog
                        .iter()
                        .find(|definition| definition.name == placement.name)
                        .map(|definition| placement.materialized(&definition.defaults))
                        .unwrap_or_else(|| placement.clone())
                })
                .collect();
            SlotWidgets::from_placements(&placements)
        })
        .collect()
}

impl EditContext<'_> {
    pub fn widget_layout(
        &mut self,
        bar: window::Id,
        expected: &[Vec<WidgetPlacement>],
        mut widgets: Vec<Vec<WidgetPlacement>>,
    ) -> bool {
        for placement in widgets.iter_mut().flatten() {
            if placement.id.is_empty() {
                placement.id = crate::config::fresh_placement_id();
            }
        }
        let Some(top) = self.bars.get_mut(&bar) else {
            return false;
        };
        if !Top::valid_widget_layout(&top.local.widgets, expected, &widgets, self.catalog) {
            return false;
        }
        top.local.widgets = widgets;
        true
    }

    pub fn widget_prop(
        &mut self,
        bar: window::Id,
        id: &str,
        patch: &crate::shell::PlacementProp,
    ) -> bool {
        let Some(placement) = self.bars.get_mut(&bar).and_then(|top| {
            top.local
                .widgets
                .iter_mut()
                .flatten()
                .find(|placement| placement.id == id)
        }) else {
            return false;
        };
        Top::apply_prop_patch(placement, patch);
        true
    }

    pub fn change(
        &mut self,
        bar: window::Id,
        relayout: bool,
        change: impl FnOnce(&mut TopLocal, f32),
    ) -> (Task<Plant>, bool) {
        let max = self.max_thickness(bar);
        if let Some(top) = self.bars.get_mut(&bar) {
            change(&mut top.local, max);
        }
        let layout = if relayout {
            self.layout(bar)
        } else {
            Task::none()
        };
        (layout, self.persist(bar))
    }
    fn output(&self, bar: window::Id) -> Option<OutputId> {
        match self.identities.get(&bar) {
            Some(PlotInfo::Top(output)) => Some(*output),
            _ => None,
        }
    }

    pub fn max_thickness(&self, bar: window::Id) -> f32 {
        self.output(bar)
            .and_then(|output| self.outputs.get(&output))
            .map(|info| {
                let (_, _, width, height) = crate::shared::geometry::output_geometry(info);
                TopLocal::max_thickness(
                    width,
                    height,
                    self.bars.get(&bar).is_none_or(|top| top.is_horizontal()),
                )
                .max(1.0)
            })
            .unwrap_or(1080.0)
    }

    pub fn persist(&mut self, bar: window::Id) -> bool {
        let output = self
            .output(bar)
            .and_then(|output| self.outputs.get(&output))
            .and_then(|info| info.name.clone())
            .unwrap_or_default();
        let Some(top) = self.bars.get_mut(&bar) else {
            return false;
        };
        let local = &top.local;
        let entry = TopConfig {
            anchor: top.anchor_label().to_lowercase(),
            output,
            length: local.length_pct,
            thickness: local.thickness_px,
            slots: local.slots.clamp(1, TopLocal::MAX_SLOTS),
            aligns: local
                .aligns
                .iter()
                .map(|align| align.as_str().to_string())
                .collect(),
            widgets: serialize_slots(self.catalog, &local.widgets),
            slot_padding: local.slot_padding,
            slot_spacing: local.slot_spacing,
            opacity: TopLocal::snap_opacity(local.opacity),
            floating: local.floating,
            margin_top: local.margins.top,
            margin_right: local.margins.right,
            margin_bottom: local.margins.bottom,
            margin_left: local.margins.left,
            radius_top_left: local.radius.top_left,
            radius_top_right: local.radius.top_right,
            radius_bottom_left: local.radius.bottom_left,
            radius_bottom_right: local.radius.bottom_right,
        };
        if top.bar_index == usize::MAX || top.bar_index >= self.saved.len() {
            self.saved.push(entry);
            top.bar_index = self.saved.len() - 1;
        } else {
            self.saved[top.bar_index] = entry;
        }
        true
    }

    pub fn layout(&mut self, bar: window::Id) -> Task<Plant> {
        if let Some(top) = self.bars.get_mut(&bar) {
            top.local.length_pct = top.local.length_pct.clamp(1.0, 100.0);
        }
        let Some(output) = self.output(bar) else {
            return Task::none();
        };
        if output == OutputId(u32::MAX) {
            return Task::none();
        }
        let (width, height) = Top::output_size(self.outputs, output);
        let Some(top) = self.bars.get_mut(&bar) else {
            return Task::none();
        };
        let horizontal = top.is_horizontal();
        top.local.thickness_px = top.local.thickness_px.clamp(
            1.0,
            TopLocal::max_thickness(width, height, horizontal).max(1.0),
        );
        let (width, height) = top.local.px_size(width, height, horizontal);
        Task::batch([
            Task::done(Plant::LayoutChange {
                id: bar,
                anchor: top.anchor,
                size: LayerSize::px(width, height),
            }),
            Task::done(Plant::ExclusiveZoneChange {
                id: bar,
                zone_size: Top::exclusive_px(top, width, height),
            }),
            Task::done(Plant::MarginChange {
                id: bar,
                margin: (0, 0, 0, 0),
            }),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PropValue, SlotEntry, WidgetDefaults};

    #[test]
    fn live_edits_persist_one_entry_with_sparse_props_and_ignore_removed_bars() {
        let bar = window::Id::unique();
        let mut top = Top::new();
        top.local.widgets = vec![vec![WidgetPlacement {
            id: "clock-placement".into(),
            name: "clock".into(),
            props: HashMap::from([("label".into(), PropValue::Text("custom".into()))]),
            ..Default::default()
        }]];
        let mut bars = HashMap::from([(bar, top)]);
        let identities = HashMap::from([(bar, PlotInfo::Top(OutputId(u32::MAX)))]);
        let outputs = HashMap::new();
        let catalog = vec![WidgetDefinition {
            name: "clock".into(),
            file: "clock.lua".into(),
            defaults: WidgetDefaults {
                interval: 5.0,
                size: 20.0,
                props: HashMap::from([("inherited".into(), PropValue::Bool(true))]),
            },
            schema: HashMap::new(),
        }];
        let mut saved = Vec::new();
        let mut context = EditContext {
            bars: &mut bars,
            identities: &identities,
            outputs: &outputs,
            catalog: &catalog,
            saved: &mut saved,
        };
        let (_, persisted) = context.change(bar, true, |local, _| {
            local.length_pct = 150.0;
            local.thickness_px = 5000.0;
        });
        assert!(persisted);
        assert_eq!(context.bars[&bar].bar_index, 0);
        assert_eq!(context.saved[0].length, 100.0);
        // Sentinel surfaces retain their fallback geometry until output arrival.
        assert_eq!(context.saved[0].thickness, 5000.0);
        let SlotWidgets::Many(entries) = &context.saved[0].widgets[0] else {
            panic!("slot");
        };
        let SlotEntry::Full(placement) = &entries[0] else {
            panic!("placement");
        };
        assert_eq!(placement.interval, Some(5.0));
        assert_eq!(placement.size, Some(20.0));
        assert_eq!(placement.props.len(), 1);
        assert!(!placement.props.contains_key("inherited"));
        let (_, persisted) = context.change(bar, false, |local, _| local.opacity = 0.5);
        assert!(persisted);
        assert_eq!(context.saved.len(), 1);
        assert_eq!(context.saved[0].opacity, 0.5);
        let removed = window::Id::unique();
        assert!(
            !context
                .change(removed, false, |_, _| panic!("stale edit"))
                .1
        );
        assert_eq!(context.saved.len(), 1);
        assert!(!context.widget_prop(
            bar,
            "removed-placement",
            &crate::shell::PlacementProp::Size(Some(30.0))
        ));
    }
}
