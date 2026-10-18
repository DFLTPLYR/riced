//! Host-owned service samplers and native caches, separate from window state.
use super::{ToplevelCache, WorkspaceCache};
use std::time::Instant;

#[derive(Debug)]
pub(crate) struct ServiceState {
    pub system: sysinfo::System,
    pub toplevels: ToplevelCache,
    pub workspaces: WorkspaceCache,
    pub(crate) started: Instant,
    pub(crate) sampled: f64,
    pub(crate) gpu: Option<f32>,
}

impl ServiceState {
    pub(crate) fn new() -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self {
            system,
            toplevels: ToplevelCache::spawn(),
            workspaces: WorkspaceCache::spawn(),
            started: Instant::now(),
            sampled: 0.0,
            gpu: super::gpu::gpu_usage_percent(),
        }
    }
    pub(crate) fn refresh_system(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.gpu = super::gpu::gpu_usage_percent();
        self.sampled = self.started.elapsed().as_secs_f64();
    }
    /// Monotonic seconds at the most recent sample. Widgets stopwatch this to
    /// append one history point per step regardless of host render cadence.
    pub(crate) fn sampled(&self) -> f64 {
        self.sampled
    }
    pub(crate) fn gpu(&self) -> Option<f32> {
        self.gpu
    }
}
