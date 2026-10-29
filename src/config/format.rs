//! Human-facing `config.lua` shapes, separate from the shell's resolved
//! layout fields. Settings saves use compact slot/geometry values rather
//! than nested widget tables; both compact and expanded input forms
//! deserialize identically. Serialization goes through `super::lua`.
use super::schema::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Settings saves use compact slot/geometry values rather than nested widget
/// tables. Both compact and expanded input forms deserialize identically.
pub(super) fn to_string(config: &Config) -> Result<String, String> {
    super::lua::to_string(config)
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct Bar {
    #[serde(skip_serializing_if = "is_top")]
    anchor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    output: String,
    #[serde(skip_serializing_if = "is_full_length")]
    length: f32,
    #[serde(skip_serializing_if = "is_default_thickness")]
    thickness: f32,
    slots: Vec<Slot>,
    #[serde(skip_serializing_if = "is_default")]
    slot_padding: f32,
    #[serde(skip_serializing_if = "is_default_spacing")]
    slot_spacing: f32,
    #[serde(skip_serializing_if = "is_one")]
    opacity: f32,
    #[serde(skip_serializing_if = "is_default")]
    floating: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    margin: Option<Margin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    radius: Option<Radius>,
}
fn is_top(value: &str) -> bool {
    value == "top"
}
fn is_full_length(value: &f32) -> bool {
    *value == 100.0
}
fn is_default_thickness(value: &f32) -> bool {
    *value == 50.0
}
fn is_default_spacing(value: &f32) -> bool {
    *value == 4.0
}
fn is_one(value: &f32) -> bool {
    *value == 1.0
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            anchor: "top".into(),
            output: String::new(),
            length: 100.0,
            thickness: 50.0,
            slots: vec![Slot::default()],
            slot_padding: 0.0,
            slot_spacing: 4.0,
            opacity: 1.0,
            floating: false,
            margin: None,
            radius: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Slot {
    #[serde(skip_serializing_if = "is_center")]
    align: String,
    widgets: Vec<SlotEntry>,
}
fn is_center(value: &str) -> bool {
    value == "center"
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            align: "center".into(),
            widgets: Vec::new(),
        }
    }
}

/// A bare `margin = 8` expands to uniform sides; tables deserialize
/// sparsely and serialize back sparsely (flat legacy keys are grouped
/// strictly by `format_migration` before serde runs).
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Margin {
    Uniform(i32),
    Sides(Sides),
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default, deny_unknown_fields)]
struct Sides {
    #[serde(skip_serializing_if = "is_default")]
    top: i32,
    #[serde(skip_serializing_if = "is_default")]
    right: i32,
    #[serde(skip_serializing_if = "is_default")]
    bottom: i32,
    #[serde(skip_serializing_if = "is_default")]
    left: i32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Radius {
    Uniform(f32),
    Corners(Corners),
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default, deny_unknown_fields)]
struct Corners {
    #[serde(skip_serializing_if = "is_default")]
    top_left: f32,
    #[serde(skip_serializing_if = "is_default")]
    top_right: f32,
    #[serde(skip_serializing_if = "is_default")]
    bottom_left: f32,
    #[serde(skip_serializing_if = "is_default")]
    bottom_right: f32,
}

