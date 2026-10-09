//! Owned UI descriptions shared by widgets, shell chrome, and the Lua host.
use super::listview::Transition;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WidgetNode {
    ListView {
        id: String,
        items: Vec<(String, WidgetNode)>,
        horizontal: bool,
        pitch: f32,
        spacing: f32,
        width: NodeLength,
        height: NodeLength,
        transitions: Box<(Transition, Transition, Transition)>,
    },
    Container {
        child: Box<WidgetNode>,
        width: NodeLength,
        height: NodeLength,
        padding: f32,
        background: Option<iced::Color>,
        radius: f32,
        border: Option<iced::Color>,
        border_width: f32,
    },
    Scrollable {
        child: Box<WidgetNode>,
        width: NodeLength,
        height: NodeLength,
    },
    Space {
        width: NodeLength,
        height: NodeLength,
    },
    Image {
        path: String,
        width: NodeLength,
        height: NodeLength,
    },
    Text {
        content: String,
        size: Option<f32>,
        width: Option<NodeLength>,
        height: Option<NodeLength>,
        color: Option<iced::Color>,
    },
    Icon {
        name: String,
        color: Option<iced::Color>,
    },
    Row {
        children: Vec<WidgetNode>,
        spacing: f32,
        width: NodeLength,
        height: NodeLength,
    },
    Column {
        children: Vec<WidgetNode>,
        spacing: f32,
        width: NodeLength,
        height: NodeLength,
    },
    /// Actions are opaque keys; each host supplies its own message routing.
    Button {
        label: String,
        action: String,
        width: Option<NodeLength>,
        height: Option<NodeLength>,
        padding: Option<f32>,
        color: Option<iced::Color>,
        background: Option<iced::Color>,
        radius: Option<f32>,
    },
    Progress {
        value: f32,
        width: NodeLength,
        height: Option<NodeLength>,
        color: Option<iced::Color>,
        background: Option<iced::Color>,
    },
    Spinner,
    Separator {
        height: f32,
        color: Option<iced::Color>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeLength {
    Fill,
    Shrink,
    Fixed(f32),
}

impl NodeLength {
    pub(crate) fn iced(self) -> iced::Length {
        match self {
            Self::Fill => iced::Length::Fill,
            Self::Shrink => iced::Length::Shrink,
            Self::Fixed(px) => iced::Length::Fixed(px.max(1.0)),
        }
    }
}
