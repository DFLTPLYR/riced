//! Surface roots are decoded separately from widget trees, so they cannot nest.
use crate::ui::{decode::parse_node_hovered, node::WidgetNode};
use iced_exwlshell::reexport::{
    Anchor, BlurOption, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings,
    OutputOption,
};
use mlua::{Lua, LuaSerdeExt, Table, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Anchors {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Margin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Surface {
    pub id: String,
    pub layer: String,
    pub anchors: Anchors,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub exclusive_zone: i32,
    pub margin: Margin,
    pub keyboard: String,
    pub input: String,
    pub pointer_events: bool,
}
impl Default for Surface {
    fn default() -> Self {
        Self {
            id: "main".into(),
            layer: "top".into(),
            anchors: Anchors::default(),
            width: None,
            height: None,
            exclusive_zone: 0,
            margin: Margin::default(),
            keyboard: "none".into(),
            input: "all".into(),
            pointer_events: false,
        }
    }
}
impl Surface {
    fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("layer.id must not be empty".into());
        }
        if !["background", "bottom", "top", "overlay"].contains(&self.layer.as_str()) {
            return Err("layer.layer must be background, bottom, top, or overlay".into());
        }
        if !["none", "on_demand", "exclusive"].contains(&self.keyboard.as_str()) {
            return Err("layer.keyboard must be none, on_demand, or exclusive".into());
        }
        if !["all", "none"].contains(&self.input.as_str()) {
            return Err("layer.input must be all or none".into());
        }
        for (name, size, opposite) in [
            ("width", self.width, self.anchors.left && self.anchors.right),
            (
                "height",
                self.height,
                self.anchors.top && self.anchors.bottom,
            ),
        ] {
            if let Some(size) = size {
                if !(1..=32768).contains(&size) {
                    return Err(format!("layer.{name} must be in 1..32768"));
                }
            } else if !opposite {
                return Err(format!(
                    "layer.{name} requires a size or both opposite anchors"
                ));
            }
        }
        if !(-1..=32768).contains(&self.exclusive_zone) {
            return Err("layer.exclusive_zone must be in -1..32768".into());
        }
        Ok(())
    }
    pub fn settings(&self, output: u32, namespace: String) -> NewLayerShellSettings {
        let mut anchor = Anchor::empty();
        for (enabled, edge) in [
            (self.anchors.top, Anchor::Top),
            (self.anchors.right, Anchor::Right),
            (self.anchors.bottom, Anchor::Bottom),
            (self.anchors.left, Anchor::Left),
        ] {
            if enabled {
                anchor |= edge;
            }
        }
        let size = match (self.width, self.height) {
            (Some(w), Some(h)) => LayerSize::px(w, h),
            (None, Some(h)) => LayerSize::fill_width(h),
            (Some(w), None) => LayerSize::fill_height(w),
            (None, None) => LayerSize::FILL,
        };
        NewLayerShellSettings {
            anchor,
            size,
            layer: match self.layer.as_str() {
                "background" => Layer::Background,
                "bottom" => Layer::Bottom,
                "overlay" => Layer::Overlay,
                _ => Layer::Top,
            },
            exclusive_zone: Some(self.exclusive_zone),
            output_option: OutputOption::GlobalName(output),
            margin: Some((
                self.margin.top,
                self.margin.right,
                self.margin.bottom,
                self.margin.left,
            )),
            namespace: Some(namespace),
            keyboard_interactivity: match self.keyboard.as_str() {
                "exclusive" => KeyboardInteractivity::Exclusive,
                "on_demand" => KeyboardInteractivity::OnDemand,
                _ => KeyboardInteractivity::None,
            },
            events_transparent: self.input == "none",
            blur_option: BlurOption::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LayerRoot {
    pub surface: Surface,
    pub content: WidgetNode,
    pub popup: Option<AppPopup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PopupGeometry {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: u32,
    pub height: u32,
}
impl Default for PopupGeometry {
    fn default() -> Self {
        Self {
            id: "popup".into(),
            x: 0.0,
            y: 0.0,
            width: 0,
            height: 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AppPopup {
    pub geometry: PopupGeometry,
    pub content: WidgetNode,
}
impl AppPopup {
    fn parse(
        lua: &Lua,
        value: Value,
        hover: &crate::ui::hover::HoverSet,
    ) -> mlua::Result<Option<Self>> {
        if value.is_nil() {
            return Ok(None);
        }
        let Value::Table(table) = value else {
            return Err(mlua::Error::RuntimeError(
                "layer.popup must be a table or nil".into(),
            ));
        };
        const ALLOWED: &[&str] = &[
            "id",
            "x",
            "y",
            "width",
            "height",
            "content",
            "padding",
            "background",
            "radius",
            "border",
            "border_width",
            "align_x",
            "align_y",
            "on_open",
            "on_close",
        ];
        let fields = lua.create_table()?;
        for pair in table.clone().pairs::<String, Value>() {
            let (key, value) = pair?;
            if !ALLOWED.contains(&key.as_str()) {
                return Err(mlua::Error::RuntimeError(format!(
                    "layer.popup unknown field {key:?}"
                )));
            }
            if key != "content" {
                fields.raw_set(key, value)?;
            }
        }
        let geometry: PopupGeometry = lua.from_value(Value::Table(fields))?;
        if geometry.id.is_empty()
            || geometry.id.len() > 64
            || !geometry
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || !geometry.x.is_finite()
            || !geometry.y.is_finite()
            || geometry.x.abs() > 32768.0
            || geometry.y.abs() > 32768.0
            || !(1..=32768).contains(&geometry.width)
            || !(1..=32768).contains(&geometry.height)
        {
            return Err(mlua::Error::RuntimeError(
                "invalid layer.popup identity, position, or size (1..32768)".into(),
            ));
        }
        let value = table.raw_get::<Value>("content")?;
        if value.is_nil() {
            return Err(mlua::Error::RuntimeError(
                "layer.popup.content is required".into(),
            ));
        }
        // Keep the popup's own presentation (background, padding, radius) by
        // parsing it as a real popup root. Geometry fields are not popup
        // properties, so only the presentation keys are copied.
        let popup = lua.create_table()?;
        popup.set("type", "popup")?;
        popup.set("child", value)?;
        for key in [
            "width",
            "height",
            "padding",
            "background",
            "radius",
            "border",
            "border_width",
            "align_x",
            "align_y",
            "on_open",
            "on_close",
        ] {
            let value = table.raw_get::<Value>(key)?;
            if value != Value::Nil {
                popup.raw_set(key, value)?;
            }
        }
        let content = parse_node_hovered(&Value::Table(popup), hover)
            .map_err(|e| mlua::Error::RuntimeError(format!("layer.popup.content: {e}")))?;
        Ok(Some(Self { geometry, content }))
    }
}
impl LayerRoot {
    pub fn parse(
        lua: &Lua,
        value: Value,
        hover: &crate::ui::hover::HoverSet,
    ) -> mlua::Result<Option<Self>> {
        if value.is_nil() {
            return Ok(None);
        }
        let Value::Table(table) = value else {
            return Err(mlua::Error::RuntimeError(
                "app:view must return iced.layer({...}) or nil".into(),
            ));
        };
        if table.raw_get::<String>("type")? != "layer" {
            return Err(mlua::Error::RuntimeError(
                "framework app root must be iced.layer({...})".into(),
            ));
        }
        let fields = lua.create_table()?;
        for pair in table.clone().pairs::<String, Value>() {
            let (key, value) = pair?;
            if key != "type" && key != "content" && key != "popup" {
                fields.raw_set(key, value)?;
            }
        }
        let surface: Surface = lua.from_value(Value::Table(fields))?;
        surface.validate().map_err(mlua::Error::RuntimeError)?;
        let value = table.raw_get::<Value>("content")?;
        if value.is_nil() {
            return Err(mlua::Error::RuntimeError(
                "layer.content is required".into(),
            ));
        }
        let content = parse_node_hovered(&value, hover).map_err(mlua::Error::RuntimeError)?;
        let popup = AppPopup::parse(lua, table.raw_get("popup")?, hover)?;
        crate::ui::scene::Lists::validate_roots(
            std::iter::once(&content).chain(popup.iter().map(|p| &p.content)),
        )
        .map_err(mlua::Error::RuntimeError)?;
        Ok(Some(Self {
            surface,
            content,
            popup,
        }))
    }

    /// Every layer an app declares: one root, an array of roots, or nothing.
    ///
    /// `iced.layer{...}` carries its own `type`; a list is a plain array of
    /// them. Ids must be unique across the list because each one owns a native
    /// surface and its own list state.
    pub fn parse_list(
        lua: &Lua,
        value: Value,
        hover: &crate::ui::hover::HoverSet,
    ) -> mlua::Result<Vec<Self>> {
        if value.is_nil() {
            return Ok(Vec::new());
        }
        let Value::Table(table) = &value else {
            return Err(mlua::Error::RuntimeError(
                "app:view must return iced.layer({...}), an array of layers, or nil".into(),
            ));
        };
        if matches!(table.raw_get::<Value>("type")?, Value::String(ref tag) if tag == "layer") {
            return Ok(Self::parse(lua, value, hover)?.into_iter().collect());
        }
        let mut roots = Vec::new();
        for entry in table.clone().sequence_values::<Value>() {
            let entry = entry?;
            let root = Self::parse(lua, entry, hover)?.ok_or_else(|| {
                mlua::Error::RuntimeError("app:view layer entries must not be nil".into())
            })?;
            roots.push(root);
        }
        if roots.is_empty() {
            return Err(mlua::Error::RuntimeError(
                "app:view must return iced.layer({...}) or a non-empty array of layers".into(),
            ));
        }
        let mut ids = HashSet::new();
        for root in &roots {
            if !ids.insert(root.surface.id.as_str()) {
                return Err(mlua::Error::RuntimeError(format!(
                    "duplicate layer id {:?}; every layer needs a unique id",
                    root.surface.id
                )));
            }
        }
        Ok(roots)
    }
}
pub(crate) fn install(lua: &Lua, iced: &Table) -> mlua::Result<()> {
    iced.set(
        "layer",
        lua.create_function(|lua, fields: Table| {
            if !fields.raw_get::<Value>("type")?.is_nil() {
                return Err(mlua::Error::RuntimeError(
                    "iced.layer sets its own type".into(),
                ));
            }
            let root = lua.create_table()?;
            for pair in fields.pairs::<Value, Value>() {
                let (key, value) = pair?;
                root.raw_set(key, value)?;
            }
            root.raw_set("type", "layer")?;
            Ok(root)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_popup_geometry_is_strict_and_content_cannot_contain_surfaces() {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let make = |fields: &str| {
            format!(
                "return iced.layer({{width=100,height=32,content=iced.text('parent'),popup={{{fields}}}}})"
            )
        };
        let source = make("id='menu',x=-5,y=48,width=200,height=100,content=iced.text('menu')");
        let value = lua.load(&source).eval().unwrap();
        let root = LayerRoot::parse(&lua, value, &Default::default())
            .unwrap()
            .unwrap();
        assert_eq!(root.popup.unwrap().geometry.id, "menu");
        for fields in [
            "width=0,height=100,content=iced.text('x')",
            "width=200,content=iced.text('x')",
            "id='../menu',width=200,height=100,content=iced.text('x')",
            "x=math.huge,width=200,height=100,content=iced.text('x')",
            "width=200,height=100,typo=true,content=iced.text('x')",
            "width=200,height=100",
            "width=200,height=100,content=iced.layer({width=1,height=1,content=iced.text('x')})",
        ] {
            let value = lua.load(make(fields)).eval().unwrap();
            assert!(
                LayerRoot::parse(&lua, value, &Default::default()).is_err(),
                "{fields}"
            );
        }
    }

    #[test]
    fn roots_validate_protocol_constraints_and_cannot_nest() {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let value = lua.load("return iced.layer({anchors={top=true,left=true,right=true},height=32,content=iced.text('bar')})").eval().unwrap();
        let root = LayerRoot::parse(&lua, value, &Default::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            root.surface.settings(1, "test".into()).size.to_set(),
            (0, 32)
        );
        for source in [
            "return iced.layer({content=iced.text('x')})",
            "return iced.layer({width=0,height=32,content=iced.text('x')})",
            "return iced.layer({width=100,height=32,keyboard='invalid',content=iced.text('x')})",
            "return iced.layer({width=100,height=32,typo=true,content=iced.text('x')})",
            "return iced.layer({width=100,height=32,content=iced.layer({width=1,height=1,content=iced.text('x')})})",
        ] {
            let value = lua.load(source).eval().unwrap();
            assert!(
                LayerRoot::parse(&lua, value, &Default::default()).is_err(),
                "{source}"
            );
        }
    }
}
