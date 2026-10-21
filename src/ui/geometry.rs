//! Shared Lua geometry: a uniform number or a strict named four-value table.
use mlua::{IntoLua, Lua, Table, Value};

macro_rules! quad {
    ($name:ident { $a:ident, $b:ident, $c:ident, $d:ident }) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq)]
        pub(crate) struct $name {
            pub $a: f32,
            pub $b: f32,
            pub $c: f32,
            pub $d: f32,
        }
        impl $name {
            const KEYS: [&'static str; 4] = [
                stringify!($a),
                stringify!($b),
                stringify!($c),
                stringify!($d),
            ];
            pub(crate) const ZERO: Self = Self::uniform(0.0);
            pub(crate) const fn uniform(value: f32) -> Self {
                Self {
                    $a: value,
                    $b: value,
                    $c: value,
                    $d: value,
                }
            }
        }
        impl From<f32> for $name {
            fn from(value: f32) -> Self {
                Self::uniform(value)
            }
        }
        impl From<[f32; 4]> for $name {
            fn from([$a, $b, $c, $d]: [f32; 4]) -> Self {
                Self { $a, $b, $c, $d }
            }
        }
        impl IntoLua for $name {
            fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
                let values = [self.$a, self.$b, self.$c, self.$d];
                if values.iter().all(|value| *value == values[0]) {
                    return Ok(Value::Number(f64::from(values[0])));
                }
                let table = lua.create_table()?;
                for (key, value) in Self::KEYS.into_iter().zip(values) {
                    table.set(key, value)?;
                }
                Ok(Value::Table(table))
            }
        }
    };
}

quad!(Insets {
    top,
    right,
    bottom,
    left
});
quad!(Corners {
    top_left,
    top_right,
    bottom_right,
    bottom_left
});

impl Insets {
    pub(crate) fn horizontal(self) -> f32 {
        self.left + self.right
    }
    pub(crate) fn vertical(self) -> f32 {
        self.top + self.bottom
    }

