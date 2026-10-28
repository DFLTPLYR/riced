//! Shared motion clock and widget-list lifetime ownership.
use crate::ui::anim::AnimRuntime;
use std::time::Instant;

#[derive(Debug, Default)]
pub(crate) struct AnimationState {
    pub motion: AnimRuntime,
    pub widgets: super::screens::top::animation::WidgetLists,
}

impl AnimationState {
    pub(crate) fn forget_window(&mut self, id: iced::window::Id) {
        let bar_prefix = format!("{id:?}/");
        let popup_prefix = format!("popup:{id:?}/");
        self.widgets.forget_retirements(&bar_prefix);
        self.widgets.forget_retirements(&format!("popup:{id:?}"));
        let scopes: Vec<_> = self
            .widgets
            .keys()
            .filter(|scope| scope.starts_with(&bar_prefix) || scope.starts_with(&popup_prefix))
            .cloned()
            .collect();
        for scope in scopes {
            if let Some(mut list) = self.widgets.remove(&scope) {
                list.clear_all(&mut self.motion);
            }
        }
    }

    pub(crate) fn reset_widgets(&mut self) {
        for list in self.widgets.values_mut() {
            list.clear_all(&mut self.motion);
        }
        self.widgets.clear();
    }

    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        self.motion.tick_at(now);
        let mut released = false;
        for list in self.widgets.values_mut() {
            let had_ghosts = list.has_ghosts();
            list.sweep(&mut self.motion);
            released |= had_ghosts && !list.has_ghosts();
        }
        released
    }
}

/// Dispatch completions from the update loop, after all motion sets are swept.
pub(crate) fn complete(
    plots: &mut super::Plots,
    callbacks: Vec<crate::lua::callbacks::Callback>,
) -> iced::Task<super::Plant> {
    if callbacks.is_empty() {
        return iced::Task::none();
    }
    let mut commands = Vec::new();
    let mut ran = false;
    for callback in callbacks {
        let result = callback.invoke(
            &crate::services::ServiceCtx::from_plots(plots, None),
            |item| {
                super::screens::notification::callback_is_current(item, &plots.notification.queue)
            },
        );
        match result {
            Ok(Some(value)) => {
                ran = true;
                commands.push(super::actions::command_from_action(&value, plots));
            }
            Ok(None) => {}
            Err(error) => {
                ran = true;
                eprintln!("transition callback: {error}");
            }
        }
    }
    if !ran {
        return iced::Task::none();
    }
    plots.placements.last_run.clear();
    commands.push(super::screens::top::Top::handle_widget_tick(plots));
    commands.push(super::screens::popup::Popup::refresh_bodies(plots));
    commands.push(super::overlay::refresh(plots, true));
    iced::Task::batch(commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::listview::{Axis, ListView};
    use std::time::Duration;

    #[test]
    fn widget_ghost_release_requests_cleanup_only_after_exit() {
        let mut state = AnimationState::default();
        let mut list = ListView::new(32.0);
        let key = ("center".to_owned(), "7".to_owned());
        let duration = Duration::from_millis(250);
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        let node = crate::ui::decode::parse_node(
            &lua.load("return iced.text('notification')").eval().unwrap(),
        )
        .unwrap();
        list.update(
            &mut state.motion,
            duration,
            Axis::Vertical,
            &[],
            std::slice::from_ref(&key),
            &[],
        );
        list.update(
            &mut state.motion,
            duration,
            Axis::Vertical,
            std::slice::from_ref(&key),
            &[],
            &[(0, key.clone(), node)],
        );
        assert!(list.has_ghosts());
        state.widgets.insert("center".into(), list);
        let now = Instant::now();
        assert!(!state.advance(now));
        assert!(state.advance(now + Duration::from_secs(1)));
        assert!(!state.advance(now + Duration::from_secs(2)));
    }
}
