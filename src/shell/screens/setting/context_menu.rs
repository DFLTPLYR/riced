//! Context-menu page with explicit config, palette and chrome-runtime inputs.
use super::{editors, section};
use crate::{
    config::{ComposableConfig, ComposableKind, ConfigPatch, PropValue, ThemeConfig},
    lua::composable::ComposableRuntime,
    shell::{ConfigEvent, Plant, state::Plots},
    ui::style::input_style,
};
use iced::{
    Element, Length,
    widget::{column, text_input},
};
use std::{cell::RefCell, rc::Rc, time::SystemTime};

pub(super) struct Context<'a> {
    pub config: &'a ComposableConfig,
    pub theme: &'a ThemeConfig,
    pub runtime: &'a RefCell<ComposableRuntime>,
    pub revision: Option<SystemTime>,
}

impl<'a> From<&'a Plots> for Context<'a> {
    fn from(plots: &'a Plots) -> Self {
        Self {
            config: &plots.config.composable,
            theme: &plots.config.theme,
            runtime: &plots.composable_runtime,
            revision: plots.components_mtime,
        }
    }
}

pub(super) fn view(context: Context<'_>) -> Element<'static, Plant> {
    column![
        section(
            "Menu surface",
            "Lua supplies sizing and appearance; Rust supplies the entries.",
            editor(&context, ComposableKind::ContextMenu)
        ),
        section(
            "Menu entries",
            "Each entry is rendered by this Lua app with label/action host props.",
            editor(&context, ComposableKind::ContextMenuItem)
        ),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

fn editor(context: &Context<'_>, kind: ComposableKind) -> Element<'static, Plant> {
    let config = context.config.get(kind);
    let host = match kind {
        ComposableKind::ContextMenu => {
            crate::shell::screens::background::context_menu_host(&context.config.context_menu_item)
        }
        _ => serde_json::json!({"label": "Preview", "action": "preview"}),
    };
    let rendered = context.runtime.borrow_mut().render(
        &format!("settings/{kind:?}"),
        config,
        host,
        context.theme,
        context.revision,
    );
    let mut props = config.props.clone();
    if let Some(rendered) = rendered
        && let Some(values) = rendered.props.as_object()
    {
        for (key, value) in values {
            if ["label", "action", "_item_revision"].contains(&key.as_str()) {
                continue;
            }
            if let Ok(value) = serde_json::from_value::<PropValue>(value.clone()) {
                props.entry(key.clone()).or_insert(value);
            }
        }
    }
    let source = text_input("component.lua", &config.src)
        .padding(6)
        .style(input_style)
        .on_input(move |src| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableSource(kind, src)))
        });
    let mut col = column![editors::prop_row(
        "Source".into(),
        Some("Relative to components/; absolute paths also work.".into()),
        source.into(),
        None
    )]
    .spacing(10);
    for (key, value) in props {
        let reset = config.props.contains_key(&key).then(|| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableProp(
                kind,
                key.clone(),
                None,
            )))
        });
        let patch_key = key.clone();
        let patch = Rc::new(move |value| {
            Plant::Config(ConfigEvent::Patch(ConfigPatch::ComposableProp(
                kind,
                patch_key.clone(),
                Some(value),
            )))
        });
        let control = editors::property_control(
            editors::PropertyControl {
                label: key.clone(),
                kind: value.kind().into(),
                value: Some(value),
                min: if key == "width" || key == "height" {
                    1.0
                } else {
                    0.0
                },
                max: 1_000_000.0,
                choices: Vec::new(),
                widget_layout: false,
            },
            patch,
        );
        let subtitle = if key == "height" && matches!(kind, ComposableKind::ContextMenu) {
            "Fixed height when auto_sizing is off. Reset inherits the Lua default."
        } else {
            "Reset clears the override and inherits the Lua default."
        };
        col = col.push(editors::prop_row(
            key,
            Some(subtitle.into()),
            control,
            reset,
        ));
    }
    col.width(Length::Fill).into()
}
