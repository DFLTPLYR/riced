//! File-backed WGSL fragments. Owned sources and GPU state never retain Lua.
mod gpu;
#[cfg(test)]
mod tests;

use super::{
    decode::node_property,
    node::{NodeLength, WidgetNode},
};
use iced::{
    Element, Event, Length, Rectangle, Size,
    advanced::{self, Widget, layout, widget::Tree},
    mouse,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, Instant},
};

const PRELUDE: &str = include_str!("shader/prelude.wgsl");
const PASSTHROUGH: &str = "fn shade(uv: vec2<f32>) -> vec4<f32> { return sample_image(uv); }";

fn passthrough_code() -> Arc<str> {
    static CODE: LazyLock<Arc<str>> = LazyLock::new(|| format!("{PRELUDE}\n{PASSTHROUGH}").into());
    CODE.clone()
}
const RELOAD_INTERVAL: Duration = Duration::from_millis(500);
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
type SourceKey = (PathBuf, bool);
static SOURCES: LazyLock<Mutex<HashMap<SourceKey, Weak<Asset>>>> = LazyLock::new(Mutex::default);

#[derive(Debug)]
struct Asset {
    path: PathBuf,
    state: Mutex<AssetState>,
}
#[derive(Debug)]
struct AssetState {
    checked: Instant,
    stamp: Option<crate::shared::files::FileStamp>,
    code: Arc<str>,
}

/// Identity is the resolved file, not its revision: native redraws hot-reload
/// shader content without rebuilding Lua views or resetting list transitions.
#[derive(Debug, Clone)]
pub(crate) struct Source(Arc<Asset>);
impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        self.path() == other.path()
    }
}
impl Source {
    pub(crate) fn path(&self) -> &Path {
        &self.0.path
    }

    fn open(path: PathBuf) -> Result<Self, String> {
        Self::open_inner(path, false)
    }

    fn open_inner(path: PathBuf, fallback: bool) -> Result<Self, String> {
        let mut sources = SOURCES.lock().map_err(|error| error.to_string())?;
        let key = (path.clone(), fallback);
        if let Some(asset) = sources.get(&key).and_then(Weak::upgrade) {
            return Ok(Self(asset));
        }
        let code = match read(&path) {
            Ok(code) => code,
            Err(error) if fallback => {
                tracing::warn!("{error}; displaying the original wallpaper");
                passthrough_code()
            }
            Err(error) => return Err(error),
        };
        let asset = Arc::new(Asset {
            state: Mutex::new(AssetState {
                checked: Instant::now(),
                stamp: crate::shared::files::stamp(&path),
                code,
            }),
            path: path.clone(),
        });
        sources.retain(|_, asset| asset.strong_count() > 0);
        sources.insert(key, Arc::downgrade(&asset));
        Ok(Self(asset))
    }

    fn code(&self, now: Instant) -> Arc<str> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if now.saturating_duration_since(state.checked) >= RELOAD_INTERVAL {
            state.checked = now;
            let stamp = crate::shared::files::stamp(self.path());
            if state.stamp != stamp {
                state.stamp = stamp;
                match read(self.path()) {
                    Ok(code) => state.code = code,
                    Err(error) => tracing::error!("{error}; keeping the last valid shader"),
                }
            }
        }
        state.code.clone()
    }
}

fn read(path: &Path) -> Result<Arc<str>, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("shader {}: {error}", path.display()))?;
    validate(&source).map_err(|error| format!("shader {}: {error}", path.display()))
}

fn validate(fragment: &str) -> Result<Arc<str>, String> {
    let code = format!("{PRELUDE}\n// User fragment begins here.\n{fragment}");
    let module =
        naga::front::wgsl::parse_str(&code).map_err(|error| error.emit_to_string(&code))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|error| error.emit_to_string(&code))?;
    // The host provides precisely this bind group and these entry points.
    if module
        .global_variables
        .iter()
        .filter(|(_, var)| var.binding.is_some())
        .count()
        != 3
        || module.entry_points.len() != 2
    {
        return Err("provide only fn shade(uv: vec2<f32>) -> vec4<f32>; extra bindings and entry points are unsupported".into());
    }
    Ok(code.into())
}

fn resolve_in(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("iced.shader() needs a non-empty WGSL path".into());
    }
    let mut resolved = if let Some(file) = path.strip_prefix("file://") {
        if !Path::new(file).is_absolute() {
            return Err("shader file:// paths must be absolute".into());
        }
        PathBuf::from(file)
    } else if let Some(relative) = path.strip_prefix("/shaders/") {
        if relative.is_empty() {
            return Err("iced.shader() needs a WGSL filename".into());
        }
        root.join(relative)
    } else if let Some(relative) = path.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or("cannot resolve shader home directory")?
            .join(relative)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    if resolved.extension().is_none() {
        resolved.set_extension("wgsl");
    }
    if resolved.extension().is_none_or(|ext| ext != "wgsl") {
        return Err("iced.shader() accepts only .wgsl files".into());
    }
    Ok(resolved)
}

