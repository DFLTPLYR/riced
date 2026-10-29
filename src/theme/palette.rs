//! Validated persistent palette overrides and the native Lua palette value.
use super::ActiveTheme;
use iced::Color;
use mlua::{Lua, Table, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Partial color overrides over the selected named theme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<String, String>",
    into = "BTreeMap<String, String>"
)]
pub struct PaletteOverrides(BTreeMap<String, String>);

impl mlua::UserData for PaletteOverrides {}

impl TryFrom<BTreeMap<String, String>> for PaletteOverrides {
    type Error = String;

    fn try_from(colors: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        let mut normalized = BTreeMap::new();
        let roles = snapshot(&ActiveTheme::fallback());
        for (name, value) in colors {
            if !roles.iter().any(|(key, _)| *key == name) {
                return Err(format!("unknown palette color {name:?}"));
            }
            let color = super::parse_hex(&value)
                .ok_or_else(|| format!("palette.{name} must be #rrggbb or #rrggbbaa"))?;
            normalized.insert(name, color_hex(color));
        }
        Ok(Self(normalized))
    }
}

impl From<PaletteOverrides> for BTreeMap<String, String> {
    fn from(value: PaletteOverrides) -> Self {
        value.0
    }
}

impl PaletteOverrides {
    /// Whether there are no explicit overrides.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn merge(&mut self, patch: Self) {
        self.0.extend(patch.0);
    }

    pub(crate) fn contains_patch(&self, patch: &Self) -> bool {
        patch
            .0
            .iter()
            .all(|(key, value)| self.0.get(key) == Some(value))
    }

    pub(crate) fn apply(&self, palette: &mut ActiveTheme) {
        for (name, value) in &self.0 {
            let Some(color) = super::parse_hex(value) else {
                continue;
            };
            match name.as_str() {
                "background" => palette.background = color,
                "on_background" => palette.on_background = color,
                "primary" => palette.primary = color,
                "on_primary" => palette.on_primary = color,
                "secondary" => palette.secondary = color,
                "on_secondary" => palette.on_secondary = color,
                "tertiary" => palette.tertiary = color,
                "on_tertiary" => palette.on_tertiary = color,
                "success" => palette.success = color,
                "on_success" => palette.on_success = color,
                "warning" => palette.warning = color,
                "on_warning" => palette.on_warning = color,
                "error" => palette.error = color,
                "on_error" => palette.on_error = color,
                "surface" => palette.surface = color,
                "on_surface" => palette.on_surface = color,
                "surface_variant" => palette.surface_variant = color,
                "on_surface_variant" => palette.on_surface_variant = color,
                "outline" => palette.outline = color,
                "shadow" => palette.shadow = color,
                "hover" => palette.hover = color,
                "on_hover" => palette.on_hover = color,
                _ => unreachable!("palette keys are validated at construction"),
            }
        }
    }
}

fn color_hex(color: Color) -> String {
    let mut hex = super::lua::hex(color);
    if color.a < 1.0 {
        hex.push_str(&format!("{:02x}", (color.a * 255.0).round() as u8));
    }
    hex
}

pub(crate) fn snapshot(p: &ActiveTheme) -> Vec<(&'static str, String)> {
    [
        ("background", p.background),
        ("on_background", p.on_background),
        ("primary", p.primary),
        ("on_primary", p.on_primary),
        ("secondary", p.secondary),
        ("on_secondary", p.on_secondary),
        ("tertiary", p.tertiary),
        ("on_tertiary", p.on_tertiary),
        ("success", p.success),
        ("on_success", p.on_success),
        ("warning", p.warning),
        ("on_warning", p.on_warning),
        ("error", p.error),
        ("on_error", p.on_error),
        ("surface", p.surface),
        ("on_surface", p.on_surface),
        ("surface_variant", p.surface_variant),
        ("on_surface_variant", p.on_surface_variant),
        ("outline", p.outline),
        ("shadow", p.shadow),
        ("hover", p.hover),
        ("on_hover", p.on_hover),
    ]
    .into_iter()
    .map(|(key, color)| (key, color_hex(color)))
    .collect()
}

