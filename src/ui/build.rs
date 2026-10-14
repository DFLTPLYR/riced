//! Realize owned nodes into iced elements. Host actions stay opaque callbacks.
use super::action::{Action, ActionValue, Sender};
use super::{
    icons::{icon_bytes, rich_text_for as rich_text},
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
        WidgetNode::Checkbox { size, .. }
        | WidgetNode::Radio { size, .. }
        | WidgetNode::Toggler { size, .. } => size.unwrap_or(base).max(base * 1.4),
        WidgetNode::Slider { height, .. } => height.unwrap_or(22.0),
        WidgetNode::TextInput { size, padding, .. }
        | WidgetNode::PickList { size, padding, .. } => {
            size.unwrap_or(base) * 1.4 + padding.unwrap_or(6.0) * 2.0
        }
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
    button_msg: Option<Sender>,
) -> Result<Element<'static, Plant>, String> {
    build_node_opacity(node, size, button_msg, 1.0)
}
pub(crate) fn build_node_opacity(
    node: &WidgetNode,
    size: f32,
    button_msg: Option<Sender>,
    opacity: f32,
) -> Result<Element<'static, Plant>, String> {
    let interactive = button_msg.is_some();
    realize(
        node,
        size,
        button_msg.unwrap_or_else(|| std::sync::Arc::new(|_| Plant::Tend)),
        interactive,
        opacity,
    )
}