impl From<Bar> for TopConfig {
    fn from(value: Bar) -> Self {
        let margin = match value.margin {
            Some(Margin::Uniform(n)) => Sides {
                top: n,
                right: n,
                bottom: n,
                left: n,
            },
            Some(Margin::Sides(sides)) => sides,
            None => Sides::default(),
        };
        let radius = match value.radius {
            Some(Radius::Uniform(n)) => Corners {
                top_left: n,
                top_right: n,
                bottom_left: n,
                bottom_right: n,
            },
            Some(Radius::Corners(corners)) => corners,
            None => Corners::default(),
        };
        Self {
            anchor: value.anchor,
            output: value.output,
            length: value.length,
            thickness: value.thickness,
            slots: value.slots.len() as u32,
            aligns: value.slots.iter().map(|slot| slot.align.clone()).collect(),
            widgets: value
                .slots
                .into_iter()
                .map(|slot| SlotWidgets::Many(slot.widgets))
                .collect(),
            slot_padding: value.slot_padding,
            slot_spacing: value.slot_spacing,
            opacity: value.opacity,
            floating: value.floating,
            margin_top: margin.top,
            margin_right: margin.right,
            margin_bottom: margin.bottom,
            margin_left: margin.left,
            radius_top_left: radius.top_left,
            radius_top_right: radius.top_right,
            radius_bottom_left: radius.bottom_left,
            radius_bottom_right: radius.bottom_right,
        }
    }
}
impl From<TopConfig> for Bar {
    fn from(value: TopConfig) -> Self {
        let margins = [
            value.margin_top,
            value.margin_right,
            value.margin_bottom,
            value.margin_left,
        ];
        let radii = [
            value.radius_top_left,
            value.radius_top_right,
            value.radius_bottom_left,
            value.radius_bottom_right,
        ];
        let margin = if margins == [0; 4] {
            None
        } else if margins.iter().all(|n| *n == margins[0]) {
            Some(Margin::Uniform(margins[0]))
        } else {
            Some(Margin::Sides(Sides {
                top: margins[0],
                right: margins[1],
                bottom: margins[2],
                left: margins[3],
            }))
        };
        let radius = if radii == [0.0; 4] {
            None
        } else if radii.iter().all(|n| *n == radii[0]) {
            Some(Radius::Uniform(radii[0]))
        } else {
            Some(Radius::Corners(Corners {
                top_left: radii[0],
                top_right: radii[1],
                bottom_left: radii[2],
                bottom_right: radii[3],
            }))
        };
        let slots = (0..value.slots)
            .map(|index| {
                let index = index as usize;
                let widgets = match value.widgets.get(index) {
                    Some(SlotWidgets::One(name)) => vec![SlotEntry::Name(name.clone())],
                    Some(SlotWidgets::Many(entries)) => entries.clone(),
                    None => Vec::new(),
                };
                Slot {
                    align: value
                        .aligns
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| "center".into()),
                    widgets,
                }
            })
            .collect();
        Self {
            anchor: value.anchor,
            output: value.output,
            length: value.length,
            thickness: value.thickness,
            slots,
            slot_padding: value.slot_padding,
            slot_spacing: value.slot_spacing,
            opacity: value.opacity,
            floating: value.floating,
            margin,
            radius,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Background {
    #[serde(skip_serializing_if = "is_default")]
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    shader: Option<WallpaperShader>,
    #[serde(skip_serializing_if = "is_default")]
    mode: WallpaperMode,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    monitors: BTreeMap<String, Wallpaper>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<BackgroundImage>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Wallpaper {
    path: String,
    #[serde(skip_serializing_if = "is_default")]
    fit: WallpaperFit,
}
impl From<Background> for BackgroundConfig {
    fn from(value: Background) -> Self {
        Self {
            disabled: value.disabled,
            shader: value.shader,
            mode: value.mode,
            image: value.images,
            monitor: value
                .monitors
                .into_iter()
                .map(|(output, wallpaper)| MonitorWallpaper {
                    output,
                    path: wallpaper.path,
                    fit: wallpaper.fit,
                })
                .collect(),
        }
    }
}
impl From<BackgroundConfig> for Background {
    fn from(value: BackgroundConfig) -> Self {
        Self {
            disabled: value.disabled,
            shader: value.shader,
            mode: value.mode,
            images: value.image,
            monitors: value
                .monitor
                .into_iter()
                .map(|wallpaper| {
                    (
                        wallpaper.output,
                        Wallpaper {
                            path: wallpaper.path,
                            fit: wallpaper.fit,
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Shader {
    Path(String),
    Options(ShaderOptions),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ShaderOptions {
    path: String,
    #[serde(default = "shader_speed", skip_serializing_if = "is_one")]
    speed: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    params: Vec<f32>,
    #[serde(
        default = "shader_animated",
        skip_serializing_if = "shader_is_animated"
    )]
    animated: bool,
}

fn shader_speed() -> f32 {
    1.0
}
fn shader_animated() -> bool {
    true
}
fn shader_is_animated(value: &bool) -> bool {
    *value
}

impl TryFrom<Shader> for WallpaperShader {
    type Error = String;

    fn try_from(value: Shader) -> Result<Self, Self::Error> {
        let options = match value {
            Shader::Path(path) => ShaderOptions {
                path,
                speed: 1.0,
                params: Vec::new(),
                animated: true,
            },
            Shader::Options(options) => options,
        };
        if options.path.trim().is_empty() {
            return Err("background.shader needs a non-empty WGSL path".into());
        }
        if !options.speed.is_finite()
            || options.params.len() > 16
            || options.params.iter().any(|n| !n.is_finite())
        {
            return Err("background.shader needs finite speed and at most 16 finite params".into());
        }
        Ok(Self {
            path: options.path,
            speed: options.speed,
            params: options.params,
            animated: options.animated,
        })
    }
}

impl From<WallpaperShader> for Shader {
    fn from(value: WallpaperShader) -> Self {
        if value.speed == 1.0 && value.params.is_empty() && value.animated {
            Self::Path(value.path)
        } else {
            Self::Options(ShaderOptions {
                path: value.path,
                speed: value.speed,
                params: value.params,
                animated: value.animated,
            })
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Image {
    path: String,
    #[serde(skip_serializing_if = "is_default", deserialize_with = "position")]
    position: [f32; 2],
    #[serde(skip_serializing_if = "is_default")]
    z: i32,
    #[serde(skip_serializing_if = "is_one")]
    scale: f32,
    #[serde(skip_serializing_if = "is_default")]
    width: f32,
    #[serde(skip_serializing_if = "is_default")]
    height: f32,
}
fn position<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<[f32; 2], D::Error> {
    let values = Vec::<f32>::deserialize(deserializer)?;
    values.try_into().map_err(|_: Vec<f32>| {
        serde::de::Error::custom("position must contain exactly two numbers: [x, y]")
    })
}
impl Default for Image {
    fn default() -> Self {
        Self {
            path: String::new(),
            position: [0.0; 2],
            z: 0,
            scale: 1.0,
            width: 0.0,
            height: 0.0,
        }
    }
}
impl From<Image> for BackgroundImage {
    fn from(value: Image) -> Self {
        Self {
            path: value.path,
            x: value.position[0],
            y: value.position[1],
            z: value.z,
            scale: value.scale,
            width: value.width,
            height: value.height,
        }
    }
}
impl From<BackgroundImage> for Image {
    fn from(value: BackgroundImage) -> Self {
        Self {
            path: value.path,
            position: [value.x, value.y],
            z: value.z,
            scale: value.scale,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ThemeMode {
    #[default]
    Dark,
    Light,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub(super) struct Theme {
    name: String,
    #[serde(deserialize_with = "icon_theme_name")]
    icon_theme: String,
    mode: ThemeMode,
    #[serde(skip_serializing_if = "is_content")]
    variant: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    templates_dir: String,
    #[serde(skip_serializing_if = "crate::theme::palette::PaletteOverrides::is_empty")]
    palette: crate::theme::palette::PaletteOverrides,
}
fn icon_theme_name<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let name = String::deserialize(deserializer)?;
    if name.is_empty() || matches!(name.as_str(), "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err(serde::de::Error::custom(
            "icon-theme must be a theme directory name",
        ));
    }
    Ok(name)
}
fn is_content(value: &str) -> bool {
    value == "content"
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "gruvbox".into(),
            icon_theme: "hicolor".into(),
            mode: ThemeMode::Dark,
            variant: "content".into(),
            templates_dir: String::new(),
            palette: Default::default(),
        }
    }
}
impl From<Theme> for ThemeConfig {
    fn from(value: Theme) -> Self {
        Self {
            name: value.name,
            icon_theme: value.icon_theme,
            darkmode: matches!(value.mode, ThemeMode::Dark),
            variant: value.variant,
            templates_dir: value.templates_dir,
            palette: value.palette,
        }
    }
}
impl From<ThemeConfig> for Theme {
    fn from(value: ThemeConfig) -> Self {
        Self {
            name: value.name,
            icon_theme: value.icon_theme,
            mode: if value.darkmode {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            variant: value.variant,
            templates_dir: value.templates_dir,
            palette: value.palette,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_format_config as save;
    use crate::config::test_parse_config as parse;

    #[test]
    fn palette_overrides_roundtrip_and_reject_invalid_fields_and_colors() {
        let source =
            "return {theme={palette={primary='#89B4FA',on_primary='#11223344',surface='#181825'}}}";
        let config = parse(source).unwrap();
        let saved = save(&config).unwrap();
        assert!(saved.contains("palette"));
        assert!(saved.contains("#89b4fa"));
        assert!(saved.contains("#11223344"));
        assert_eq!(parse(&saved).unwrap().theme, config.theme);
        for source in [
            "return {theme={palette={typo='#ffffff'}}}",
            "return {theme={palette={primary='not-a-color'}}}",
            "return {theme={palette={primary=42}}}",
        ] {
            assert!(parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn palette_patches_merge_keep_mode_overrides_and_named_selection_resets_them() {
        let mut config = Config::default();
        config.apply(ConfigPatch::ThemePalette(
            serde_json::from_value(serde_json::json!({"primary":"#123456"})).unwrap(),
        ));
        config.apply(ConfigPatch::ThemePalette(
            serde_json::from_value(serde_json::json!({"surface":"#181825"})).unwrap(),
        ));
        let custom = config.theme.palette.clone();
        config.apply(ConfigPatch::ThemeDarkmode(false));
        assert_eq!(config.theme.palette, custom);
        let restored = parse(&save(&config).unwrap()).unwrap();
        assert_eq!(restored.theme, config.theme);
        let colors: serde_json::Value = serde_json::to_value(&restored.theme.palette).unwrap();
        assert_eq!(colors["primary"], "#123456");
        assert_eq!(colors["surface"], "#181825");
        config.apply(ConfigPatch::ThemeName(config.theme.name.clone()));
        assert!(config.theme.palette.is_empty());
        assert!(!save(&config).unwrap().contains("palette"));
    }

    #[test]
    fn icon_theme_selection_defaults_patches_and_roundtrips() {
        let mut config: Config = parse("return {}").unwrap();
        assert_eq!(config.theme.icon_theme, "hicolor");
        config.apply(ConfigPatch::IconTheme("Papirus-Dark".into()));
        let saved = to_string(&config).unwrap();
        assert!(saved.contains("icon_theme = \"Papirus-Dark\""));
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(reloaded.theme.icon_theme, "Papirus-Dark");
        for name in ["", "..", "../Papirus", "/tmp/icons", r"some\theme"] {
            let escaped = name.replace('\\', "\\\\");
            let source = format!("return {{ theme = {{ icon_theme = '{escaped}' }} }}");
            assert!(parse(&source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn extensions_roundtrip_and_reject_invalid_names() {
        let config: Config = parse(
            "return { extensions = { { name = 'palette' }, { name = 'custom', path = 'vendor/custom' } } }",
        )
        .unwrap();
        let saved = save(&config).unwrap();
        assert!(saved.contains("extensions"));
        assert!(saved.contains("name = \"palette\""));
        assert!(saved.contains("path = \"vendor/custom\""));
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(reloaded.extensions, config.extensions);
        for source in [
            "return { extensions = { { name = 'Bad' } } }",
            "return { extensions = { { name = 'demo' }, { name = 'demo' } } }",
            "return { extensions = { { name = 'demo', unknown = true } } }",
        ] {
            assert!(parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn identical_slot_layouts_share_saved_form_and_omit_defaults() {
        let source = "return { bar = { { output = 'DP-1', slots = { { align = 'start', widgets = { 'clock', { name = 'stats', size = 17 } } }, { align = 'end', widgets = {} } } } } }";
        let config: Config = parse(source).unwrap();
        assert_eq!(config.bar[0].slots, 2);
        assert_eq!(config.bar[0].aligns, ["start", "end"]);
        let saved = save(&config).unwrap();
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            serde_json::to_value(&reloaded).unwrap()
        );
        assert!(
            matches!(&reloaded.bar[0].widgets[1], SlotWidgets::Many(entries) if entries.is_empty())
        );
        assert!(!saved.contains("aligns ="));
        assert!(!saved.contains("interval ="));
        assert!(!saved.contains("floating ="));
        assert!(!saved.contains("margin"));
        assert!(saved.contains("slots = {"));
    }

    #[test]
    fn grouped_and_uniform_geometry_roundtrip_without_swapping_corners() {
        let config: Config = parse(
            "return { bar = { { margin = { top = -2, right = 3, bottom = 4, left = 5 }, radius = { top_left = 1, top_right = 2, bottom_left = 3, bottom_right = 4 } }, { margin = 8, radius = 10 } } }",
        )
        .unwrap();
        let bar = &config.bar[0];
        assert_eq!(
            [
                bar.margin_top,
                bar.margin_right,
                bar.margin_bottom,
                bar.margin_left
            ],
            [-2, 3, 4, 5]
        );
        assert_eq!(
            [
                bar.radius_top_left,
                bar.radius_top_right,
                bar.radius_bottom_left,
                bar.radius_bottom_right
            ],
            [1.0, 2.0, 3.0, 4.0]
        );
        let saved = save(&config).unwrap();
        assert!(saved.contains("margin = 8"));
        assert!(saved.contains("radius = 10"));
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            serde_json::to_value(&reloaded).unwrap()
        );
    }

    #[test]
    fn keyed_wallpapers_keep_both_recipes_and_native_image_defaults() {
        let config: Config = parse(
            "return { background = { mode = 'per_monitor', monitors = { ['DP-2'] = { path = 'portrait.png', fit = 'stretch' }, ['DP-1'] = { path = 'landscape.png' } }, images = { { path = 'free-form.png', position = { -291.53717, -132.1731 }, z = 1 } } }, theme = { mode = 'light', templates_dir = 'off' }, notifications = { timeout_ms = 9000 } }",
        )
        .unwrap();
        assert_eq!(config.background.monitor[0].output, "DP-1");
        assert_eq!(config.background.monitor[1].fit, WallpaperFit::Stretch);
        assert_eq!(config.background.image[0].width, 0.0);
        assert!(!config.theme.darkmode);
        assert_eq!(config.notifications.timeout_ms, 9000);
        let saved = save(&config).unwrap();
        assert!(!saved.contains("width ="));
        assert!(!saved.contains("scale ="));
        let reloaded: Config = parse(&saved).unwrap();
        assert_eq!(reloaded.background, config.background);
    }

    #[test]
    fn wallpaper_shaders_accept_shorthand_and_options_and_roundtrip() {
        for shader in [
            "'/shaders/water.wgsl'",
            "{path='water',speed=-0.5,params={0.01,2},animated=false}",
        ] {
            let config: Config =
                parse(&format!("return {{background={{shader={shader}}}}}")).unwrap();
            let saved = save(&config).unwrap();
            let restored: Config = parse(&saved).unwrap();
            assert_eq!(restored.background, config.background);
        }
        let config: Config = parse("return {background={shader='water'}}").unwrap();
        let effect = config.background.shader.unwrap();
        assert_eq!(effect.speed, 1.0);
        assert!(effect.animated && effect.params.is_empty());
        let default_config = Config::default();
        assert!(!save(&default_config).unwrap().contains("shader ="));
    }

    #[test]
    fn wallpaper_shader_options_reject_invalid_values() {
        for shader in [
            "42",
            "true",
            "''",
            "{}",
            "{path='water',typo=1}",
            "{path='water',speed=0/0}",
            "{path='water',params={1/0}}",
            "{path='water',params={1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17}}",
            "{path='water',params={x=1}}",
            "{path='water',animated=1}",
        ] {
            assert!(
                parse(&format!("return {{background={{shader={shader}}}}}")).is_err(),
                "{shader}"
            );
        }
    }

    #[test]
    fn malformed_structured_fields_fail_instead_of_being_ignored() {
        for source in [
            "return { bar = { { margin = { sideways = 8 } } } }",
            "return { bar = { { slots = { { align = 'start', widgets = 4 } } } } } }",
            "return { background = { images = { { position = { 1, 2, 3 } } } } }",
            "return { background = { monitors = { ['DP-1'] = { fit = 'squish' } } } } }",
            "return { theme = { mode = 'bright' } }",
        ] {
            assert!(
                crate::config::lua::parse::<Config>(source, "test").is_err(),
                "accepted {source}"
            );
        }
    }
}
