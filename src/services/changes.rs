//! A bounded, event-driven wakeup shared by service listener threads.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Default)]
pub(crate) struct Changes(Arc<State>);

#[derive(Debug, Default)]
struct State {
    pending: AtomicBool,
    wake: tokio::sync::Notify,
}

impl std::hash::Hash for Changes {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl Changes {
    pub(crate) fn notify(&self) {
        // Keep at most one UI message pending, even if the subscriber has
        // already consumed the wakeup and the UI is still busy rendering.
        if !self.0.pending.swap(true, Ordering::AcqRel) {
            self.0.wake.notify_one();
        }
    }

    pub(crate) async fn next(&self) {
        self.0.wake.notified().await;
    }

    /// Acknowledge before reading caches: a concurrent change either lands
    /// in those snapshots or schedules another delivery, never gets lost.
    pub(crate) fn acknowledge(&self) {
        self.0.pending.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::{FutureExt, executor::block_on};

    #[test]
    fn changes_before_subscription_and_bursts_until_acknowledgement_are_delivered_once() {
        let changes = Changes::default();
        changes.notify();
        changes.notify();
        block_on(changes.next());
        changes.notify();
        assert!(changes.next().now_or_never().is_none());
        changes.acknowledge();
        changes.notify();
        block_on(changes.next());
        changes.acknowledge();
        assert!(changes.next().now_or_never().is_none());
    }
}
