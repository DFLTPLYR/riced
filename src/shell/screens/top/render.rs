//! Read-only bar composition over cached placement views and animation state.
use super::{Top, TopLocal, build_anim_list, build_with_lists, lua_cell_text};
use crate::{
    config::WidgetDefinition, lua::widgets::WidgetState, shell::Plant,
    ui::widgets::panel_window::top_window,
};
use iced::{
    Element, Fill,
    widget::{column, container, row},
    window,
};

pub(super) struct BarViewContext<'a> {
    pub catalog: &'a [WidgetDefinition],
    pub placements: &'a WidgetState,
    pub motion: &'a aura_anim::core::runtime::MotionRuntime,
    pub lists: &'a super::animation::WidgetLists,
}

fn render_slot_widgets(
    bar: window::Id,
    slot: usize,
    placements: &[crate::config::WidgetPlacement],
    gap: f32,
    horizontal: bool,
    context: &BarViewContext<'_>,
) -> Element<'static, Plant> {
    use crate::{
        shell::{TopEvent, WidgetEvent},
        ui::{icons::rich_text, node::WidgetNode},
    };
    use iced::widget::{Space, mouse_area};
    let mut items = Vec::new();
    for placement in placements {
        if TopLocal::is_empty_widget(&placement.name) {
            continue;
        }
        let id = placement.id.clone();
        let area = |element: Element<'static, Plant>| -> Element<'static, Plant> {
            mouse_area(element)
                .on_press(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Pressed(
                    bar,
                    slot,
                    id.clone(),
                ))))
                .on_release(Plant::TopPlot(TopEvent::Widget(WidgetEvent::Released(
                    bar,
                    slot,
                    id.clone(),
                ))))
                .into()
        };
        if let Some(node) = context.placements.trees.get(&(bar, id.clone())) {
            let size = context
                .catalog
                .iter()
                .find(|definition| definition.name == placement.name)
                .map(|definition| placement.effective_size(&definition.defaults))
                .unwrap_or(crate::config::WidgetDefaults::default().size);
            let scope = format!("{bar:?}/{id}");
            let action_id = id.clone();
            let message = move |action| {
                Plant::TopPlot(TopEvent::Widget(WidgetEvent::CellAction(
                    bar,
                    action_id.clone(),
                    action,
                )))
            };
            let built = match node {
                WidgetNode::Row {
                    children,
                    spacing,
                    width,
                    height,
                }
                | WidgetNode::Column {
                    children,
                    spacing,
                    width,
                    height,
                } => build_anim_list(
                    &scope,
                    children,
                    *spacing,
                    width.clone(),
                    height.clone(),
                    matches!(node, WidgetNode::Row { .. }),
                    size,
                    &message,
                    context.motion,
                    context.lists,
                ),
                _ => build_with_lists(
                    node,
                    &scope,
                    size,
                    Some(&message),
                    context.motion,
                    context.lists,
                ),
            };
            if let Ok(element) = built {
                items.push(area(element));
            }
        } else if let Some((text, size)) =
            lua_cell_text(bar, placement, context.catalog, &context.placements.outputs)
        {
            items.push(area(rich_text(text, size, gap)));
        }
    }
    if items.is_empty() {
        return Space::new().into();
    }
    if horizontal {
        let mut cells = row![]
            .spacing(gap.max(0.0))
            .align_y(iced::Alignment::Center)
            .width(iced::Length::Shrink)
            .height(iced::Length::Shrink);
        for item in items {
            cells = cells.push(item);
        }
        cells.into()
    } else {
        let mut cells = column![]
            .spacing(gap.max(0.0))
            .align_x(iced::Alignment::Center)
            .width(iced::Length::Shrink)
            .height(iced::Length::Shrink);
        for item in items {
            cells = cells.push(item);
        }
        cells.into()
    }
}

pub(super) fn view<'a>(
    top: &'a Top,
    id: window::Id,
    context: BarViewContext<'_>,
) -> Element<'a, Plant> {
    let count = top.local.slots.clamp(1, TopLocal::MAX_SLOTS) as usize;
    let gap = top.local.slot_spacing.clamp(0.0, TopLocal::MAX_SLOT_GAP);
    let padding = top.local.slot_padding.clamp(0.0, TopLocal::MAX_SLOT_GAP);
    let horizontal = top.is_horizontal();
    let cell = |slot| {
        let body = render_slot_widgets(
            id,
            slot,
            top.local.widgets_at(slot),
            gap,
            horizontal,
            &context,
        );
        let (x, y) = top.local.align_at(slot).for_bar(horizontal);
        container(body)
            .width(Fill)
            .height(Fill)
            .align_x(x)
            .align_y(y)
            .padding(padding)
    };
    let content: Element<'_, Plant> = if horizontal {
        let mut cells = row![].width(Fill).height(Fill).spacing(gap);
        for slot in 0..count {
            cells = cells.push(cell(slot));
        }
        cells.into()
    } else {
        let mut cells = column![].width(Fill).height(Fill).spacing(gap);
        for slot in 0..count {
            cells = cells.push(cell(slot));
        }
        cells.into()
    };
    let radius = top.local.radius;
    let opacity = TopLocal::snap_opacity(top.local.opacity);
    let margins = top.local.margins;
    let padding = if top.local.floating {
        iced::Padding {
            top: margins.top.max(0) as f32,
            right: margins.right.max(0) as f32,
            bottom: margins.bottom.max(0) as f32,
            left: margins.left.max(0) as f32,
        }
    } else {
        iced::Padding::ZERO
    };
    top_window(id)
        .padding(padding)
        .content(
            container(content)
                .width(Fill)
                .height(Fill)
                .center_x(Fill)
                .center_y(Fill)
                .style(move |theme: &iced::Theme| {
                    let mut style = crate::theme::bar(theme);
                    if let Some(iced::Background::Color(color)) = style.background {
                        style.background = Some(iced::Background::Color(iced::Color {
                            a: color.a * opacity,
                            ..color
                        }));
                    }
                    style.border.radius = iced::border::Radius {
                        top_left: radius.top_left,
                        top_right: radius.top_right,
                        bottom_right: radius.bottom_right,
                        bottom_left: radius.bottom_left,
                    };
                    style
                }),
        )
        .into()
}
