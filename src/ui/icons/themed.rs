//! SVG icons inherit the enclosing text color, including button state changes.
// Adapted from lucide-iced 0.4.0 src/themed.rs.
// ISC License
// Copyright (c) 2026 lucide-iced contributors
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
use iced::{
    Element, Length, Radians, Rectangle, Size,
    advanced::{Layout, Widget, layout, renderer, svg, widget::Tree},
    mouse,
};

pub(crate) struct ThemedIcon {
    handle: svg::Handle,
    size: f32,
    opacity: f32,
}

impl ThemedIcon {
    pub(crate) fn new(handle: svg::Handle, size: f32) -> Self {
        Self {
            handle,
            size,
            opacity: 1.0,
        }
    }
    pub(crate) fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for ThemedIcon
where
    Renderer: renderer::Renderer + svg::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }
    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fixed(self.size), Length::Fixed(self.size))
    }
    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        renderer.draw_svg(
            svg::Svg {
                handle: self.handle.clone(),
                color: Some(style.text_color),
                rotation: Radians(0.0),
                opacity: self.opacity,
            },
            layout.bounds(),
            *viewport,
        );
    }
}

impl<'a, Message, Theme, Renderer> From<ThemedIcon> for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + svg::Renderer + 'a,
{
    fn from(icon: ThemedIcon) -> Self {
        Element::new(icon)
    }
}

pub(crate) fn themed_icon<'a, Message: 'a>(
    bytes: &'static [u8],
    size: f32,
) -> Element<'a, Message> {
    ThemedIcon::new(svg::Handle::from_memory(bytes), size).into()
}
