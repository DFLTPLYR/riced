//! Placement tick scheduling, independent of bar-window layout and input.
use super::{Plots, ResolvedWidget, Top};
use iced::window;
use std::time::{Duration, Instant};

pub(super) fn run_due(plots: &mut Plots, jobs: Vec<(window::Id, ResolvedWidget)>) -> bool {
    let now = Instant::now();
    plots.services.refresh_system();
    let gpu = crate::services::gpu::gpu_usage_percent();
    let due: Vec<_> = jobs
        .into_iter()
        .filter(|(_, resolved)| {
            plots.widget_last_run.get(&resolved.id).is_none_or(|last| {
                now.duration_since(*last) >= Duration::from_secs_f32(resolved.interval)
            }) || plots
                .popups
                .values()
                .any(|popup| popup.placement == resolved.id)
        })
        .collect();
    let mut changed = false;
    for (bar, resolved) in &due {
        let edited = Top::sync_script_state(plots, resolved);
        plots.widget_last_run.insert(resolved.id.clone(), now);
        let result = Top::render_lua_value(plots, resolved, gpu, *bar);
        let key = (*bar, resolved.id.clone());
        let before = (
            plots.widget_outputs.get(&key).cloned(),
            plots.widget_trees.get(&key).cloned(),
        );
        Top::ingest_render_value(plots, resolved, *bar, result);
        let after = (
            plots.widget_outputs.get(&key).cloned(),
            plots.widget_trees.get(&key).cloned(),
        );
        changed |= before != after || edited;
    }
    changed
}
