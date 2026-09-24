use crate::app::Plant;
use iced::widget::container;
use iced::{Color, Element, Fill, Length, Padding};

/// QML-like `Menu { }` component. Content is passed in, each
/// `.property()` is only applied when set:
///
/// ```ignore
/// menu(
///     column![
///         button(text("Add Top").size(12).color(Color::WHITE))
///             .on_press(Plant::AddTop)
///             .width(Fill)
///     ]
///     .spacing(8)
///     .width(Fill),
/// )
/// .width(plots.config.context_menu.width)
/// .height(plots.config.menu.height)
/// .position(lx, ly)
/// .into()
/// ```
pub struct Menu<'a> {
    content: Element<'a, Plant>,
    padding: Option<Padding>,
    width: Option<Length>,
    height: Option<Length>,
    lx: Option<f32>,
    ly: Option<f32>,
}

pub fn menu<'a>(content: impl Into<Element<'a, Plant>>) -> Menu<'a> {
    Menu {
        content: content.into(),
        padding: None,
        width: None,
        height: None,
        lx: None,
        ly: None,
    }
}

impl<'a> Menu<'a> {
    /// Accepts int or float: `.padding(8)`, `.padding(8.0)`, `.padding([8, 12])`.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = Some(padding.into());
        self
    }

    /// Accepts int or float: `.width(180)`, `.width(180.0)`, `.width(Fill)`.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = Some(height.into());
        self
    }

    /// Local position via the outer `Fill + padding` trick.
    /// Skipped entirely unless passed.
    pub fn position(mut self, lx: f32, ly: f32) -> Self {
        self.lx = Some(lx);
        self.ly = Some(ly);
        self
    }

    pub fn view(self) -> Element<'a, Plant> {
        let mut inner = container(self.content)
            .clip(true)
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
                border: iced::Border {
                    color: Color::from_rgb(0.5, 0.5, 0.55),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            });

        if let Some(p) = self.padding {
            inner = inner.padding(p);
        }
        if let Some(w) = self.width {
            inner = inner.width(w);
        }
        if let Some(h) = self.height {
            inner = inner.height(h);
        }

        match (self.lx, self.ly) {
            (Some(lx), Some(ly)) => container(inner)
                .width(Fill)
                .height(Fill)
                .padding(iced::Padding {
                    top: ly,
                    left: lx,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into(),
            _ => inner.into(),
        }
    }
}

impl<'a> From<Menu<'a>> for Element<'a, Plant> {
    fn from(menu: Menu<'a>) -> Self {
        menu.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[test]
    fn int_and_float_props_compile() {
        // int literals resolve via From<u16>/From<u32>, floats via From<f32>
        let _: Element<'_, Plant> = menu(text("hi")).padding(8).width(180).height(92).into();
        let _: Element<'_, Plant> = menu(text("hi"))
            .padding(8.0)
            .width(180.0)
            .height(92.0)
            .position(1.0, 2.0)
            .into();
    }
}