fn parse(value: Value) -> mlua::Result<PaletteOverrides> {
    let Value::Table(table) = value else {
        return Err(mlua::Error::RuntimeError(
            "iced.palette expects a color table".into(),
        ));
    };
    let mut colors = BTreeMap::new();
    for pair in table.pairs::<Value, Value>() {
        let (Value::String(key), Value::String(value)) = pair? else {
            return Err(mlua::Error::RuntimeError(
                "palette keys and colors must be strings".into(),
            ));
        };
        colors.insert(key.to_str()?.to_owned(), value.to_str()?.to_owned());
    }
    PaletteOverrides::try_from(colors).map_err(mlua::Error::RuntimeError)
}

pub(crate) fn install(lua: &Lua, iced: &Table) -> mlua::Result<()> {
    if lua.app_data_ref::<ActiveTheme>().is_none() {
        lua.set_app_data(super::active());
    }
    iced.set(
        "palette",
        lua.create_function(|lua, args: mlua::MultiValue| {
            if args.is_empty() {
                let active = lua
                    .app_data_ref::<ActiveTheme>()
                    .map(|p| *p)
                    .unwrap_or_else(ActiveTheme::fallback);
                return lua.create_table_from(snapshot(&active)).map(Value::Table);
            }
            if args.len() != 1 {
                return Err(mlua::Error::RuntimeError(
                    "iced.palette expects zero arguments or one color table".into(),
                ));
            }
            let patch = parse(args.into_iter().next().unwrap_or(Value::Nil))?;
            lua.create_userdata(patch).map(Value::UserData)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_snapshots_are_owned_and_constructors_do_not_apply_colors() {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let active = ActiveTheme {
            primary: Color::from_rgb8(1, 2, 3),
            ..ActiveTheme::fallback()
        };
        lua.set_app_data(active);
        lua.load(
            r##"
            local colors = iced.palette()
            assert(colors.primary == '#010203')
            colors.primary = '#ffffff'
            theme = {primary='#000000'}
            assert(iced.palette().primary == '#010203')
            pending = iced.palette({primary='#89B4FA', on_primary='#11223344'})
            assert(iced.palette().primary == '#010203')
            assert(not pcall(iced.define, 'palette', function() return iced.text('bad') end))
        "##,
        )
        .exec()
        .unwrap();
        let command: Value = lua.globals().get("pending").unwrap();
        let Value::UserData(patch) = &command else {
            panic!("expected a native palette value");
        };
        assert_eq!(
            patch.borrow::<PaletteOverrides>().unwrap().0["primary"],
            "#89b4fa"
        );
        assert!(crate::ui::decode::parse_node(&command).is_err());
        assert!(
            crate::framework::LayerRoot::parse_list(&lua, command, &Default::default()).is_err()
        );
        assert!(lua.load("pending.primary = 'invalid'").exec().is_err());
    }

    #[test]
    fn palette_validation_rejects_unknown_keys_wrong_types_and_bad_colors_atomically() {
        let lua = Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        for expression in [
            "iced.palette(nil)",
            "iced.palette({}, {})",
            "iced.palette('dark')",
            "iced.palette({primary='#ffffff',typo='#000000'})",
            "iced.palette({primary=42})",
            "iced.palette({[1]='#ffffff'})",
            "iced.palette({primary='#fff'})",
            "iced.palette({primary='#gggggg'})",
            "iced.palette({primary={1,0,0}})",
        ] {
            assert!(
                lua.load(format!("return {expression}"))
                    .eval::<Value>()
                    .is_err(),
                "{expression}"
            );
        }
    }

    #[test]
    fn palette_extension_generated_colors_fit_the_host_schema() {
        let mut registry = riced_extension_sdk::Registry::default();
        riced_extension_palette::register(&mut registry, &[]).unwrap();
        for dark in [false, true] {
            let output = registry
                .request(
                    "palette",
                    "generate",
                    serde_json::json!({"seed":"#89b4fa","dark":dark}),
                )
                .unwrap()
                .execute(&Default::default())
                .unwrap();
            let patch: PaletteOverrides =
                serde_json::from_value(output.data["colors"].clone()).unwrap();
            assert_eq!(patch.0.len(), snapshot(&ActiveTheme::fallback()).len());
            let mut colors = ActiveTheme::fallback();
            patch.apply(&mut colors);
            assert_eq!(
                color_hex(colors.primary),
                output.data["colors"]["primary"].as_str().unwrap()
            );
        }
    }
}