/// Realize a description for any host message type. The owned sender captures
/// routing identity; native control callbacks never borrow a Lua VM or host.
pub(crate) fn realize<M: Clone + 'static>(
    node: &WidgetNode,
    size: f32,
    button_msg: Sender<M>,
    interactive: bool,
    opacity: f32,
) -> Result<Element<'static, M>, String> {
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
            realize(&layout, size, button_msg, interactive, opacity)
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
            let content = container(realize(child, size, button_msg, interactive, opacity)?);
            let hints = iced::advanced::Widget::size(&content);
            Ok(content
                .width(width.resolve(hints.width))
                .height(height.resolve(hints.height))
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
                .into())
        }
        WidgetNode::Scrollable {
            child,
            width,
            height,
        } => {
            let content =
                iced::widget::scrollable(realize(child, size, button_msg, interactive, opacity)?);
            let hints = iced::advanced::Widget::size(&content);
            Ok(content
                .width(width.resolve(hints.width))
                .height(height.resolve(hints.height))
                .into())
        }
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
            match super::assets::image(std::path::Path::new(path)) {
                Some(handle) => Ok(iced::widget::image::Image::new(handle)
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
            wrapping,
            color,
        } => {
            let mut t = text(content.clone()).size(own.unwrap_or(size).max(1.0));
            if let Some(w) = width {
                t = t.width(w.clone().iced());
            }
            if let Some(h) = height {
                t = t.height(h.clone().iced());
            }
            if let Some(wrapping) = wrapping {
                t = t.wrapping(*wrapping);
            }
            if color.is_some() || opacity < 1.0 {
                t = t.color(color.unwrap_or_else(theme::text).scale_alpha(opacity));
            }
            Ok(t.into())
        }
        WidgetNode::Icon { name, color } => {
            let base: Element<'static, M> = match icon_bytes(name) {
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
                .align_y(iced::Alignment::Center);
            for child in children {
                row = row.push(realize(
                    child,
                    size,
                    button_msg.clone(),
                    interactive,
                    opacity,
                )?);
            }
            let hints = iced::advanced::Widget::size(&row);
            Ok(row
                .width(width.resolve(hints.width))
                .height(height.resolve(hints.height))
                .into())
        }
        WidgetNode::Column {
            children,
            spacing,
            width,
            height,
        } => {
            let mut column = column![]
                .spacing(spacing.max(0.0))
                .align_x(iced::Alignment::Center);
            for child in children {
                column = column.push(realize(
                    child,
                    size,
                    button_msg.clone(),
                    interactive,
                    opacity,
                )?);
            }
            let hints = iced::advanced::Widget::size(&column);
            Ok(column
                .width(width.resolve(hints.width))
                .height(height.resolve(hints.height))
                .into())
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
            if interactive {
                item = item.on_press(button_msg(action.clone().into()));
            }
            Ok(item.into())
        }
        WidgetNode::Checkbox {
            label,
            checked,
            on_toggle,
            width,
            size: own,
            spacing,
        } => {
            let mut item = iced::widget::checkbox(*checked)
                .label(label.clone())
                .spacing(*spacing);
            if opacity < 1.0 {
                item = item.style(move |theme, status| {
                    let mut style = iced::widget::checkbox::primary(theme, status);
                    style.background = style.background.scale_alpha(opacity);
                    style.icon_color = style.icon_color.scale_alpha(opacity);
                    style.border.color = style.border.color.scale_alpha(opacity);
                    style.text_color = Some(
                        style
                            .text_color
                            .unwrap_or_else(crate::theme::text)
                            .scale_alpha(opacity),
                    );
                    style
                });
            }
            if let Some(width) = width {
                item = item.width(width.clone().iced());
            }
            if let Some(size) = own {
                item = item.size(*size);
            }
            if interactive && let Some(key) = on_toggle {
                let key = key.clone();
                item = item.on_toggle(move |value| {
                    button_msg(Action::change(key.clone(), ActionValue::Boolean(value)))
                });
            }
            Ok(item.into())
        }
        WidgetNode::Toggler {
            label,
            enabled,
            on_toggle,
            width,
            size: own,
            spacing,
        } => {
            let mut item = iced::widget::toggler(*enabled)
                .label(label.clone())
                .spacing(*spacing);
            if opacity < 1.0 {
                item = item.style(move |theme, status| {
                    let mut style = iced::widget::toggler::default(theme, status);
                    style.background = style.background.scale_alpha(opacity);
                    style.foreground = style.foreground.scale_alpha(opacity);
                    style.background_border_color =
                        style.background_border_color.scale_alpha(opacity);
                    style.foreground_border_color =
                        style.foreground_border_color.scale_alpha(opacity);
                    style.text_color = Some(
                        style
                            .text_color
                            .unwrap_or_else(crate::theme::text)
                            .scale_alpha(opacity),
                    );
                    style
                });
            }
            if let Some(width) = width {
                item = item.width(width.clone().iced());
            }
            if let Some(size) = own {
                item = item.size(*size);
            }
            if interactive && let Some(key) = on_toggle {
                let key = key.clone();
                item = item.on_toggle(move |value| {
                    button_msg(Action::change(key.clone(), ActionValue::Boolean(value)))
                });
            }
            Ok(item.into())
        }
        WidgetNode::Slider {
            min,
            max,
            value,
            step,
            on_change,
            on_release,
            width,
            height,
        } => {
            let key = on_change.clone().unwrap_or_default();
            let sender = button_msg.clone();
            let mut item = iced::widget::slider(*min..=*max, *value, move |value| {
                sender(Action::change(key.clone(), ActionValue::Number(value)))
            })
            .step(*step);
            if opacity < 1.0 {
                item = item.style(move |theme, status| {
                    let mut style = iced::widget::slider::default(theme, status);
                    style.rail.backgrounds.0 = style.rail.backgrounds.0.scale_alpha(opacity);
                    style.rail.backgrounds.1 = style.rail.backgrounds.1.scale_alpha(opacity);
                    style.rail.border.color = style.rail.border.color.scale_alpha(opacity);
                    style.handle.background = style.handle.background.scale_alpha(opacity);
                    style.handle.border_color = style.handle.border_color.scale_alpha(opacity);
                    style
                });
            }
            if let Some(width) = width {
                item = item.width(width.clone().iced());
            }
            if let Some(height) = height {
                item = item.height(*height);
            }
            if interactive && let Some(key) = on_release {
                item = item.on_release(button_msg(key.clone().into()));
            }
            let element = item.into();
            Ok(if interactive && on_change.is_some() {
                element
            } else {
                super::motion::shifted(element, 0.0, 0.0, false)
            })
        }
        WidgetNode::TextInput {
            placeholder,
            value,
            on_input,
            on_submit,
            width,
            size: own,
            padding,
            secure,
        } => {
            let mut item = iced::widget::text_input(placeholder, value).secure(*secure);
            if opacity < 1.0 {
                item = item.style(move |theme, status| {
                    let mut style = iced::widget::text_input::default(theme, status);
                    style.background = style.background.scale_alpha(opacity);
                    style.border.color = style.border.color.scale_alpha(opacity);
                    style.icon = style.icon.scale_alpha(opacity);
                    style.placeholder = style.placeholder.scale_alpha(opacity);
                    style.value = style.value.scale_alpha(opacity);
                    style.selection = style.selection.scale_alpha(opacity);
                    style
                });
            }
            if let Some(width) = width {
                item = item.width(width.clone().iced());
            }
            if let Some(size) = own {
                item = item.size(*size);
            }
            if let Some(padding) = padding {
                item = item.padding(*padding);
            }
            if interactive && let Some(key) = on_submit {
                item = item.on_submit(button_msg(key.clone().into()));
            }
            if interactive && let Some(key) = on_input {
                let key = key.clone();
                item = item.on_input(move |value| {
                    button_msg(Action::change(key.clone(), ActionValue::Text(value)))
                });
            }
            Ok(item.into())
        }
        WidgetNode::PickList {
            options,
            selected,
            on_select,
            width,
            placeholder,
            padding,
            size: own,
        } => {
            let key = on_select.clone().unwrap_or_default();
            let mut item =
                iced::widget::pick_list(options.clone(), selected.clone(), move |value| {
                    button_msg(Action::change(key.clone(), ActionValue::Text(value)))
                });
            if opacity < 1.0 {
                item = item.style(move |theme, status| {
                    let mut style = iced::widget::pick_list::default(theme, status);
                    style.background = style.background.scale_alpha(opacity);
                    style.border.color = style.border.color.scale_alpha(opacity);
                    style.text_color = style.text_color.scale_alpha(opacity);
                    style.placeholder_color = style.placeholder_color.scale_alpha(opacity);
                    style.handle_color = style.handle_color.scale_alpha(opacity);
                    style
                });
            }
            if let Some(width) = width {
                item = item.width(width.clone().iced());
            }
            if let Some(placeholder) = placeholder {
                item = item.placeholder(placeholder.clone());
            }
            if let Some(padding) = padding {
                item = item.padding(*padding);
            }
            if let Some(size) = own {
                item = item.text_size(*size);
            }
            let element = item.into();
            Ok(if interactive && on_select.is_some() {
                element
            } else {
                super::motion::shifted(element, 0.0, 0.0, false)
            })
        }
        WidgetNode::Radio {
            label,
            value,
            selected,
            on_select,
            width,
            size: own,
            spacing,
        } => {
            let key = on_select.clone().unwrap_or_default();
            let value = value.clone();
            let mut item = iced::widget::radio(
                label.clone(),
                true,
                Some(selected.as_ref() == Some(&value)),
                move |_| button_msg(Action::change(key, ActionValue::Text(value))),
            )
            .spacing(*spacing);
            if opacity < 1.0 {
                item = item.style(move |theme, status| {
                    let mut style = iced::widget::radio::default(theme, status);
                    style.background = style.background.scale_alpha(opacity);
                    style.dot_color = style.dot_color.scale_alpha(opacity);
                    style.border_color = style.border_color.scale_alpha(opacity);
                    style.text_color = Some(
                        style
                            .text_color
                            .unwrap_or_else(crate::theme::text)
                            .scale_alpha(opacity),
                    );
                    style
                });
            }
            if let Some(width) = width {
                item = item.width(width.clone().iced());
            }
            if let Some(size) = own {
                item = item.size(*size);
            }
            let element = item.into();
            Ok(if interactive && on_select.is_some() {
                element
            } else {
                super::motion::shifted(element, 0.0, 0.0, false)
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::decode::parse_node;
    use mlua::{Lua, Value};

    #[test]
    fn text_wrap_word_folds_narrow_text_while_none_keeps_one_line() {
        use iced::Size;
        use iced::advanced::{layout, renderer::Headless, widget::Tree};
        use iced::widget::text::Wrapping;
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let lua = lua();
        let layout_height = |source: &str| {
            let value: Value = lua.load(format!("return {source}")).eval().unwrap();
            let node = parse_node(&value).unwrap();
            let mut built = build_node(&node, 13.0, None).unwrap();
            let mut tree = Tree::new(&built);
            let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 400.0));
            built
                .as_widget_mut()
                .layout(&mut tree, &renderer, &limits)
                .bounds()
                .height
        };
        let body = "word ".repeat(24).trim_end().to_string();
        let word_h = layout_height(&format!(
            "iced.text('{body}'):size(24):width(120):wrap('word')"
        ));
        let none_h = layout_height(&format!(
            "iced.text('{body}'):size(24):width(120):wrap('none')"
        ));
        assert!(
            word_h > none_h * 2.0,
            "word wrap should fold into several lines ({word_h} vs {none_h})"
        );
        assert!(
            none_h < 40.0,
            "no wrap should keep one clipped line ({none_h})"
        );
        let native_h = {
            let mut native: Element<'static, Plant> = text(body.clone())
                .size(24.0)
                .width(120)
                .wrapping(Wrapping::Word)
                .into();
            let mut tree = Tree::new(&native);
            let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 400.0));
            native
                .as_widget_mut()
                .layout(&mut tree, &renderer, &limits)
                .bounds()
                .height
        };
        assert_eq!(word_h, native_h, "lua wrap must match native iced");
    }

    #[test]
    fn omitted_dimensions_and_axis_overrides_match_native_iced_layout() {
        use iced::advanced::{layout, renderer::Headless, widget::Tree};
        use iced::{Length, Size};
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let fixed = || Space::new().width(40).height(20);
        let fill = || Space::new().width(Length::Fill).height(20);
        let cases: Vec<(&str, Element<'static, Plant>)> = vec![
            (
                "iced.container(iced.space():width(40):height(20)):padding(7)",
                container(fixed()).padding(7).into(),
            ),
            (
                "iced.container(iced.space():width('fill'):height(20)):padding(7)",
                container(fill()).padding(7).into(),
            ),
            (
                "iced.container(iced.space():width(40):height(20)):width('fill')",
                container(fixed()).width(Length::Fill).into(),
            ),
            (
                "iced.container(iced.space():width('fill'):height(20)):height(60)",
                container(fill()).height(60).into(),
            ),
            (
                "iced.container(iced.space():width('fill'):height(20)):width('shrink')",
                container(fill()).width(Length::Shrink).into(),
            ),
            (
                "iced.scrollable(iced.space():width(40):height(20))",
                iced::widget::scrollable(fixed()).into(),
            ),
            (
                "iced.scrollable(iced.space():width('fill'):height(20))",
                iced::widget::scrollable(fill()).into(),
            ),
            (
                "iced.row({iced.space():width('fill'):height(20)}):width('shrink')",
                row![fill()].width(Length::Shrink).into(),
            ),
            (
                "iced.column({iced.space():width('fill'):height(20)}):width(80)",
                iced::widget::column![fill()].width(80).into(),
            ),
            (
                "iced.container(iced.column({iced.text('Hello'),iced.text('World')}):spacing(8)):padding(12)",
                container(
                    iced::widget::column![text("Hello").size(13), text("World").size(13)]
                        .spacing(8)
                        .align_x(iced::Alignment::Center),
                )
                .padding(12)
                .into(),
            ),
        ];
        let lua = lua();
        for (source, mut native) in cases {
            let value: Value = lua.load(format!("return {source}")).eval().unwrap();
            let node = parse_node(&value).unwrap();
            let mut built = build_node(&node, 13.0, None).unwrap();
            assert_eq!(
                built.as_widget().size_hint(),
                native.as_widget().size_hint(),
                "{source}"
            );
            let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 200.0));
            let mut built_tree = Tree::new(&built);
            let mut native_tree = Tree::new(&native);
            let actual = built
                .as_widget_mut()
                .layout(&mut built_tree, &renderer, &limits);
            let expected = native
                .as_widget_mut()
                .layout(&mut native_tree, &renderer, &limits);
            assert_eq!(actual.bounds(), expected.bounds(), "{source}");
        }
    }

    #[test]
    fn style_setters_parse_build_and_gate_by_type() {
        let lua = lua();
        for (source, width, has_color) in [
            (
                r##"return iced.container(iced.text('x')):border('#ff0000')"##,
                1.0,
                true,
            ),
            (
                r##"return iced.container(iced.text('x')):border('#ff0000'):border_width(3)"##,
                3.0,
                true,
            ),
            (
                "return iced.container(iced.text('x')):border_width(3)",
                3.0,
                false,
            ),
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            match parse_node(&value).unwrap() {
                WidgetNode::Container {
                    border,
                    border_width,
                    ..
                } => {
                    assert_eq!(border_width, width, "{source}");
                    assert_eq!(border.is_some(), has_color, "{source}");
                    if has_color {
                        assert_eq!(border, Some(iced::Color::from_rgb(1.0, 0.0, 0.0)));
                    }
                }
                other => panic!("container: {other:?}"),
            }
        }
        let value: Value = lua
            .load(r##"return iced.button('go','run'):background('#00ff00')"##)
            .eval()
            .unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::Button {
                background: Some(_),
                ..
            }
        ));
        let value: Value = lua
            .load(r##"return iced.progress(0.5):color('#ff0000'):background('#0000ff')"##)
            .eval()
            .unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::Progress {
                color: Some(_),
                background: Some(_),
                ..
            }
        ));
        let value: Value = lua
            .load("return iced.separator():color({r=1,g=0,b=0})")
            .eval()
            .unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::Separator { color: Some(_), .. }
        ));
        for source in [
            r##"return iced.container(iced.text('x')):background('#112233'):border('#ff0000'):radius(8)"##,
            r##"return iced.button('go','run'):background('#00ff00'):color('#000000')"##,
            r##"return iced.progress(0.5):color('#ff0000'):background('#0000ff')"##,
            r##"return iced.separator():color('#ff0000')"##,
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            let _ = build_node(&parse_node(&value).unwrap(), 13.0, None).unwrap();
        }
        for source in [
            "return iced.button('go','run'):background(42)",
            "return iced.separator():color(42)",
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            assert!(parse_node(&value).is_err());
        }
        assert!(
            lua.load(r##"return iced.text('x'):background('#fff')"##)
                .eval::<Value>()
                .is_err()
        );
        assert!(
            lua.load(r##"return iced.button('go','run'):border('#fff')"##)
                .eval::<Value>()
                .is_err()
        );
    }

    fn lua() -> Lua {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        lua
    }

    #[test]
    fn build_node_builds_every_primitive_without_a_renderer() {
        let text = |content: &str| WidgetNode::Text {
            content: content.into(),
            size: None,
            width: None,
            height: None,
            wrapping: None,
            color: None,
        };
        let button = WidgetNode::Button {
            label: "go".into(),
            action: "run".into(),
            width: None,
            height: None,
            padding: None,
            color: None,
            background: None,
            radius: None,
        };
        for node in [
            text("hi"),
            WidgetNode::Icon {
                name: "cpu".into(),
                color: None,
            },
            WidgetNode::Icon {
                name: "typo".into(),
                color: None,
            },
            WidgetNode::Row {
                children: vec![text("a")],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 2.0,
            },
            WidgetNode::Column {
                children: vec![],
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
                spacing: 2.0,
            },
            button.clone(),
            WidgetNode::Progress {
                value: 1.5,
                width: NodeLength::Fixed(120.0),
                height: None,
                color: None,
                background: None,
            },
            WidgetNode::Separator {
                height: 2.0,
                color: None,
            },
        ] {
            let _ = build_node(&node, 13.0, None).unwrap();
        }
        let _ = build_node(&button, 13.0, Some(std::sync::Arc::new(|_| Plant::Tend))).unwrap();
    }

    #[test]
    fn ui_spinner_parses_and_builds() {
        let lua = lua();
        let value: Value = lua.load("return iced.spinner()").eval().unwrap();
        let node = parse_node(&value).unwrap();
        assert_eq!(node, WidgetNode::Spinner);
        let _ = build_node(&node, 13.0, None).unwrap();
        let value: Value = lua
            .load("return iced.row({iced.spinner(),iced.text('x')})")
            .eval()
            .unwrap();
        assert!(
            matches!(parse_node(&value).unwrap(), WidgetNode::Row {children, ..} if children.len() == 2)
        );
    }

    #[test]
    fn color_setter_accepts_hex_and_rgba_tables() {
        let lua = lua();
        let red = iced::Color::from_rgb(1.0, 0.0, 0.0);
        for (source, expected) in [
            (r##"return iced.text('hi'):color('#ff0000')"##, red),
            (r##"return iced.text('hi'):color('#f00')"##, red),
            ("return iced.text('hi'):color({r=1,g=0,b=0})", red),
            (
                "return iced.text('hi'):color({1,0,0,0.5})",
                iced::Color::from_rgba(1.0, 0.0, 0.0, 0.5),
            ),
        ] {
            let value: Value = lua.load(source).eval().unwrap();
            match parse_node(&value).unwrap() {
                WidgetNode::Text {
                    color: Some(color), ..
                } => assert!(
                    (color.r - expected.r).abs() < 0.01
                        && (color.g - expected.g).abs() < 0.01
                        && (color.b - expected.b).abs() < 0.01
                        && (color.a - expected.a).abs() < 0.01,
                    "{source} -> {color:?}"
                ),
                other => panic!("{source} -> {other:?}"),
            }
        }
        let value: Value = lua.load("return iced.text('hi')").eval().unwrap();
        assert!(matches!(
            parse_node(&value).unwrap(),
            WidgetNode::Text { color: None, .. }
        ));
        let value: Value = lua
            .load("return iced.text('hi'):color('nope')")
            .eval()
            .unwrap();
        let error = parse_node(&value).unwrap_err();
        assert!(error.contains("color"), "{error}");
        let value: Value = lua
            .load(r##"return iced.button('go','run'):color('#00ff00')"##)
            .eval()
            .unwrap();
        let node = parse_node(&value).unwrap();
        assert!(matches!(node, WidgetNode::Button { color: Some(_), .. }));
        let _ = build_node(&node, 13.0, None).unwrap();
        let value: Value = lua
            .load(r##"return iced.icon('cpu'):color('#00ff00')"##)
            .eval()
            .unwrap();
        let node = parse_node(&value).unwrap();
        assert!(matches!(node, WidgetNode::Icon { color: Some(_), .. }));
        let _ = build_node(&node, 13.0, None).unwrap();
    }
}
