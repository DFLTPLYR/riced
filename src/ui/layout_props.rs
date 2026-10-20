//! Common layout/event properties preserve the underlying component's setters.
use super::{node::WidgetNode, placement::Placement};
pub(crate) const SETTERS: &[&str] = &["anchor", "margin", "on_press"];
/// Cross/main-axis alignment for the widgets whose native API exposes it.
/// `None` keeps the widget's native default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Align {
    pub x: Option<iced::Alignment>,
    pub y: Option<iced::Alignment>,
}
impl Align {
    fn is_set(self) -> bool {
        self.x.is_some() || self.y.is_some()
    }
}
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Common {
    pub anchor: Option<Placement>,
    pub margin: f32,
    pub on_press: Option<String>,
    pub align: Align,
}
impl Common {
    pub(crate) fn wrap(self, mut node: WidgetNode) -> WidgetNode {
        if self.align.is_set() {
            node = WidgetNode::Aligned {
                child: Box::new(node),
                align_x: self.align.x,
                align_y: self.align.y,
            };
        }
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
            WidgetNode::Aligned {
                child,
                align_x,
                align_y,
            } => {
                props.align = Align {
                    x: align_x,
                    y: align_y,
                };
                node = *child;
            }
            _ => return (node, props),
        }
    }
}