    /// Fit each edge pair proportionally when margins consume the whole axis.
    /// Unbounded axes keep their authored insets for intrinsic measurement.
    pub(crate) fn fitted(self, size: iced::Size) -> Self {
        fn pair(start: f32, end: f32, extent: f32) -> (f32, f32) {
            let sum = f64::from(start) + f64::from(end);
            if extent.is_finite() && sum > f64::from(extent.max(0.0)) {
                let scale = f64::from(extent.max(0.0)) / sum;
                (
                    (f64::from(start) * scale) as f32,
                    (f64::from(end) * scale) as f32,
                )
            } else {
                (start, end)
            }
        }
        let (left, right) = pair(self.left, self.right, size.width);
        let (top, bottom) = pair(self.top, self.bottom, size.height);
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}

impl From<Insets> for iced::Padding {
    fn from(value: Insets) -> Self {
        Self {
            top: value.top,
            right: value.right,
            bottom: value.bottom,
            left: value.left,
        }
    }
}

impl From<Corners> for iced::border::Radius {
    fn from(value: Corners) -> Self {
        Self {
            top_left: value.top_left,
            top_right: value.top_right,
            bottom_right: value.bottom_right,
            bottom_left: value.bottom_left,
        }
    }
}

pub(crate) fn opt_insets(table: &Table, field: &str, what: &str) -> Result<Option<Insets>, String> {
    let value = optional(table, field, what, Insets::KEYS)?.map(Insets::from);
    if value.is_some_and(|v| !v.horizontal().is_finite() || !v.vertical().is_finite()) {
        return Err(format!("{what} {field} edge totals must be finite"));
    }
    Ok(value)
}

pub(crate) fn opt_corners(
    table: &Table,
    field: &str,
    what: &str,
) -> Result<Option<Corners>, String> {
    optional(table, field, what, Corners::KEYS).map(|value| value.map(Corners::from))
}

fn optional(
    table: &Table,
    field: &str,
    what: &str,
    keys: [&str; 4],
) -> Result<Option<[f32; 4]>, String> {
    let value = super::decode::node_property(table, field).map_err(|e| e.to_string())?;
    let number = |value: &Value| -> Result<f32, String> {
        let value = match value {
            Value::Integer(value) => *value as f32,
            Value::Number(value) => *value as f32,
            _ => return Err(format!("{what} {field} values must be numbers")),
        };
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "{what} {field} values must be finite and non-negative"
            ));
        }
        Ok(value)
    };
    match value {
        Value::Nil => Ok(None),
        Value::Integer(_) | Value::Number(_) => Ok(Some([number(&value)?; 4])),
        Value::Table(table) => {
            let mut values = [0.0; 4];
            for pair in table.pairs::<Value, Value>() {
                let (key, value) = pair.map_err(|e| e.to_string())?;
                let Value::String(key) = key else {
                    return Err(format!(
                        "{what} {field} expects named keys: {}",
                        keys.join(", ")
                    ));
                };
                let key = key.to_string_lossy();
                let index = keys.iter().position(|known| *known == key).ok_or_else(|| {
                    format!(
                        "{what} {field} unknown key {key:?}; expected {}",
                        keys.join(", ")
                    )
                })?;
                values[index] = number(&value)?;
            }
            Ok(Some(values))
        }
        _ => Err(format!(
            "{what} {field} must be a number or a named table ({})",
            keys.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{decode::parse_node, node::WidgetNode};

    fn lua() -> Lua {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        lua
    }

    #[test]
    fn geometry_tables_roundtrip_through_every_supported_component() {
        let lua = lua();
        for source in [
            "iced.container(iced.space()):padding({top=2,right=3,bottom=5,left=7}):radius({top_left=11,bottom_right=13})",
            "iced.popup(iced.text('body')):padding({left=12}):radius({top_right=8})",
            "iced.button('Go','go'):padding({top=3,left=8}):radius({bottom_left=4})",
            "iced.text_input('Search',''):padding({top=1,bottom=5})",
            "iced.pick_list({'one'},'one'):padding({left=4,right=8})",
            "iced.text('Corner'):anchor('top_right'):margin({top=7,right=11})",
            "iced.container(iced.space()):padding(6):radius(4)",
            "iced.container(iced.space()):padding({}):radius({})",
        ] {
            let node = parse_node(&lua.load(format!("return {source}")).eval().unwrap()).unwrap();
            let table = crate::ui::encode::to_table(&lua, &node).unwrap();
            assert_eq!(parse_node(&Value::Table(table)).unwrap(), node, "{source}");
        }
        let node = parse_node(
            &lua.load(
                "return iced.container(iced.space()):padding({left=12}):radius({top_left=8})",
            )
            .eval()
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            node,
            WidgetNode::Container {
                padding: Insets {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 12.0
                },
                radius: Corners {
                    top_left: 8.0,
                    top_right: 0.0,
                    bottom_right: 0.0,
                    bottom_left: 0.0
                },
                ..
            }
        ));
    }

    #[test]
    fn geometry_rejects_unknown_keys_arrays_and_invalid_numbers() {
        let lua = lua();
        for setter in [
            "padding({left=-1})",
            "padding(-1)",
            "padding({right=1/0})",
            "padding({top=0/0})",
            "padding({Left=2})",
            "padding({1,2})",
            "padding({left='12'})",
            "padding(true)",
            "padding(function() end)",
            "padding({left=1e100})",
            "padding({left=3e38,right=3e38})",
            "radius({left=8})",
            "radius({topLeft=8})",
            "radius('8')",
            "radius({top_left=false})",
            "radius({bottom_left=-2})",
            "margin({horizontal=4})",
            "margin({top=-1})",
            "margin({4,8})",
        ] {
            let value = lua
                .load(format!(
                    "return iced.container(iced.space()):anchor('center'):{setter}"
                ))
                .eval()
                .unwrap();
            assert!(parse_node(&value).is_err(), "{setter}");
        }
        let value = lua
            .load("return {type='container', child=iced.space(), padding=function() end}")
            .eval()
            .unwrap();
        assert!(parse_node(&value).is_err());
    }

    #[test]
    fn asymmetric_corners_survive_every_button_status() {
        let corners = Corners {
            top_left: 2.0,
            top_right: 4.0,
            bottom_right: 6.0,
            bottom_left: 8.0,
        };
        for status in [
            iced::widget::button::Status::Active,
            iced::widget::button::Status::Hovered,
            iced::widget::button::Status::Pressed,
            iced::widget::button::Status::Disabled,
        ] {
            assert_eq!(
                crate::theme::menu_button(corners)(&iced::Theme::Dark, status)
                    .border
                    .radius,
                corners.into()
            );
            assert_eq!(
                crate::theme::menu_button_tinted(corners, iced::Color::WHITE)(
                    &iced::Theme::Dark,
                    status
                )
                .border
                .radius,
                corners.into()
            );
        }
    }
}
