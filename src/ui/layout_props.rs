//! Common layout/event properties preserve the underlying component's setters.
use super::{node::WidgetNode, placement::Placement};
pub(crate) const SETTERS: &[&str] = &["anchor", "margin", "on_press"];
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Common {
    pub anchor: Option<Placement>,
    pub margin: f32,
    pub on_press: Option<String>,
}
impl Common {
    pub(crate) fn wrap(self, mut node: WidgetNode) -> WidgetNode {
        if let Some(action) = self.on_press {
            node = WidgetNode::Clickable {
                child: Box::new(node),
                action,
            };
        }
        if let Some(placement) = self.anchor {
            node = WidgetNode::Anchored {
                child: Box::new(node),
                placement,
                margin: self.margin,
            };
        }
        node
    }
}
pub(crate) fn unwrap(mut node: WidgetNode) -> (WidgetNode, Common) {
    let mut props = Common::default();
    loop {
        match node {
            WidgetNode::Anchored {
                child,
                placement,
                margin,
            } => {
                props.anchor = Some(placement);
                props.margin = margin;
                node = *child;
            }
            WidgetNode::Clickable { child, action } => {
                props.on_press = Some(action);
                node = *child;
            }
            _ => return (node, props),
        }
    }
}
