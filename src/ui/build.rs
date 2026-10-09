//! Realize owned nodes into iced elements. Host actions stay opaque callbacks.
use super::{
    icons::{icon_bytes, rich_text},
    node::{NodeLength, WidgetNode},
};
use crate::{shell::Plant, theme};
use iced::{
    Element,
    widget::{Space, button, column, container, progress_bar, row, text},
};

pub(crate) fn estimate_height(node: &WidgetNode, size: f32) -> f32 {
    let base = size.max(1.0);
    let px = |length: &NodeLength| match length {
        NodeLength::Fixed(px) => *px,
        _ => base,
    };
    match node {
        WidgetNode::Text { size: own, .. } => own.unwrap_or(base).max(1.0) * 1.4,
        WidgetNode::Icon { .. } | WidgetNode::Spinner => base * 1.5,
        WidgetNode::Separator { height, .. } => height.max(1.0),
        WidgetNode::Button { padding, .. } => base * 1.4 + 2.0 * padding.unwrap_or(6.0).max(0.0),
        WidgetNode::Progress { height, .. } => height.as_ref().map(px).unwrap_or(12.0),
        WidgetNode::Space { height, .. } => px(height),
        WidgetNode::Image { height, .. } => px(height).max(base),
        WidgetNode::Row { children, .. } => children
            .iter()
            .map(|c| estimate_height(c, base))
            .fold(0.0, f32::max),
        WidgetNode::Column {
            children, spacing, ..
        } => {
            children
                .iter()
                .map(|c| estimate_height(c, base))
                .sum::<f32>()
                + spacing.max(0.0) * children.len().saturating_sub(1) as f32
        }
        WidgetNode::ListView {
            items,
            horizontal,
            spacing,
            ..
        } => {
            let heights = items.iter().map(|(_, c)| estimate_height(c, base));
            if *horizontal {
                heights.fold(0.0, f32::max)
            } else {
                heights.sum::<f32>() + spacing.max(0.0) * items.len().saturating_sub(1) as f32
            }
        }
        WidgetNode::Container { child, padding, .. } => {
            estimate_height(child, base) + 2.0 * padding.max(0.0)
        }
        WidgetNode::Scrollable { child, .. } => estimate_height(child, base),
    }
}
pub(crate) fn build_node(
    node: &WidgetNode,
    size: f32,
    button_msg: Option<&dyn Fn(String) -> Plant>,
) -> Result<Element<'static, Plant>, String> {
    build_node_opacity(node, size, button_msg, 1.0)
}
pub(crate) fn build_node_opacity(
    node: &WidgetNode,
    size: f32,
    button_msg: Option<&dyn Fn(String) -> Plant>,
    opacity: f32,
) -> Result<Element<'static, Plant>, String> {
    match node {
        WidgetNode::ListView {
            items,
            horizontal,
            spacing,
            width,
            height,
            ..
        } => {
            let children = items.iter().map(|(_, c)| c.clone()).collect();
            let layout = if *horizontal {
                WidgetNode::Row {
                    children,
                    spacing: *spacing,
                    width: width.clone(),
                    height: height.clone(),
                }
            } else {
                WidgetNode::Column {
                    children,
                    spacing: *spacing,
                    width: width.clone(),
                    height: height.clone(),
                }
            };
            build_node_opacity(&layout, size, button_msg, opacity)
        }
        WidgetNode::Container {
            child,
            width,
            height,
            padding,
            background,
            radius,
            border,
            border_width,
        } => {
            let background = background.map(|c| iced::Background::Color(c.scale_alpha(opacity)));
            let radius = *radius;
            let border = border.map(|c| c.scale_alpha(opacity));
            let border_width = *border_width;
            Ok(
                container(build_node_opacity(child, size, button_msg, opacity)?)
                    .width(width.clone().iced())
                    .height(height.clone().iced())
                    .padding(padding.max(0.0))
                    .style(move |_| iced::widget::container::Style {
                        background,
                        border: iced::Border {
                            color: border.unwrap_or_default(),
                            width: border_width,
                            radius: radius.into(),
                        },
                        ..Default::default()
                    })
                    .into(),
            )
        }
        WidgetNode::Scrollable {
            child,
            width,
            height,
        } => Ok(
            iced::widget::scrollable(build_node_opacity(child, size, button_msg, opacity)?)
                .width(width.clone().iced())
                .height(height.clone().iced())
                .into(),
        ),
        WidgetNode::Space { width, height } => Ok(Space::new()
            .width(width.clone().iced())
            .height(height.clone().iced())
            .into()),
        WidgetNode::Image {
            path,
            width,
            height,
        } => {
            let path = path.strip_prefix("file://").unwrap_or(path);
            match crate::config::decode_handle(std::path::Path::new(path)) {
                Some((_, _, handle)) => Ok(iced::widget::image::Image::new(handle)
                    .width(width.clone().iced())
                    .height(height.clone().iced())
                    .opacity(opacity)
                    .into()),
                None => Ok(Space::new()
                    .width(width.clone().iced())
                    .height(height.clone().iced())
                    .into()),
            }
        }
        WidgetNode::Text {
            content,
            size: own,
            width,
            height,
            color,
        } => {
            let mut t = text(content.clone()).size(own.unwrap_or(size).max(1.0));
            if let Some(w) = width {
                t = t.width(w.clone().iced());
            }
            if let Some(h) = height {
                t = t.height(h.clone().iced());
            }
            if color.is_some() || opacity < 1.0 {
                t = t.color(color.unwrap_or_else(theme::text).scale_alpha(opacity));
            }
            Ok(t.into())
        }
        WidgetNode::Icon { name, color } => {
            let base: Element<'static, Plant> = match icon_bytes(name) {
                Some(bytes) => lucide_iced::themed_icon(bytes, size.max(1.0)),
                None => text(format!("{{icon:{name}}}")).size(size.max(1.0)).into(),
            };
            if color.is_some() || opacity < 1.0 {
                let tint = color.unwrap_or_else(theme::text).scale_alpha(opacity);
                Ok(container(base)
                    .width(iced::Length::Shrink)
                    .height(iced::Length::Shrink)
                    .style(move |_| iced::widget::container::Style {
                        text_color: Some(tint),
                        ..Default::default()
                    })
                    .into())
            } else {
                Ok(base)
            }
        }
        WidgetNode::Row {
            children,
            spacing,
            width,
            height,
        } => {
            let mut row = row![]
                .spacing(spacing.max(0.0))
                .align_y(iced::Alignment::Center)
                .width(width.clone().iced())
                .height(height.clone().iced());
            for child in children {
                row = row.push(build_node_opacity(child, size, button_msg, opacity)?);
            }
            Ok(row.into())
        }
        WidgetNode::Column {
            children,
            spacing,
            width,
            height,
        } => {
            let mut column = column![]
                .spacing(spacing.max(0.0))
                .align_x(iced::Alignment::Center)
                .width(width.clone().iced())
                .height(height.clone().iced());
            for child in children {
                column = column.push(build_node_opacity(child, size, button_msg, opacity)?);
            }
            Ok(column.into())
        }
        WidgetNode::Button {
            label,
            action,
            width,
            height,
            padding,
            color,
            background,
            radius,
        } => {
            let mut item = button(rich_text(label.clone(), size, 4.0))
                .padding(padding.unwrap_or(6.0).max(0.0));
            let tint = *color;
            let custom = *background;
            let radius = radius.unwrap_or(theme::RADIUS);
            item = item.style(move |theme, status| {
                let mut style = if let Some(c) = tint {
                    theme::menu_button_tinted(radius, c)(theme, status)
                } else {
                    theme::menu_button(radius)(theme, status)
                };
                if matches!(status, iced::widget::button::Status::Active)
                    && let Some(bg) = custom
                {
                    style.background = Some(bg.into());
                }
                if opacity < 1.0 {
                    style.text_color = tint.unwrap_or(style.text_color).scale_alpha(opacity);
                    style.background = style.background.map(|bg| bg.scale_alpha(opacity));
                    style.border.color = style.border.color.scale_alpha(opacity);
                    style.shadow.color = style.shadow.color.scale_alpha(opacity);
                }
                style
            });
            if let Some(w) = width {
                item = item.width(match w {
                    NodeLength::Fixed(px) => iced::Length::Fixed(px.max(20.0)),
                    other => other.clone().iced(),
                });
            }
            if let Some(h) = height {
                item = item.height(h.clone().iced());
            }
            if let Some(make_msg) = button_msg {
                item = item.on_press(make_msg(action.clone()));
            }
            Ok(item.into())
        }
        WidgetNode::Progress {
            value,
            width,
            height,
            color,
            background,
        } => {
            let length = match width {
                NodeLength::Fixed(px) => iced::Length::Fixed(px.max(20.0)),
                other => other.clone().iced(),
            };
            let mut bar = progress_bar(0.0..=1.0, value.clamp(0.0, 1.0)).length(length);
            if let Some(h) = height {
                bar = bar.girth(h.clone().iced());
            }
            let fill = *color;
            let track = *background;
            bar = bar.style(move |theme| {
                let mut style = iced::widget::progress_bar::primary(theme);
                if let Some(fill) = fill {
                    style.bar = iced::Background::Color(fill);
                }
                if let Some(track) = track {
                    style.background = iced::Background::Color(track);
                }
                style.background = style.background.scale_alpha(opacity);
                style.bar = style.bar.scale_alpha(opacity);
                style.border.color = style.border.color.scale_alpha(opacity);
                style
            });
            Ok(bar.into())
        }
        WidgetNode::Spinner => Ok(lucide_iced::ThemedIcon::new(
            iced::advanced::svg::Handle::from_memory(lucide_iced::bytes::LOADER_CIRCLE),
            size.max(1.0) * 1.5,
        )
        .opacity(opacity)
        .into()),
        WidgetNode::Separator { height, color } => {
            let color = color.unwrap_or_else(theme::border_color);
            Ok(iced::widget::rule::horizontal(height.max(1.0))
                .style(move |_| iced::widget::rule::Style {
                    color: color.scale_alpha(opacity),
                    radius: 0.0.into(),
                    fill_mode: iced::widget::rule::FillMode::Full,
                    snap: true,
                })
                .into())
        }
    }
}
