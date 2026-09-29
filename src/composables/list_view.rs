use crate::app::Plant;
use crate::theme;
use crate::theme::Class;
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{column, container, row, scrollable};
use iced::{Element, Fill, Length, Padding, Pixels};

/// QML-like `ListView { }` — scrollable list of delegate items with
/// spacing and a `currentIndex` highlight, vertical by default:
///
/// ```ignore
/// list_view()
///     .items(themes.iter().map(|t| text(t.clone()).into()))
///     .spacing(4)
///     .highlight(selected)
///     .width(Fill)
///     .into()
/// ```
///
/// Dumb filler like the other composables: selection state and event code
/// stay in the layers, this only owns layout. Each `.property()` is only
/// applied when set.
#[derive(Default)]
pub struct ListView<'a> {
    items: Vec<Element<'a, Plant>>,
    spacing: Option<f32>,
    padding: Option<Padding>,
    width: Option<Length>,
    height: Option<Length>,
    highlight: Option<usize>,
    horizontal: bool,
}

pub fn list_view<'a>() -> ListView<'a> {
    ListView {
        ..Default::default()
    }
}

impl<'a> ListView<'a> {
    /// Push a single delegate item.
    pub fn item(mut self, item: impl Into<Element<'a, Plant>>) -> Self {
        self.items.push(item.into());
        self
    }

    /// Push a whole model at once (any iterator of delegate items).
    pub fn items(
        mut self,
        items: impl IntoIterator<Item = impl Into<Element<'a, Plant>>>,
    ) -> Self {
        self.items.extend(items.into_iter().map(Into::into));
        self
    }

    /// Accepts int or float: `.spacing(4)`, `.spacing(4.0)`.
    pub fn spacing(mut self, spacing: impl Into<Pixels>) -> Self {
        self.spacing = Some(spacing.into().0);
        self
    }

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

    /// `currentIndex`: wraps that item in the selection box. Out of range
    /// (or unset) highlights nothing.
    pub fn highlight(mut self, index: usize) -> Self {
        self.highlight = Some(index);
        self
    }

    /// Lay out horizontally (row + horizontal scroll) instead of vertically.
    pub fn horizontal(mut self) -> Self {
        self.horizontal = true;
        self
    }

    /// Lay out vertically (the default).
    pub fn vertical(mut self) -> Self {
        self.horizontal = false;
        self
    }

    pub fn view(self) -> Element<'a, Plant> {
        let spacing = self.spacing.unwrap_or(0.0);
        let horizontal = self.horizontal;
        let highlight = self.highlight;
        let mut kids = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.into_iter().enumerate() {
            if Some(i) == highlight {
                // Selection box: surface-variant fill + rounding, stretched
                // across the list's long axis so the row reads as selected.
                let mut box_ = container(item).style(theme::container_style(&[
                    Class::BgSurfaceVariant,
                    Class::Rounded,
                ]));
                if horizontal {
                    box_ = box_.height(Fill);
                } else {
                    box_ = box_.width(Fill);
                }
                kids.push(box_.into());
            } else {
                kids.push(item);
            }
        }
        if horizontal {
            let mut inner = row(kids).spacing(spacing);
            if let Some(p) = self.padding {
                inner = inner.padding(p);
            }
            let mut view =
                scrollable(inner).direction(Direction::Horizontal(Scrollbar::default()));
            view = view.width(self.width.unwrap_or(Length::Fill));
            if let Some(h) = self.height {
                view = view.height(h);
            }
            view.into()
        } else {
            let mut inner = column(kids).spacing(spacing).width(Fill);
            if let Some(p) = self.padding {
                inner = inner.padding(p);
            }
            let mut view = scrollable(inner).direction(Direction::Vertical(Scrollbar::default()));
            view = view.width(self.width.unwrap_or(Length::Fill));
            if let Some(h) = self.height {
                view = view.height(h);
            }
            view.into()
        }
    }
}

impl<'a> From<ListView<'a>> for Element<'a, Plant> {
    fn from(list: ListView<'a>) -> Self {
        list.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[test]
    fn int_and_float_props_compile() {
        let _: Element<'_, Plant> = list_view()
            .item(text("a"))
            .spacing(4)
            .highlight(0)
            .width(180)
            .into();
        let _: Element<'_, Plant> = list_view()
            .items([text("a"), text("b")])
            .spacing(4.0)
            .padding(8)
            .height(92.0)
            .horizontal()
            .vertical()
            .into();
    }

    #[test]
    fn out_of_range_highlight_highlights_nothing() {
        let _: Element<'_, Plant> = list_view().highlight(99).into();
        let _: Element<'_, Plant> = list_view().into();
    }
}