pub(crate) fn parse(table: &mlua::Table) -> Result<WidgetNode, String> {
    use mlua::Value;
    const WHAT: &str = "iced.shader()";
    // Shader descriptions are strict even when authored as plain Lua tables.
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|error| error.to_string())?;
        if !key.as_string().is_some_and(|key| {
            matches!(
                key.to_string_lossy().as_str(),
                "type"
                    | "path"
                    | "speed"
                    | "params"
                    | "animated"
                    | "width"
                    | "height"
                    | "anchor"
                    | "margin"
                    | "on_press"
                    | "animate"
                    | "_animation_owner"
                    | "_properties"
                    | "_defaults"
            )
        }) {
            return Err(format!("{WHAT} unknown field {key:?}"));
        }
    }
    let properties = match table
        .raw_get::<Value>("_properties")
        .map_err(|error| error.to_string())?
    {
        Value::Nil => None,
        Value::Table(properties) => Some(properties),
        _ => return Err(format!("{WHAT} _properties must be a table")),
    };
    if let Some(properties) = properties {
        for pair in properties.pairs::<String, Value>() {
            let (key, _) = pair.map_err(|error| error.to_string())?;
            if !matches!(
                key.as_str(),
                "speed"
                    | "params"
                    | "animated"
                    | "width"
                    | "height"
                    | "anchor"
                    | "margin"
                    | "on_press"
                    | "animate"
            ) {
                return Err(format!("{WHAT} unknown property {key:?}"));
            }
        }
    }
    let Value::String(path) = node_property(table, "path").map_err(|error| error.to_string())?
    else {
        return Err(format!("{WHAT} needs a WGSL path string"));
    };
    let speed = match node_property(table, "speed").map_err(|error| error.to_string())? {
        Value::Nil => 1.0,
        Value::Integer(value) => value as f32,
        Value::Number(value) => value as f32,
        _ => return Err(format!("{WHAT} speed must be a number")),
    };
    if !speed.is_finite() {
        return Err(format!("{WHAT} speed must be finite"));
    }
    let animated = match node_property(table, "animated").map_err(|error| error.to_string())? {
        Value::Nil => true,
        Value::Boolean(value) => value,
        _ => return Err(format!("{WHAT} animated must be a boolean")),
    };
    let params = match node_property(table, "params").map_err(|error| error.to_string())? {
        Value::Nil => Vec::new(),
        Value::Table(values) => {
            let mut params = Vec::new();
            for pair in values.pairs::<Value, Value>() {
                let (key, value) = pair.map_err(|error| error.to_string())?;
                let Value::Integer(index) = key else {
                    return Err(format!(
                        "{WHAT} params must be a dense array of at most 16 finite numbers"
                    ));
                };
                if !(1..=16).contains(&index) {
                    return Err(format!("{WHAT} params indices must be 1..16"));
                }
                let number = match value {
                    Value::Integer(n) => n as f32,
                    Value::Number(n) => n as f32,
                    _ => return Err(format!("{WHAT} params must contain finite numbers")),
                };
                if !number.is_finite() {
                    return Err(format!("{WHAT} params must contain finite numbers"));
                }
                params.push((index as usize, number));
            }
            params.sort_by_key(|(index, _)| *index);
            if params
                .iter()
                .enumerate()
                .any(|(index, (key, _))| index + 1 != *key)
            {
                return Err(format!("{WHAT} params must be a dense array"));
            }
            params.into_iter().map(|(_, value)| value).collect()
        }
        _ => return Err(format!("{WHAT} params must be an array")),
    };
    let dimension = |field| -> Result<NodeLength, String> {
        if matches!(
            node_property(table, field).map_err(|error| error.to_string())?,
            Value::Function(_)
        ) {
            return Err(format!("{WHAT} {field} must be a size"));
        }
        let length = super::decode::opt_length(table, field, WHAT)?.unwrap_or(NodeLength::Native);
        if matches!(length, NodeLength::Fixed(n) if !n.is_finite() || n <= 0.0) {
            return Err(format!("{WHAT} {field} must be positive and finite"));
        }
        Ok(length)
    };
    let width = dimension("width")?;
    let height = dimension("height")?;
    let source = Source::open(resolve_in(
        &crate::config::shaders_dir(),
        &path.to_string_lossy(),
    )?)?;
    Ok(WidgetNode::Shader {
        source,
        speed,
        params,
        animated,
        width,
        height,
    })
}

pub(crate) fn element<M: 'static>(
    source: Source,
    speed: f32,
    params: &[f32],
    animated: bool,
    width: Length,
    height: Length,
    opacity: f32,
) -> Element<'static, M> {
    let mut packed = [[0.0; 4]; 4];
    for (index, value) in params.iter().take(16).enumerate() {
        packed[index / 4][index % 4] = *value;
    }
    // Initialize the shared clock before the first draw; reconstructing views never resets it.
    LazyLock::force(&EPOCH);
    Element::new(Shader {
        source,
        speed,
        params: packed,
        animated,
        width,
        height,
        opacity,
        image: None,
    })
}

