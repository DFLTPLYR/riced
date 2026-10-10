//! Host-owned service samplers and native caches, separate from window state.
use super::{ToplevelCache, WorkspaceCache};

#[derive(Debug)]
pub(crate) struct ServiceState {
    pub system: sysinfo::System,
    pub toplevels: ToplevelCache,
    pub workspaces: WorkspaceCache,
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
        }
    }
    pub(crate) fn refresh_system(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
    }
}
