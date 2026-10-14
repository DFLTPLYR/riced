//! Owned UI descriptions shared by widgets, shell chrome, and the Lua host.
pub(crate) use super::components::WidgetNode;

/// Whether a description tree draws any raster image nodes.
pub(crate) fn contains_images(node: &WidgetNode) -> bool {
    match node {
        WidgetNode::Image { .. } => true,
        WidgetNode::Container { child, .. } | WidgetNode::Scrollable { child, .. } => {
            contains_images(child)
        }
        WidgetNode::Row { children, .. } | WidgetNode::Column { children, .. } => {
            children.iter().any(contains_images)
        }
        WidgetNode::ListView { items, .. } => items.iter().any(|(_, child)| contains_images(child)),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeLength {
    /// No setter: use the native widget's content-derived sizing strategy.
    Native,
    Fill,
    Shrink,
    Fixed(f32),
}

impl NodeLength {
    pub(crate) fn resolve(&self, native: iced::Length) -> iced::Length {
        match self {
            Self::Native => native,
            other => other.clone().iced(),
        }
    }

    pub(crate) fn iced(self) -> iced::Length {
        match self {
            Self::Fill => iced::Length::Fill,
            Self::Native | Self::Shrink => iced::Length::Shrink,
            Self::Fixed(px) => iced::Length::Fixed(px.max(1.0)),
        }
    }
}
