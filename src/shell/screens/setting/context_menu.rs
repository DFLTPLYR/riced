//! Source selection for Lua-owned context-menu presentation.
use super::{editors, section};
use crate::{
    config::{ComposableConfig, ComposableKind, ConfigPatch},
    shell::{ConfigEvent, Plant, state::Plots},
    ui::style::input_style,
};
use iced::{
    Element, Length,
    widget::{column, text_input},
};

pub(super) struct Context<'a> {
    pub config: &'a ComposableConfig,
}

impl<'a> From<&'a Plots> for Context<'a> {
    fn from(plots: &'a Plots) -> Self {
        Self {
            config: &plots.config.composable,
        }
    }
}

pub(super) fn view(context: Context<'_>) -> Element<'static, Plant> {
    column![
        section(
            "Menu surface",
            "Edit sizing and appearance in the Lua app.defaults table.",
            editor(&context, ComposableKind::ContextMenu)
        ),
        section(
            "Menu entries",
            "Edit padding and rounding in the Lua app.defaults table.",
            editor(&context, ComposableKind::ContextMenuItem)
        ),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

fn editor(context: &Context<'_>, kind: ComposableKind) -> Element<'static, Plant> {
    let source = text_input("component.lua", context.config.get(kind))
        .padding(6)
        .style(input_style)
        .on_input(move |src| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableSource(kind, src)))
        });
    editors::prop_row(
        "Source".into(),
        Some("Relative to components/; absolute paths also work. Changes reload live.".into()),
        source.into(),
        None,
    )
}
