//! Named, owned component descriptions and their single Lua API schema.
//! Structs convert through mlua::FromLua/IntoLua and never retain a Lua VM.
use super::{listview::Transition, node::NodeLength};

macro_rules! components {
    ($($variant:ident => $name:ident, $lua:literal, [$($arg:literal),*], [$($setter:literal),*] {
        $($field:ident: $ty:ty),* $(,)?
    })*) => {
        #[derive(Debug, Clone, PartialEq)]
        pub(crate) enum WidgetNode {
            $($variant { $($field: $ty),* },)*
            Spinner,
        }
        $(
            #[derive(Debug, Clone, PartialEq)]
            pub(crate) struct $name { $(pub $field: $ty),* }

            impl From<$name> for WidgetNode {
                fn from(node: $name) -> Self {
                    let $name { $($field),* } = node;
                    Self::$variant { $($field),* }
                }
            }
            impl mlua::FromLua for $name {
                fn from_lua(value: mlua::Value, _: &mlua::Lua) -> mlua::Result<Self> {
                    match super::decode::parse_node(&value).map_err(mlua::Error::RuntimeError)? {
                        WidgetNode::$variant { $($field),* } => Ok(Self { $($field),* }),
                        _ => Err(mlua::Error::RuntimeError(concat!("expected iced.", $lua, " component").into())),
                    }
                }
            }
            impl mlua::IntoLua for $name {
                fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
                    super::encode::to_table(lua, &self.into()).map(mlua::Value::Table)
                }
            }
        )*
        pub(crate) const COMPONENTS: &[Component] = &[
            $(Component { name: $lua, rust_type: stringify!($name), arguments: &[$($arg),*], setters: &[$($setter),*] },)*
            Component { name: "spinner", rust_type: "SpinnerNode", arguments: &[], setters: &[] },
        ];
        impl WidgetNode {
            pub(crate) fn kind(&self) -> &'static str {
                match self { $(Self::$variant { .. } => $lua,)* Self::Spinner => "spinner" }
            }
        }
    };
}

pub(crate) struct Component {
    pub name: &'static str,
    pub rust_type: &'static str,
    pub arguments: &'static [&'static str],
    pub setters: &'static [&'static str],
}

components! {
    ListView => ListViewNode, "listview", ["data"], ["id", "key", "delegate", "axis", "pitch", "spacing", "width", "height"] {
        id: String, items: Vec<(String, WidgetNode)>, horizontal: bool,
        pitch: f32, spacing: f32, width: NodeLength, height: NodeLength,
        transitions: Box<(Transition, Transition, Transition)>
    }
    Container => ContainerNode, "container", ["child"], ["width", "height", "padding", "background", "radius", "border", "border_width"] {
        child: Box<WidgetNode>, width: NodeLength, height: NodeLength, padding: f32,
        background: Option<iced::Color>, radius: f32, border: Option<iced::Color>, border_width: f32
    }
    Scrollable => ScrollableNode, "scrollable", ["child"], ["width", "height"] {
        child: Box<WidgetNode>, width: NodeLength, height: NodeLength
    }
    Space => SpaceNode, "space", [], ["width", "height"] { width: NodeLength, height: NodeLength }
    Image => ImageNode, "image", ["path"], ["width", "height"] { path: String, width: NodeLength, height: NodeLength }
    Text => TextNode, "text", ["text"], ["size", "width", "height", "wrap", "color"] {
        content: String, size: Option<f32>, width: Option<NodeLength>, height: Option<NodeLength>,
        wrapping: Option<iced::widget::text::Wrapping>, color: Option<iced::Color>
    }
    Icon => IconNode, "icon", ["name"], ["color"] { name: String, color: Option<iced::Color> }
    Row => RowNode, "row", ["children", "spacing"], ["spacing", "width", "height"] {
        children: Vec<WidgetNode>, spacing: f32, width: NodeLength, height: NodeLength
    }
    Column => ColumnNode, "column", ["children", "spacing"], ["spacing", "width", "height"] {
        children: Vec<WidgetNode>, spacing: f32, width: NodeLength, height: NodeLength
    }
    Button => ButtonNode, "button", ["label", "action"], ["width", "height", "padding", "radius", "background", "color"] {
        label: String, action: String, width: Option<NodeLength>, height: Option<NodeLength>,
        padding: Option<f32>, color: Option<iced::Color>, background: Option<iced::Color>, radius: Option<f32>
    }
    Progress => ProgressNode, "progress", ["value", "width"], ["width", "height", "color", "background"] {
        value: f32, width: NodeLength, height: Option<NodeLength>, color: Option<iced::Color>, background: Option<iced::Color>
    }
    Separator => SeparatorNode, "separator", [], ["height", "color"] { height: f32, color: Option<iced::Color> }
    Checkbox => CheckboxNode, "checkbox", ["label", "checked"], ["checked", "on_toggle", "width", "size", "spacing"] {
        label: String, checked: bool, on_toggle: Option<String>, width: Option<NodeLength>, size: Option<f32>, spacing: f32
    }
    Toggler => TogglerNode, "toggler", ["label", "enabled"], ["enabled", "on_toggle", "width", "size", "spacing"] {
        label: String, enabled: bool, on_toggle: Option<String>, width: Option<NodeLength>, size: Option<f32>, spacing: f32
    }
    Slider => SliderNode, "slider", ["min", "max", "value"], ["value", "step", "on_change", "on_release", "width", "height"] {
        min: f32, max: f32, value: f32, step: f32, on_change: Option<String>, on_release: Option<String>, width: Option<NodeLength>, height: Option<f32>
    }
    TextInput => TextInputNode, "text_input", ["placeholder", "value"], ["value", "on_input", "on_submit", "width", "size", "padding", "secure"] {
        placeholder: String, value: String, on_input: Option<String>, on_submit: Option<String>, width: Option<NodeLength>, size: Option<f32>, padding: Option<f32>, secure: bool
    }
    PickList => PickListNode, "pick_list", ["options", "selected"], ["selected", "on_select", "width", "placeholder", "padding", "size"] {
        options: Vec<String>, selected: Option<String>, on_select: Option<String>, width: Option<NodeLength>, placeholder: Option<String>, padding: Option<f32>, size: Option<f32>
    }
    Radio => RadioNode, "radio", ["label", "value", "selected"], ["selected", "on_select", "width", "size", "spacing"] {
        label: String, value: String, selected: Option<String>, on_select: Option<String>, width: Option<NodeLength>, size: Option<f32>, spacing: f32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpinnerNode;
impl From<SpinnerNode> for WidgetNode {
    fn from(_: SpinnerNode) -> Self {
        Self::Spinner
    }
}
impl mlua::IntoLua for SpinnerNode {
    fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        super::encode::to_table(lua, &WidgetNode::Spinner).map(mlua::Value::Table)
    }
}
impl mlua::FromLua for SpinnerNode {
    fn from_lua(value: mlua::Value, _: &mlua::Lua) -> mlua::Result<Self> {
        match super::decode::parse_node(&value).map_err(mlua::Error::RuntimeError)? {
            WidgetNode::Spinner => Ok(Self),
            _ => Err(mlua::Error::RuntimeError(
                "expected iced.spinner component".into(),
            )),
        }
    }
}
