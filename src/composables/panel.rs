use crate::app::Plant;
use iced::widget::{Space, container};
use iced::{Element, Fill, Length, Padding};

/// QML-like `Panel { }` — dumb full-size layer filler.
/// Event code stays in the layers; this only owns layout:
///
/// ```ignore
/// panel()
///     .content(text("TOP BAR"))
///     .into()
/// ```
pub struct Panel<'a> {
    content: Option<Element<'a, Plant>>,
    padding: Option<Padding>,
    width: Option<Length>,
    height: Option<Length>,
}

impl<'a> Default for Panel<'a> {
    fn default() -> Self {
        Panel {
            content: None,
            padding: None,
            width: None,
            height: None,
        }
    }
}

pub fn panel<'a>() -> Panel<'a> {
    Panel {
        ..Default::default()
    }
}

impl<'a> Panel<'a> {
    pub fn content(mut self, content: impl Into<Element<'a, Plant>>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Accepts int or float: `.padding(8)`, `.padding(8.0)`, `.padding([8, 12])`.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = Some(padding.into());
        self
    }

    /// Defaults to `Fill` when unset: `.width(180)`, `.width(180.0)`, `.width(Fill)`.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    /// Defaults to `Fill` when unset.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = Some(height.into());
        self
    }

    pub fn view(self) -> Element<'a, Plant> {
        let content = self
            .content
            .unwrap_or_else(|| Space::new().width(0).height(0).into());
        let mut inner = container(content);
        if let Some(p) = self.padding {
            inner = inner.padding(p);
        }
        inner
            .width(self.width.unwrap_or(Fill))
            .height(self.height.unwrap_or(Fill))
            .into()
    }
}

impl<'a> From<Panel<'a>> for Element<'a, Plant> {
    fn from(panel: Panel<'a>) -> Self {
        panel.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[test]
    fn fill_by_default() {
        let _: Element<'_, Plant> = panel().content(text("hi")).into();
        let _: Element<'_, Plant> = panel()
            .content(text("hi"))
            .padding(8)
            .width(180)
            .height(92)
            .into();
    }
}
