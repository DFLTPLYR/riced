//! Build service input snapshots at the host boundary, not inside services.
use super::state::Plots;
use crate::services::ServiceCtx;
impl<'a> ServiceCtx<'a> {
    pub fn from_plots(plots: &'a Plots, gpu: Option<f32>) -> Self {
        Self {
            sys: &plots.services.system,
            gpu,
            theme: &plots.config.theme,
            outputs: &plots.output_infos,
            notifications: &plots.notifications,
            toplevels: &plots.services.toplevels,
            workspaces: &plots.services.workspaces,
        }
    }
}
