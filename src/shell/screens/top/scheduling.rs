//! Placement tick scheduling, independent of bar-window layout and input.
use super::{ResolvedWidget, runtime::RuntimeContext};
use iced::window;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

fn due_jobs(
    jobs: Vec<(window::Id, ResolvedWidget)>,
    last_run: &HashMap<String, Instant>,
    open_popups: &HashSet<String>,
    now: Instant,
) -> Vec<(window::Id, ResolvedWidget)> {
    jobs.into_iter()
        .filter(|(_, resolved)| {
            last_run.get(&resolved.id).is_none_or(|last| {
                now.duration_since(*last) >= Duration::from_secs_f32(resolved.interval)
            }) || open_popups.contains(&resolved.id)
        })
        .collect()
}

pub(super) fn run_due(
    context: &mut RuntimeContext<'_>,
    jobs: Vec<(window::Id, ResolvedWidget)>,
    open_popups: &HashSet<String>,
    now: Instant,
) -> bool {
    let due = due_jobs(jobs, &context.placements.last_run, open_popups, now);
    let mut changed = false;
    for (bar, resolved) in &due {
        let edited = context.sync_revision(resolved);
        context.placements.last_run.insert(resolved.id.clone(), now);
        let result = context.render(resolved, *bar);
        let key = (*bar, resolved.id.clone());
        let before = (
            context.placements.outputs.get(&key).cloned(),
            context.placements.trees.get(&key).cloned(),
        );
        context.ingest(resolved, *bar, result);
        let after = (
            context.placements.outputs.get(&key).cloned(),
            context.placements.trees.get(&key).cloned(),
        );
        changed |= before != after || edited;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_selection_preserves_order_interval_boundaries_and_popup_forcing() {
        let now = Instant::now();
        let bar = window::Id::unique();
        let jobs = ["new", "waiting", "boundary", "popup", "boundary"]
            .map(|id| {
                (
                    bar,
                    ResolvedWidget {
                        id: id.into(),
                        name: "counter".into(),
                        path: "counter.lua".into(),
                        interval: 2.0,
                        size: 13.0,
                        props: HashMap::new(),
                    },
                )
            })
            .into();
        let last_run = HashMap::from([
            ("waiting".into(), now - Duration::from_secs(1)),
            ("boundary".into(), now - Duration::from_secs(2)),
            ("popup".into(), now),
        ]);
        let due = due_jobs(jobs, &last_run, &HashSet::from(["popup".into()]), now);
        assert_eq!(
            due.iter()
                .map(|(_, resolved)| resolved.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "boundary", "popup", "boundary"]
        );
    }
}
