//! Surface registries, compositor output identity, and idempotent detachment.
use super::screens::{Background, Popup, Setting, Top};
use iced::window;
use iced_wayland_subscriber::{OutputId, OutputInfo};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlotInfo {
    Setting,
    Background(OutputId),
    Top(OutputId),
    Popup(OutputId),
    Notification(OutputId),
}

#[derive(Debug, Default)]
pub(crate) struct WindowState {
    pub ids: HashMap<window::Id, PlotInfo>,
    pub tops: HashMap<window::Id, Top>,
    pub popups: HashMap<window::Id, Popup>,
    pub settings: HashMap<window::Id, Setting>,
    pub backgrounds: HashMap<OutputId, Background>,
    pub background_ids: HashMap<OutputId, window::Id>,
    pub output_infos: HashMap<OutputId, OutputInfo>,
    pub notifications: HashMap<OutputId, window::Id>,
}

impl WindowState {
    pub(crate) fn register_popup(&mut self, output: OutputId, popup: Popup) {
        let id = popup.win_id;
        self.ids.insert(id, PlotInfo::Popup(output));
        self.popups.insert(id, popup);
    }
    pub(crate) fn sentinel_bars(&self) -> Vec<window::Id> {
        self.ids
            .iter()
            .filter_map(|(id, info)| {
                matches!(info, PlotInfo::Top(output) if *output == OutputId(u32::MAX))
                    .then_some(*id)
            })
            .collect()
    }

    pub(crate) fn forget(&mut self, id: window::Id) -> Option<PlotInfo> {
        let info = self.ids.remove(&id);
        self.tops.remove(&id);
        self.popups.remove(&id);
        self.settings.remove(&id);
        let backgrounds: Vec<_> = self
            .background_ids
            .iter()
            .filter_map(|(output, window)| (*window == id).then_some(*output))
            .collect();
        for output in backgrounds {
            self.background_ids.remove(&output);
            self.backgrounds.remove(&output);
        }
        self.notifications.retain(|_, window| *window != id);
        info
    }

    pub(crate) fn on_output(&self, output: OutputId) -> Vec<window::Id> {
        self.ids
            .iter()
            .filter_map(|(id, info)| match info {
                PlotInfo::Background(owner)
                | PlotInfo::Top(owner)
                | PlotInfo::Popup(owner)
                | PlotInfo::Notification(owner)
                    if *owner == output =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_bar_and_background_surfaces_never_request_keyboard_focus() {
        use iced_exwlshell::reexport::{Anchor, KeyboardInteractivity};
        for anchor in [Anchor::Top, Anchor::Bottom, Anchor::Left, Anchor::Right] {
            let bar = Top::with_anchor(anchor);
            assert_eq!(
                bar.open(1, 100, 50).1.keyboard_interactivity,
                KeyboardInteractivity::None
            );
            assert_eq!(
                bar.open_active().1.keyboard_interactivity,
                KeyboardInteractivity::None
            );
        }
        assert_eq!(
            Background.open(1).1.keyboard_interactivity,
            KeyboardInteractivity::None
        );
    }

    #[test]
    fn output_membership_includes_every_surface_and_excludes_settings_and_other_outputs() {
        let output = OutputId(1);
        let mut state = WindowState::default();
        let owned = [
            PlotInfo::Top(output),
            PlotInfo::Popup(output),
            PlotInfo::Background(output),
            PlotInfo::Notification(output),
        ]
        .map(|kind| {
            let id = window::Id::unique();
            state.ids.insert(id, kind);
            id
        });
        let settings = window::Id::unique();
        state.ids.insert(settings, PlotInfo::Setting);
        let other = window::Id::unique();
        state.ids.insert(other, PlotInfo::Top(OutputId(2)));
        let mut found = state.on_output(output);
        found.sort();
        let mut expected = owned.to_vec();
        expected.sort();
        assert_eq!(found, expected);
        for id in found {
            assert!(state.forget(id).is_some());
            assert!(state.forget(id).is_none());
        }
        assert_eq!(state.ids.len(), 2);
        assert_eq!(state.ids[&settings], PlotInfo::Setting);
        assert_eq!(state.ids[&other], PlotInfo::Top(OutputId(2)));
    }

    #[test]
    fn late_close_does_not_detach_a_replacement_background() {
        let output = OutputId(1);
        let old = window::Id::unique();
        let replacement = window::Id::unique();
        let mut state = WindowState::default();
        state.ids.insert(old, PlotInfo::Background(output));
        state.ids.insert(replacement, PlotInfo::Background(output));
        state.background_ids.insert(output, replacement);
        state.backgrounds.insert(output, Background);
        state.forget(old);
        assert_eq!(state.background_ids[&output], replacement);
        assert!(state.backgrounds.contains_key(&output));
        state.forget(replacement);
        assert!(state.background_ids.is_empty());
        assert!(state.backgrounds.is_empty());
    }
}