/// Sample a baked wallpaper tile without capturing desktop widgets or menus.
pub(crate) fn wallpaper<M: 'static>(
    image: iced::advanced::image::Handle,
    effect: &crate::config::WallpaperShader,
) -> Element<'static, M> {
    let fallback = || {
        iced::widget::Image::new(image.clone())
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(iced::ContentFit::Fill)
            .into()
    };
    if !matches!(&image, iced::advanced::image::Handle::Rgba {width, height, pixels, ..}
        if *width > 0 && *height > 0
            && u64::from(*width).checked_mul(u64::from(*height))
                .and_then(|pixels| pixels.checked_mul(4)) == Some(pixels.len() as u64))
    {
        return fallback();
    }
    let path = match resolve_in(&crate::config::shaders_dir(), &effect.path) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!("{error}; displaying the original wallpaper");
            return fallback();
        }
    };
    let source = match Source::open_inner(path, true) {
        Ok(source) => source,
        Err(error) => {
            tracing::warn!("{error}; displaying the original wallpaper");
            return fallback();
        }
    };
    let mut packed = [[0.0; 4]; 4];
    for (index, value) in effect.params.iter().take(16).enumerate() {
        packed[index / 4][index % 4] = *value;
    }
    LazyLock::force(&EPOCH);
    Element::new(Shader {
        source,
        speed: effect.speed,
        params: packed,
        animated: effect.animated,
        width: Length::Fill,
        height: Length::Fill,
        opacity: 1.0,
        image: Some(image),
    })
}

struct Shader {
    source: Source,
    speed: f32,
    params: [[f32; 4]; 4],
    animated: bool,
    width: Length,
    height: Length,
    opacity: f32,
    image: Option<iced::advanced::image::Handle>,
}
impl<M> Widget<M, iced::Theme, iced::Renderer> for Shader {
    fn tag(&self) -> advanced::widget::tree::Tag {
        advanced::widget::tree::Tag::of::<Source>()
    }
    fn state(&self) -> advanced::widget::tree::State {
        // Keep the weak source cache alive across view reconstruction, even
        // for wallpaper effects that have no Lua-owned intermediate tree.
        advanced::widget::tree::State::new(self.source.clone())
    }
    fn diff(&self, tree: &mut Tree) {
        *tree.state.downcast_mut::<Source>() = self.source.clone();
    }
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }
    fn layout(
        &mut self,
        _: &mut Tree,
        _: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, self.width, self.height)
    }
    fn update(
        &mut self,
        _: &mut Tree,
        event: &Event,
        layout: advanced::Layout<'_>,
        _: mouse::Cursor,
        renderer: &iced::Renderer,
        _: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if !matches!(renderer, iced_renderer::fallback::Renderer::Primary(_))
            || self.opacity <= 0.0
            || layout.bounds().intersection(viewport).is_none()
        {
            return;
        }
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            let fallback =
                self.image.is_some() && Arc::ptr_eq(&self.source.code(*now), &passthrough_code());
            let interval = if self.animated && self.speed != 0.0 && !fallback {
                Duration::from_millis(16)
            } else {
                RELOAD_INTERVAL
            };
            shell.request_redraw_at(*now + interval);
        } else {
            shell.request_redraw();
        }
    }
    fn draw(
        &self,
        _: &Tree,
        renderer: &mut iced::Renderer,
        _: &iced::Theme,
        _: &advanced::renderer::Style,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced_renderer::wgpu::primitive::Renderer as _;
        let bounds = layout.bounds();
        if self.opacity <= 0.0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        // iced's software fallback logs per primitive per frame; skip it explicitly.
        if !matches!(renderer, iced_renderer::fallback::Renderer::Primary(_)) {
            if let Some(image) = &self.image {
                use advanced::image::Renderer as _;
                renderer.draw_image(
                    advanced::image::Image::new(image.clone()).opacity(self.opacity),
                    bounds,
                    clip,
                );
                return;
            }
            static WARN: std::sync::Once = std::sync::Once::new();
            WARN.call_once(|| {
                tracing::warn!(
                    "iced.shader requires the wgpu renderer; tiny-skia leaves shaders transparent"
                )
            });
            return;
        }
        let now = Instant::now();
        let mouse = cursor
            .position_in(bounds)
            .map_or([-1.0, -1.0], |p| [p.x, p.y]);
        let uniforms = gpu::Uniforms {
            time: if self.animated {
                now.saturating_duration_since(*EPOCH).as_secs_f32() * self.speed
            } else {
                0.0
            },
            opacity: self.opacity,
            resolution: [bounds.width, bounds.height],
            mouse,
            padding: [0.0; 2],
            params: self.params,
            uv_rect: [
                (clip.x - bounds.x) / bounds.width,
                (clip.y - bounds.y) / bounds.height,
                clip.width / bounds.width,
                clip.height / bounds.height,
            ],
        };
        let primitive = gpu::Fragment::new(self.source.code(now), uniforms, self.image.clone());
        // A clipped viewport must not stretch the fragment's UV coordinates.
        // Keep the parent's layer (starting a new iced layer replaces its clip).
        renderer.draw_primitive(clip, primitive);
    }
}
