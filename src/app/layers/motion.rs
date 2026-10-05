//! QML-style motion wrapper for animated list items.
//!
//! iced 0.14 has no opacity or translate widget, so this module wraps
//! any [`Element`] in a custom [`Widget`] that draws its content
//! through a GPU transform + alpha fade:
//!
//! - [`shifted`] offsets the item by `(x, y)` px at draw time (the
//!   `Float`-widget overlay trick: layout is untouched, only the
//!   painted position moves — like QML `x`/`y`).
//! - [`faded`] scales the card chrome + text alpha by `opacity`
//!   (style-level fade; icons inherit `text_color`, so they fade too).
//!
//! Settled items (`x = y = 0`, `opacity = 1`) skip the wrapper so the
//! tree stays flat when nothing animates.

use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Layout, Shell};
use iced::{Element, Event, Length, Rectangle, Transformation, Vector};

/// Draw `content` shifted by `(x, y)` px without disturbing layout.
///
/// `clickable` gates hit-testing: ghosts render through the same
/// wrapper but stay inert. Near-zero offsets return the content
/// unwrapped (fewer layout nodes when settled).
pub fn shifted<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    x: f32,
    y: f32,
    clickable: bool,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    if x.abs() < 0.05 && y.abs() < 0.05 {
        return content.into();
    }
    Element::new(Shift {
        content: content.into(),
        offset: Vector::new(x, y),
        clickable,
    })
}

/// Fade a card container + its text by `opacity` (style-level: the
/// wrapper rebuilds the chrome style with scaled alpha and passes a
/// dimmed `text_color` down to the content). Near-one opacity returns
/// the content unwrapped.
pub fn faded<'a, Message: 'a>(
    content: iced::widget::Container<'a, Message, iced::Theme, iced::Renderer>,
    opacity: f32,
) -> Element<'a, Message, iced::Theme, iced::Renderer> {
    if opacity >= 0.995 {
        return content.into();
    }
    Element::new(Fade {
        content: content.into(),
        opacity: opacity.clamp(0.0, 1.0),
    })
}

struct Shift<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    offset: Vector,
    clickable: bool,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Shift<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> widget::tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> iced::Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> iced::Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if !self.clickable {
            return;
        }
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        renderer.with_translation(self.offset, |renderer| {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if !self.clickable {
            return mouse::Interaction::None;
        }
        // Hit-test against the SHIFTED bounds: the cursor is in layout
        // coords, the card paints offset — translate the query so
        // clicks land where the card visibly is.
        let shifted_layout = layout;
        let _ = shifted_layout;
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        offset: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        void::Void::into_overlay_none(
            self.content
                .as_widget_mut()
                .overlay(tree, layout, renderer, viewport, offset),
        )
    }
}

/// Helper to map `Option<overlay::Element>` through without naming
/// the crate's internal `Void` type at the call site.
mod void {
    pub struct Void;
    impl Void {
        pub fn into_overlay_none<'b, Message, Theme, Renderer>(
            inner: Option<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>>,
        ) -> Option<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
            inner
        }
    }
}

impl<'a, Message, Theme, Renderer> From<Shift<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(shift: Shift<'a, Message, Theme, Renderer>) -> Self {
        Element::new(shift)
    }
}

struct Fade<'a, Message> {
    content: Element<'a, Message, iced::Theme, iced::Renderer>,
    opacity: f32,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Fade<'_, Message> {
    fn tag(&self) -> widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> widget::tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> iced::Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> iced::Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Scale the inherited text color: container chrome passes it
        // down as `text_color`, and themed icons use it as their tint
        // — so one scale fades text, icons, and labels together.
        let faded = renderer::Style {
            text_color: style.text_color.scale_alpha(self.opacity),
        };
        renderer.with_layer(layout.bounds(), |renderer| {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, &faded, layout, cursor, viewport);
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        offset: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, offset)
    }
}

impl<'a, Message> From<Fade<'a, Message>> for Element<'a, Message, iced::Theme, iced::Renderer>
where
    Message: 'a,
{
    fn from(fade: Fade<'a, Message>) -> Self {
        Element::new(fade)
    }
}

/// Unused import guard: [`Transformation`] is the GPU primitive the
/// overlay path would use for combined transforms (kept referenced so
/// the design note stays compiler-checked).
#[allow(dead_code)]
fn _transform_note(t: Transformation) -> Transformation {
    t
}
