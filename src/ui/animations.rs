//! Owned tweens: translation is sampled natively; opacity refreshes cached Rust UI.
use aura_anim::core::{
    timing::{Delay, Timing},
    tween::Tween,
};
use iced::{
    Element, Event, Length, Rectangle, Size, Transformation, Vector,
    advanced::{
        self, Widget, layout,
        widget::{Tree, tree},
    },
    mouse,
};
use mlua::{Function, Lua, Table, Value};
use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bezier([f64; 4]),
}
impl Easing {
    pub(crate) fn descriptor(table: &Table) -> Result<Self, String> {
        strict(table, &["type", "bezier"], "easing")?;
        if !matches!(table.raw_get::<Value>("type").map_err(|e| e.to_string())?, Value::String(s) if s.to_string_lossy() == "easing")
        {
            return Err("easing.type must be 'easing'".into());
        }
        let points = table
            .raw_get::<Table>("bezier")
            .map_err(|_| "easing.bezier must be an array of four finite numbers")?;
        for pair in points.clone().pairs::<Value, Value>() {
            let (key, _) = pair.map_err(|e| e.to_string())?;
            if !matches!(key, Value::Integer(1..=4)) {
                return Err("easing.bezier must contain exactly four finite numbers".into());
            }
        }
        let mut curve = [0.0; 4];
        for (index, point) in curve.iter_mut().enumerate() {
            *point = match points
                .raw_get::<Value>(index + 1)
                .map_err(|e| e.to_string())?
            {
                Value::Integer(n) => n as f64,
                Value::Number(n) if n.is_finite() => n,
                _ => return Err("easing.bezier must contain exactly four finite numbers".into()),
            };
        }
        if !(0.0..=1.0).contains(&curve[0]) || !(0.0..=1.0).contains(&curve[2]) {
            return Err("easing.bezier x control points must be in 0..1".into());
        }
        Ok(Self::Bezier(curve))
    }
    pub(crate) fn encode(self, lua: &Lua) -> mlua::Result<Value> {
        let name = match self {
            Self::Linear => "linear",
            Self::EaseIn => "ease_in",
            Self::EaseOut => "ease_out",
            Self::EaseInOut => "ease_in_out",
            Self::Bezier(points) => {
                let table = lua.create_table()?;
                table.set("type", "easing")?;
                table.set("bezier", lua.create_sequence_from(points)?)?;
                return Ok(Value::Table(table));
            }
        };
        Ok(Value::String(lua.create_string(name)?))
    }
    fn sample(self, progress: f64) -> f64 {
        let Self::Bezier([x1, y1, x2, y2]) = self else {
            return progress;
        };
        if progress <= 0.0 || progress >= 1.0 {
            return progress;
        }
        let coordinate = |t: f64, a: f64, b: f64| {
            let u = 1.0 - t;
            3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
        };
        // Invert x(t), rather than treating elapsed progress as the curve parameter.
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..40 {
            let mid = (low + high) * 0.5;
            if coordinate(mid, x1, x2) < progress {
                low = mid;
            } else {
                high = mid;
            }
        }
        coordinate((low + high) * 0.5, y1, y2)
    }
    fn timing(self, duration: f64) -> Timing {
        match self {
            Self::Linear => Timing::linear(duration),
            Self::EaseIn => Timing::ease_in(duration),
            Self::EaseOut => Timing::ease_out(duration),
            Self::EaseInOut => Timing::ease_in_out(duration),
            Self::Bezier(_) => Timing::linear(duration),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Property {
    pub from: Option<f32>,
    pub to: f32,
    duration: f64,
    delay: f64,
    easing: Easing,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) struct Owner {
    vm: u64,
    epoch: u64,
    host: String,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spec {
    pub id: String,
    pub properties: [Option<Property>; 3],
    pub reduced_motion: bool,
    pub owner: Owner,
    pub scope: Vec<String>,
}
const NAMES: [&str; 3] = ["x", "y", "opacity"];
static NEXT_VM: AtomicU64 = AtomicU64::new(1);
static REDUCED: AtomicBool = AtomicBool::new(false);
pub(crate) fn reduced_motion(value: bool) {
    REDUCED.store(value, Ordering::Relaxed);
}
pub(crate) fn reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

impl Spec {
    /// Longest delay + duration across declared properties.
    pub(crate) fn total_duration(&self) -> Duration {
        if self.reduced_motion || reduced() {
            return Duration::ZERO;
        }
        let millis = self
            .properties
            .iter()
            .flatten()
            .map(|property| property.delay + property.duration)
            .fold(0.0_f64, f64::max);
        Duration::from_secs_f64(millis / 1000.0)
    }

    /// Popup close transition: keep the identity (so the live tween retargets
    /// from its current position) and head back to each property's start.
    pub(crate) fn reversed(&self) -> Self {
        let mut reversed = self.clone();
        reversed.properties = std::array::from_fn(|index| {
            self.properties[index].as_ref().map(|property| Property {
                from: None,
                to: property.from.unwrap_or(if index == 2 { 1.0 } else { 0.0 }),
                duration: property.duration,
                delay: property.delay,
                easing: property.easing,
            })
        });
        reversed
    }
}

pub(crate) fn install(lua: &Lua, iced: &Table) -> mlua::Result<()> {
    let vm = NEXT_VM.fetch_add(1, Ordering::Relaxed);
    lua.set_named_registry_value("riced.animation_host", "")?;
    lua.set_named_registry_value(
        "riced.animation_owner",
        lua.create_function(move |lua, ()| {
            Ok((
                vm,
                lua.named_registry_value::<u64>("riced.callback_epoch")?,
                lua.named_registry_value::<String>("riced.animation_host")?,
            ))
        })?,
    )?;
    let namespace = lua.create_table()?;
    for name in NAMES {
        namespace.set(
            name,
            lua.create_function(move |_, value: Table| {
                property(&value, name).map_err(mlua::Error::RuntimeError)?;
                Ok(value)
            })?,
        )?;
    }
    iced.set("animations", namespace)
}
fn strict(table: &Table, allowed: &[&str], what: &str) -> Result<(), String> {
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|e| e.to_string())?;
        if !key
            .as_string()
            .is_some_and(|key| allowed.contains(&key.to_string_lossy().as_str()))
        {
            return Err(format!("{what}: unknown field {key:?}"));
        }
    }
    Ok(())
}
fn number(table: &Table, key: &str) -> Result<Option<f64>, String> {
    match table.raw_get::<Value>(key).map_err(|e| e.to_string())? {
        Value::Nil => Ok(None),
        Value::Integer(n) => Ok(Some(n as f64)),
        Value::Number(n) if n.is_finite() => Ok(Some(n)),
        _ => Err(format!("{key} must be a finite number")),
    }
}
fn property(table: &Table, name: &str) -> Result<Property, String> {
    strict(table, &["from", "to", "duration", "delay", "easing"], name)?;
    let from = number(table, "from")?.map(|n| n as f32);
    let to = number(table, "to")?.ok_or_else(|| format!("{name}.to is required"))? as f32;
    if !to.is_finite() || from.is_some_and(|n| !n.is_finite()) {
        return Err(format!("{name} endpoints must be finite f32 values"));
    }
    if name == "opacity"
        && (!(0.0..=1.0).contains(&to) || from.is_some_and(|n| !(0.0..=1.0).contains(&n)))
    {
        return Err("opacity endpoints must be in 0..1".into());
    }
    let duration = number(table, "duration")?.unwrap_or(200.0);
    let delay = number(table, "delay")?.unwrap_or(0.0);
    if !(0.0..=86_400_000.0).contains(&duration) || !(0.0..=86_400_000.0).contains(&delay) {
        return Err("duration and delay must be in 0..86400000 milliseconds".into());
    }
    let easing = match table
        .raw_get::<Value>("easing")
        .map_err(|e| e.to_string())?
    {
        Value::Nil => Easing::EaseOut,
        Value::String(s) => match s.to_string_lossy().as_str() {
            "linear" => Easing::Linear,
            "ease_in" => Easing::EaseIn,
            "ease_out" => Easing::EaseOut,
            "ease_in_out" => Easing::EaseInOut,
            _ => return Err("easing must be linear, ease_in, ease_out, or ease_in_out".into()),
        },
        Value::Table(table) => Easing::descriptor(&table)?,
        _ => return Err("easing must be a built-in string or easing descriptor".into()),
    };
    Ok(Property {
        from,
        to,
        duration,
        delay,
        easing,
    })
}
pub(crate) fn parse(node: &Table) -> Result<Option<Spec>, String> {
    let value = super::decode::node_property(node, "animate").map_err(|e| e.to_string())?;
    match value {
        Value::Nil => Ok(None),
        Value::Table(table) => Ok(Some(parse_animation(&table, None, node)?)),
        _ => Err("animate must be a table".into()),
    }
}

fn parse_animation(
    table: &Table,
    default_id: Option<String>,
    node: &Table,
) -> Result<Spec, String> {
    strict(
        table,
        &["id", "x", "y", "opacity", "reduced_motion"],
        "animate",
    )?;
    let id = match table.raw_get::<Value>("id").map_err(|e| e.to_string())? {
        Value::String(id) if !id.to_string_lossy().trim().is_empty() => id.to_string_lossy(),
        Value::Nil => default_id.ok_or("animate.id must be a non-empty stable string")?,
        _ => return Err("animate.id must be a non-empty stable string".into()),
    };
    let mut properties = [None, None, None];
    for (i, name) in NAMES.iter().enumerate() {
        properties[i] = match table.raw_get::<Value>(*name).map_err(|e| e.to_string())? {
            Value::Nil => None,
            Value::Table(t) => {
                Some(property(&t, name).map_err(|e| format!("animate.{name}: {e}"))?)
            }
            _ => {
                return Err(format!(
                    "animate.{name} must be an animation descriptor table"
                ));
            }
        };
    }
    if properties.iter().all(Option::is_none) {
        return Err("animate needs x, y, or opacity".into());
    }
    let reduced_motion = match table
        .raw_get::<Value>("reduced_motion")
        .map_err(|e| e.to_string())?
    {
        Value::Nil => false,
        Value::Boolean(value) => value,
        _ => return Err("animate.reduced_motion must be a boolean".into()),
    };
    let owner = match node
        .raw_get::<Option<Function>>("_animation_owner")
        .map_err(|e| e.to_string())?
    {
        Some(function) => {
            let (vm, epoch, host) = function
                .call::<(u64, u64, String)>(())
                .map_err(|e| e.to_string())?;
            Owner { vm, epoch, host }
        }
        None => return Err("animated nodes must be built with iced constructors".into()),
    };
    Ok(Spec {
        id,
        properties,
        reduced_motion,
        owner,
        scope: Vec::new(),
    })
}

/// Parse `:on_open(...)`/`:on_close(...)` on a popup root. A function is a
/// callback shorthand; a table may carry an explicit transition and callback.
pub(crate) fn parse_lifecycle(
    node: &Table,
    field: &str,
) -> Result<Option<super::components::PopupLifecycle>, String> {
    let value = super::decode::node_property(node, field).map_err(|e| e.to_string())?;
    let default_id =
        match super::decode::node_property(node, "animate").map_err(|e| e.to_string())? {
            Value::Table(table) => table
                .raw_get::<Value>("id")
                .ok()
                .and_then(|value| value.as_string().map(|id| id.to_string_lossy())),
            _ => None,
        };
    let mut lifecycle = super::components::PopupLifecycle::default();
    match value {
        Value::Nil => return Ok(None),
        Value::Function(function) => lifecycle.callback = Some(capture_lifecycle(node, function)?),
        Value::Table(table) => {
            strict(&table, &["animate", "callback"], field)?;
            lifecycle.animate = match table
                .raw_get::<Value>("animate")
                .map_err(|e| e.to_string())?
            {
                Value::Nil => None,
                Value::Table(animation) => Some(Box::new(
                    parse_animation(&animation, default_id, node)
                        .map_err(|e| format!("{field}.{e}"))?,
                )),
                _ => return Err(format!("{field}.animate must be an animation table")),
            };
            lifecycle.callback = match table
                .raw_get::<Value>("callback")
                .map_err(|e| e.to_string())?
            {
                Value::Nil => None,
                Value::Function(function) => Some(capture_lifecycle(node, function)?),
                Value::UserData(token) => Some(
                    token
                        .borrow::<crate::lua::callbacks::Callback>()
                        .map_err(|e| e.to_string())?
                        .clone(),
                ),
                _ => return Err(format!("{field}.callback must be a function")),
            };
        }
        _ => return Err(format!("{field} must be a function or table")),
    }
    Ok(Some(lifecycle))
}

fn capture_lifecycle(
    node: &Table,
    function: Function,
) -> Result<crate::lua::callbacks::Callback, String> {
    let capture: Function = node
        .raw_get("_capture_lifecycle")
        .map_err(|e| e.to_string())?;
    let token: mlua::AnyUserData = capture.call(function).map_err(|e| e.to_string())?;
    token
        .borrow::<crate::lua::callbacks::Callback>()
        .map_err(|e| e.to_string())
        .map(|callback| callback.clone())
}
pub(crate) fn encode(lua: &Lua, spec: &Spec) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("id", spec.id.as_str())?;
    if spec.reduced_motion {
        table.set("reduced_motion", true)?;
    }
    for (name, property) in NAMES.iter().zip(&spec.properties) {
        if let Some(p) = property {
            let value = lua.create_table()?;
            value.set("from", p.from)?;
            value.set("to", p.to)?;
            value.set("duration", p.duration)?;
            value.set("delay", p.delay)?;
            value.set("easing", p.easing.encode(lua)?)?;
            table.set(*name, value)?;
        }
    }
    Ok(table)
}

#[derive(Debug)]
struct Track {
    property: Property,
    tween: Tween<f64>,
    from: f64,
}
impl Track {
    fn value(&self) -> f64 {
        let value = *self.tween.value();
        if !matches!(self.property.easing, Easing::Bezier(_)) {
            return value;
        }
        let to = f64::from(self.property.to);
        if self.from == to {
            return to;
        }
        self.from
            + (to - self.from)
                * self
                    .property
                    .easing
                    .sample((value - self.from) / (to - self.from))
    }
}
#[derive(Debug)]
struct State {
    tracks: [Option<Track>; 3],
    last: Instant,
}
impl State {
    fn new(spec: &Spec, now: Instant) -> Self {
        let mut state = Self {
            tracks: [None, None, None],
            last: now,
        };
        state.reconcile(spec, now);
        state
    }
    fn tick(&mut self, now: Instant) {
        let delta = now.saturating_duration_since(self.last);
        self.last = self.last.max(now);
        for track in self.tracks.iter_mut().flatten() {
            track.tween.tick(delta);
        }
    }
    fn reconcile(&mut self, spec: &Spec, now: Instant) {
        self.tick(now);
        for (current, declared) in self.tracks.iter_mut().zip(&spec.properties) {
            let Some(p) = declared else {
                *current = None;
                continue;
            };
            let reduced = spec.reduced_motion || REDUCED.load(Ordering::Relaxed);
            if reduced || p.duration == 0.0 {
                *current = Some(Track {
                    property: p.clone(),
                    tween: Tween::new(f64::from(p.to)),
                    from: f64::from(p.to),
                });
                continue;
            }
            if current
                .as_ref()
                .is_some_and(|track| track.property.to == p.to)
            {
                continue;
            }
            let to = f64::from(p.to);
            let from = current
                .as_ref()
                .map(Track::value)
                .unwrap_or(f64::from(p.from.unwrap_or(p.to)));
            let timing = p
                .easing
                .timing(p.duration)
                .with_delay(Delay::from_millis(p.delay));
            *current = Some(Track {
                property: p.clone(),
                from,
                tween: if from == to {
                    Tween::new(to)
                } else {
                    Tween::between(from, to, timing)
                },
            });
        }
    }
    fn values(&self) -> [f32; 3] {
        std::array::from_fn(|i| {
            self.tracks[i]
                .as_ref()
                .map_or(if i == 2 { 1.0 } else { 0.0 }, |track| track.value() as f32)
        })
    }
    fn active(&self) -> bool {
        self.tracks
            .iter()
            .flatten()
            .any(|track| track.tween.is_active())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key(Owner, Vec<String>, String);
type Shared = Arc<Mutex<State>>;
struct Cache {
    entries: HashMap<Key, Weak<Mutex<State>>>,
    pruned: Instant,
}
static STATES: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        entries: HashMap::new(),
        pruned: Instant::now(),
    })
});

/// Build from cached descriptions only. Native Tree ownership bounds lifetime.
pub(crate) fn realize<M: Clone + 'static>(
    spec: &Spec,
    frame: M,
    build: impl FnOnce([f32; 3]) -> Result<Element<'static, M>, String>,
) -> Result<Element<'static, M>, String> {
    let now = Instant::now();
    let key = Key(spec.owner.clone(), spec.scope.clone(), spec.id.clone());
    let state = {
        let mut states = STATES.lock().map_err(|e| e.to_string())?;
        if now.saturating_duration_since(states.pruned) >= Duration::from_secs(1) {
            states.entries.retain(|_, state| state.strong_count() > 0);
            states.pruned = now;
        }
        match states.entries.get(&key).and_then(Weak::upgrade) {
            Some(state) => state,
            None => {
                let state = Arc::new(Mutex::new(State::new(spec, now)));
                states.entries.insert(key, Arc::downgrade(&state));
                state
            }
        }
    };
    let values = {
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.reconcile(spec, now);
        state.values()
    };
    let content = build(values)?;
    Ok(Element::new(Animated {
        content,
        state,
        opacity: values[2],
        frame,
    }))
}
struct Animated<M> {
    content: Element<'static, M>,
    state: Shared,
    opacity: f32,
    frame: M,
}
impl<M> Animated<M> {
    fn offset(&self) -> Vector {
        self.state
            .lock()
            .map(|state| {
                let values = state.values();
                Vector::new(values[0], values[1])
            })
            .unwrap_or(Vector::ZERO)
    }
}
impl<M: Clone + 'static> Widget<M, iced::Theme, iced::Renderer> for Animated<M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Shared>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(self.state.clone())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut Tree) {
        *tree.state.downcast_mut::<Shared>() = self.state.clone();
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let now = match event {
            Event::Window(iced::window::Event::RedrawRequested(now)) => *now,
            _ => Instant::now(),
        };
        if let Ok(mut state) = self.state.lock() {
            state.tick(now);
            let opacity_changed = state.values()[2] != self.opacity;
            if opacity_changed
                && matches!(
                    event,
                    Event::Window(iced::window::Event::RedrawRequested(_))
                )
            {
                // Do not invalidate_widgets here: the layer-shell redraw path
                // discards Outdated, including its next-frame request.
                shell.publish(self.frame.clone());
            }
            if state.active() || opacity_changed {
                shell.request_redraw();
            }
        }
        let offset = self.offset();
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor * Transformation::translate(-offset.x, -offset.y),
            renderer,
            clipboard,
            shell,
            &(*viewport - offset),
        );
        if matches!(event, Event::Mouse(_)) {
            shell.request_redraw();
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &advanced::renderer::Style,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let offset = self.offset();
        advanced::Renderer::with_translation(renderer, offset, |renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor * Transformation::translate(-offset.x, -offset.y),
                &(*viewport - offset),
            )
        });
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: advanced::Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let offset = self.offset();
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor * Transformation::translate(-offset.x, -offset.y),
            &(*viewport - offset),
            renderer,
        )
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: advanced::Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn advanced::widget::Operation,
    ) {
        let offset = self.offset();
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            &mut super::widgets::measured::Translated { operation, offset },
        );
    }
    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: advanced::Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<advanced::overlay::Element<'a, M, iced::Theme, iced::Renderer>> {
        let offset = self.offset();
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation + offset,
        )
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
    fn node(lua: &Lua, source: &str) -> WidgetNode {
        parse_node(&lua.load(source).eval().unwrap()).unwrap()
    }
    fn spec(lua: &Lua, body: &str) -> Spec {
        let WidgetNode::Animated { spec, .. } =
            node(lua, &format!("return iced.text('x'):animate({body})"))
        else {
            panic!("animated")
        };
        spec
    }
    #[test]
    fn descriptors_roundtrip_and_reject_invalid_values() {
        let lua = lua();
        let value = node(
            &lua,
            "local a=iced.animations; return iced.container(iced.text('x')):animate({id='card',x=a.x({from=0,to=10,duration=100,delay=25,easing='linear'}),opacity=a.opacity({from=0,to=1})}):padding(8)",
        );
        let encoded = crate::ui::encode::to_table(&lua, &value).unwrap();
        assert_eq!(parse_node(&Value::Table(encoded)).unwrap(), value);
        for body in [
            "{}",
            "{id='x'}",
            "{id='',x={to=1}}",
            "{id='x',width={to=1}}",
            "{id='x',x=1}",
            "{id='x',x={to=0/0}}",
            "{id='x',x={to=1,duration=-1}}",
            "{id='x',x={to=1,delay=1/0}}",
            "{id='x',x={to=1,easing='bounce'}}",
            "{id='x',x={to=1,typo=true}}",
            "{id='x',opacity={from=-1,to=1}}",
            "{id='x',opacity={to=2}}",
            "{id='x',reduced_motion=1,x={to=1}}",
            "{id='x',x={from=0}}",
            "{id='x',x={to=function() end}}",
        ] {
            let value = lua
                .load(format!("return iced.text('x'):animate({body})"))
                .eval()
                .unwrap();
            assert!(parse_node(&value).is_err(), "{body}");
        }
    }
    #[test]
    fn defined_easing_roundtrips_and_snapshots_tables() {
        let lua = lua();
        lua.load(
            r#"
            local definition = {type='easing', bezier={0.2, 0, 0.2, 1}}
            iced.define('snappy', definition)
            definition.bezier[1] = 99
            local curve = iced.snappy()
            curve.bezier[1] = 99
            assert(iced.use('snappy').bezier[1] == 0.2)
        "#,
        )
        .exec()
        .unwrap();
        let value = node(
            &lua,
            "return iced.text('x'):animate({id='curve', x=iced.animations.x({from=0,to=10,easing=iced.snappy()})})",
        );
        let encoded = crate::ui::encode::to_table(&lua, &value).unwrap();
        assert_eq!(parse_node(&Value::Table(encoded)).unwrap(), value);
    }
    #[test]
    fn easing_definitions_reject_malformed_descriptors_and_reserved_names() {
        let lua = lua();
        for definition in [
            "{}",
            "{type='other',bezier={0,0,1,1}}",
            "{type='easing',bezier={0,0,1}}",
            "{type='easing',bezier={0,0,1,1,2}}",
            "{type='easing',bezier={0,0,1,1,extra=1}}",
            "{type='easing',bezier={-0.1,0,1,1}}",
            "{type='easing',bezier={0,0,1.1,1}}",
            "{type='easing',bezier={0,0/0,1,1}}",
            "{type='easing',bezier={0,0,1,1/0}}",
            "{type='easing',bezier={0,'0',1,1}}",
            "{type='easing',bezier={0,0,1,1},typo=true}",
        ] {
            assert!(
                lua.load(format!("iced.define('invalid', {definition})"))
                    .exec()
                    .is_err(),
                "{definition}"
            );
        }
        assert!(
            lua.load("iced.define('text', {type='easing',bezier={0,0,1,1}})")
                .exec()
                .is_err()
        );
    }
    #[test]
    fn bezier_sampling_inverts_time_and_preserves_endpoints_and_overshoot() {
        let curve = Easing::Bezier([0.0, 0.0, 1.0, 1.0]);
        for progress in [0.0, 0.1, 0.5, 0.9, 1.0] {
            assert!((curve.sample(progress) - progress).abs() < 1e-9);
        }
        assert!((Easing::Bezier([0.0, 0.0, 0.0, 1.0]).sample(0.125) - 0.5).abs() < 1e-9);
        assert!(Easing::Bezier([0.2, 2.0, 0.8, 2.0]).sample(0.5) > 1.0);
    }
    #[test]
    fn custom_curves_delay_retarget_reverse_and_finish_natively() {
        let lua = lua();
        lua.load("iced.define('snappy', {type='easing',bezier={0,0,0,1}})")
            .exec()
            .unwrap();
        let mut spec = spec(
            &lua,
            "{id='custom',x={from=0,to=10,duration=800,delay=100,easing=iced.snappy()}}",
        );
        let start = Instant::now();
        let mut state = State::new(&spec, start);
        state.tick(start + Duration::from_millis(50));
        assert_eq!(state.values()[0], 0.0);
        state.tick(start + Duration::from_millis(200));
        assert!((state.values()[0] - 5.0).abs() < 1e-5);
        assert_eq!(
            spec.reversed().properties[0].as_ref().unwrap().easing,
            Easing::Bezier([0.0, 0.0, 0.0, 1.0])
        );
        spec.properties[0].as_mut().unwrap().to = 20.0;
        spec.properties[0].as_mut().unwrap().delay = 0.0;
        state.reconcile(&spec, start + Duration::from_millis(200));
        assert!((state.values()[0] - 5.0).abs() < 1e-5);
        state.tick(start + Duration::from_millis(1000));
        assert_eq!(state.values()[0], 20.0);
        assert!(!state.active());
    }
    #[test]
    fn duplicate_ids_are_rejected_but_list_items_scope_their_animations() {
        let lua = lua();
        let value=lua.load("local function card() return iced.text('x'):animate({id='same',x={to=1}}) end; return iced.row({card(),card()})").eval().unwrap();
        assert!(
            parse_node(&value)
                .unwrap_err()
                .contains("duplicate animation id")
        );
        let value = node(
            &lua,
            "return iced.listview({{id='a'},{id='b'}}):id('rows'):key('id'):delegate(function() return iced.text('x'):animate({id='same',opacity={from=0,to=1}}) end)",
        );
        let WidgetNode::ListView { items, .. } = value else {
            panic!("list")
        };
        let scopes: Vec<_> = items
            .iter()
            .map(|(_, node)| match node {
                WidgetNode::Animated { spec, .. } => spec.scope.clone(),
                _ => panic!("animated"),
            })
            .collect();
        assert_ne!(scopes[0], scopes[1]);
    }
    #[test]
    fn tween_delays_retargets_and_does_not_restart_unchanged_declarations() {
        let lua = lua();
        let mut spec = spec(
            &lua,
            "{id='retarget',x={from=0,to=10,duration=100,delay=50,easing='linear'}}",
        );
        let start = Instant::now();
        let mut state = State::new(&spec, start);
        state.tick(start + Duration::from_millis(25));
        assert_eq!(state.values()[0], 0.0);
        state.reconcile(&spec, start + Duration::from_millis(100));
        assert_eq!(state.values()[0], 5.0);
        spec.properties[0].as_mut().unwrap().to = 20.0;
        spec.properties[0].as_mut().unwrap().delay = 0.0;
        state.reconcile(&spec, start + Duration::from_millis(100));
        assert_eq!(state.values()[0], 5.0);
        state.reconcile(&spec, start + Duration::from_millis(150));
        assert_eq!(state.values()[0], 12.5);
        state.tick(start + Duration::from_millis(200));
        assert_eq!(state.values()[0], 20.0);
        assert!(!state.active());
    }
    #[test]
    fn mount_without_from_zero_duration_and_reduced_motion_settle_immediately() {
        let lua = lua();
        let now = Instant::now();
        for body in [
            "{id='no-from',x={to=20}}",
            "{id='zero',x={from=0,to=20,duration=0,delay=100}}",
            "{id='reduced',reduced_motion=true,x={from=0,to=20}}",
        ] {
            let state = State::new(&spec(&lua, body), now);
            assert_eq!(state.values(), [20.0, 0.0, 1.0]);
            assert!(!state.active());
        }
    }
    #[test]
    fn translation_frames_do_not_rebuild_widgets_or_touch_lua_and_release_state() {
        use iced::advanced::{Layout, renderer::Headless};
        let lua = lua();
        let description = node(
            &lua,
            "calls=0; local node=iced.text('animated'):animate({id='native-test',x={from=0,to=20,duration=100}}); calls=calls+1; return node",
        );
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut content =
            crate::ui::build::realize(&description, 13.0, Arc::new(|_| ()), false, 1.0).unwrap();
        let mut tree = Tree::new(&content);
        let state = tree.state.downcast_ref::<Shared>().clone();
        let weak = Arc::downgrade(&state);
        let limits = layout::Limits::new(Size::ZERO, Size::new(100.0, 50.0));
        let layout = content
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits);
        let now = state.lock().unwrap().last;
        let mut messages = Vec::new();
        let mut shell = advanced::Shell::new(&mut messages);
        content.as_widget_mut().update(
            &mut tree,
            &Event::Window(iced::window::Event::RedrawRequested(
                now + Duration::from_millis(50),
            )),
            Layout::new(&layout),
            mouse::Cursor::Unavailable,
            &renderer,
            &mut advanced::clipboard::Null,
            &mut shell,
            &Rectangle::with_size(Size::new(100.0, 50.0)),
        );
        assert!(!shell.are_widgets_invalid());
        assert_eq!(
            shell.redraw_request(),
            iced::window::RedrawRequest::NextFrame
        );
        assert!(shell.is_empty());
        assert_eq!(lua.globals().get::<u32>("calls").unwrap(), 1);
        let rebuilt =
            crate::ui::build::realize(&description, 13.0, Arc::new(|_| ()), false, 1.0).unwrap();
        let rebuilt_tree = Tree::new(&rebuilt);
        assert!(Arc::ptr_eq(
            &state,
            rebuilt_tree.state.downcast_ref::<Shared>()
        ));
        state.lock().unwrap().tick(now + Duration::from_secs(1));
        let mut settled =
            crate::ui::build::realize(&description, 13.0, Arc::new(|_| ()), false, 1.0).unwrap();
        let mut settled_tree = Tree::new(&settled);
        let settled_layout = settled
            .as_widget_mut()
            .layout(&mut settled_tree, &renderer, &limits);
        let mut settled_messages = Vec::new();
        let mut settled_shell = advanced::Shell::new(&mut settled_messages);
        settled.as_widget_mut().update(
            &mut settled_tree,
            &Event::Window(iced::window::Event::RedrawRequested(
                now + Duration::from_secs(2),
            )),
            Layout::new(&settled_layout),
            mouse::Cursor::Unavailable,
            &renderer,
            &mut advanced::clipboard::Null,
            &mut settled_shell,
            &Rectangle::with_size(Size::new(100.0, 50.0)),
        );
        assert_eq!(
            settled_shell.redraw_request(),
            iced::window::RedrawRequest::Wait
        );
        assert!(!settled_shell.are_widgets_invalid());
        drop(settled_tree);
        drop(settled);
        drop(rebuilt_tree);
        drop(rebuilt);
        drop(state);
        drop(tree);
        drop(content);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn windows_and_reload_epochs_do_not_share_tweens() {
        let mut runtime = crate::lua::LuaRuntime::new().unwrap();
        let source = "return {view=function() return iced.text('x'):animate({id='same',x={from=0,to=20}}) end}";
        runtime.load(source).unwrap();
        let a = runtime.view(crate::lua::WindowId(1)).unwrap();
        let b = runtime.view(crate::lua::WindowId(2)).unwrap();
        runtime.load(source).unwrap();
        let c = runtime.view(crate::lua::WindowId(1)).unwrap();
        let owner = |node| match node {
            WidgetNode::Animated { spec, .. } => spec.owner,
            _ => panic!("animated"),
        };
        assert_ne!(owner(a.clone()), owner(b));
        assert_ne!(owner(a), owner(c));
    }

    #[test]
    fn opacity_frames_publish_native_refreshes_without_outdating_the_redraw() {
        use iced::advanced::{Layout, renderer::Headless};
        let lua = lua();
        let description = node(
            &lua,
            "return iced.text('fade'):animate({id='native-alpha-refresh',opacity={from=0,to=1,duration=100,easing='linear'}})",
        );
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut content =
            crate::ui::build::realize(&description, 13.0, Arc::new(|action| action), false, 1.0)
                .unwrap();
        let mut tree = Tree::new(&content);
        let now = native_state(&tree).lock().unwrap().last;
        let bounds = Rectangle::with_size(Size::new(100.0, 50.0));
        let layout = content.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, bounds.size()),
        );
        let mut messages = Vec::new();
        let mut shell = advanced::Shell::new(&mut messages);
        content.as_widget_mut().update(
            &mut tree,
            &Event::Window(iced::window::Event::RedrawRequested(
                now + Duration::from_millis(50),
            )),
            Layout::new(&layout),
            mouse::Cursor::Unavailable,
            &renderer,
            &mut advanced::clipboard::Null,
            &mut shell,
            &bounds,
        );
        assert!(!shell.are_widgets_invalid());
        assert_eq!(
            shell.redraw_request(),
            iced::window::RedrawRequest::NextFrame
        );
        assert_eq!(messages.len(), 1);
        assert!(messages[0].is_animation_frame());
    }

    #[test]
    fn native_refresh_does_not_invoke_handlers_or_invalidate_lua_views() {
        let mut runtime = crate::lua::LuaRuntime::new().unwrap();
        runtime.load("return {calls=0,view=function(self) self.calls=self.calls+1; return iced.text(self.calls) end,actions=function() error('frame reached Lua') end}").unwrap();
        let window = crate::lua::WindowId(1);
        let before = runtime.view(window).unwrap();
        runtime
            .update(&crate::lua::Message::Action(
                crate::ui::action::Action::animation_frame(),
            ))
            .unwrap();
        assert_eq!(runtime.view(window).unwrap(), before);
        assert!(runtime.last_error.is_none());
    }

    #[test]
    fn translation_draws_every_frame_using_the_same_element_and_layout() {
        use iced::advanced::{Layout, renderer::Headless, renderer::Style};
        let lua = lua();
        let description = node(
            &lua,
            "return iced.container(iced.space():width(10):height(10)):background('#00ff00'):animate({id='draw-without-rebuild',x={from=0,to=40,duration=100,easing='linear'}})",
        );
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut content =
            crate::ui::build::realize(&description, 13.0, Arc::new(|action| action), false, 1.0)
                .unwrap();
        let mut tree = Tree::new(&content);
        let start = native_state(&tree).lock().unwrap().last;
        let bounds = Rectangle::with_size(Size::new(80.0, 20.0));
        let layout = content.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, bounds.size()),
        );
        for millis in [0, 25, 50, 75, 100] {
            let mut messages = Vec::new();
            let mut shell = advanced::Shell::new(&mut messages);
            content.as_widget_mut().update(
                &mut tree,
                &Event::Window(iced::window::Event::RedrawRequested(
                    start + Duration::from_millis(millis),
                )),
                Layout::new(&layout),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut advanced::clipboard::Null,
                &mut shell,
                &bounds,
            );
            assert!(!shell.are_widgets_invalid());
            assert!(shell.is_empty());
            assert_eq!(
                shell.redraw_request(),
                if millis == 100 {
                    iced::window::RedrawRequest::Wait
                } else {
                    iced::window::RedrawRequest::NextFrame
                }
            );
            advanced::Renderer::reset(&mut renderer, bounds);
            content.as_widget().draw(
                &tree,
                &mut renderer,
                &iced::Theme::Dark,
                &Style::default(),
                Layout::new(&layout),
                mouse::Cursor::Unavailable,
                &bounds,
            );
            let pixels = renderer.screenshot(Size::new(80, 20), 1.0, iced::Color::TRANSPARENT);
            let x = (millis * 40 / 100) as usize + 5;
            assert_eq!(
                &pixels[(5 * 80 + x) * 4..(5 * 80 + x) * 4 + 4],
                [0, 255, 0, 255],
                "frame {millis}"
            );
        }
    }

    #[test]
    fn a_widgets_view_and_popup_have_independent_animation_owners() {
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        crate::lua::widgets::load_widget_script(&lua,"animation-hosts","return {view=function() return iced.text('main'):animate({id='same',x={from=0,to=10}}) end, popup=function() return iced.text('popup'):animate({id='same',x={from=0,to=20}}) end}").unwrap();
        let view = parse_node(&crate::lua::widgets::call_lua_value(&lua, "view").unwrap()).unwrap();
        let popup =
            parse_node(&crate::lua::widgets::call_lua_value(&lua, "popup").unwrap()).unwrap();
        let owner = |node| match node {
            WidgetNode::Animated { spec, .. } => spec.owner,
            _ => panic!("animated"),
        };
        assert_ne!(owner(view), owner(popup));
    }

    fn native_state(tree: &Tree) -> Shared {
        if tree.tag == tree::Tag::of::<Shared>() {
            return tree.state.downcast_ref::<Shared>().clone();
        }
        for child in &tree.children {
            if let Some(state) = find_state(child) {
                return state;
            }
        }
        panic!("animation state")
    }
    fn find_state(tree: &Tree) -> Option<Shared> {
        if tree.tag == tree::Tag::of::<Shared>() {
            return Some(tree.state.downcast_ref::<Shared>().clone());
        }
        tree.children.iter().find_map(find_state)
    }
    fn host(node: &WidgetNode, kind: usize) -> Element<'static, crate::shell::Plant> {
        let sender: crate::ui::action::Sender = Arc::new(|_| crate::shell::Plant::Tend);
        match kind {
            0 => crate::ui::build::realize(node, 13.0, sender, true, 1.0).unwrap(),
            1 => crate::ui::scene::Lists::default()
                .build(node, &Default::default(), sender, None)
                .unwrap(),
            _ => crate::shell::screens::top::build_with_lists(
                node,
                "test",
                13.0,
                Some(sender),
                &Default::default(),
                &Default::default(),
                None,
            )
            .unwrap(),
        }
    }
    #[test]
    fn translated_buttons_hit_the_displayed_position_in_every_host() {
        use iced::advanced::{Layout, renderer::Headless};
        let lua = lua();
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        for kind in 0..3 {
            let description = node(
                &lua,
                &format!(
                    "return iced.button('go','go'):width(40):height(20):animate({{id='hit-{kind}',x={{from=0,to=40,duration=100,easing='linear'}}}})"
                ),
            );
            let mut original = host(&description, kind);
            let mut tree = Tree::new(&original);
            let state = native_state(&tree);
            {
                let mut state = state.lock().unwrap();
                let now = state.last;
                state.tick(now + Duration::from_millis(50));
            }
            let bounds = Rectangle::with_size(Size::new(100.0, 40.0));
            let layout = original.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, bounds.size()),
            );
            let mut messages = Vec::new();
            for x in [5.0, 25.0] {
                for button in [
                    mouse::Event::ButtonPressed(mouse::Button::Left),
                    mouse::Event::ButtonReleased(mouse::Button::Left),
                ] {
                    original.as_widget_mut().update(
                        &mut tree,
                        &Event::Mouse(button),
                        Layout::new(&layout),
                        mouse::Cursor::Available(iced::Point::new(x, 10.0)),
                        &renderer,
                        &mut advanced::clipboard::Null,
                        &mut advanced::Shell::new(&mut messages),
                        &bounds,
                    );
                }
            }
            assert_eq!(messages.len(), 1, "host {kind}");
        }
    }

    #[test]
    fn primitive_opacity_is_sampled_in_every_host_without_lua_frames() {
        use iced::advanced::{Layout, renderer::Headless, renderer::Style};
        let lua = lua();
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        for kind in 0..3 {
            let description = node(
                &lua,
                &format!(
                    "return iced.container(iced.space():width(10):height(10)):background('#00ff00'):animate({{id='fade-{kind}',opacity={{from=0,to=1,duration=100,easing='linear'}}}})"
                ),
            );
            let original = host(&description, kind);
            let mut tree = Tree::new(&original);
            let state = native_state(&tree);
            {
                let mut state = state.lock().unwrap();
                let now = state.last;
                state.tick(now + Duration::from_millis(50));
            }
            let mut rebuilt = host(&description, kind);
            tree.diff(&rebuilt);
            let bounds = Rectangle::with_size(Size::new(40.0, 40.0));
            let layout = rebuilt.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, bounds.size()),
            );
            advanced::Renderer::reset(&mut renderer, bounds);
            rebuilt.as_widget().draw(
                &tree,
                &mut renderer,
                &iced::Theme::Dark,
                &Style::default(),
                Layout::new(&layout),
                mouse::Cursor::Unavailable,
                &bounds,
            );
            let pixels = renderer.screenshot(Size::new(40, 40), 1.0, iced::Color::TRANSPARENT);
            let pixel = &pixels[(5 * 40 + 5) * 4..(5 * 40 + 5) * 4 + 4];
            assert!(
                pixel[0] == 0
                    && (126..=129).contains(&pixel[1])
                    && pixel[2] == 0
                    && (126..=129).contains(&pixel[3]),
                "host {kind}: {pixel:?}"
            );
        }
    }
    #[test]
    fn animated_wrappers_preserve_nested_list_registration() {
        let lua = lua();
        let description = node(
            &lua,
            "return iced.listview({{id='a'}}):id('rows'):key('id'):delegate(function() return iced.text('row'):animate({id='row',x={from=10,to=0}}) end):animate({id='whole-list',opacity={from=0,to=1}})",
        );
        let mut motion = crate::ui::anim::AnimRuntime::default();
        let mut scene = crate::ui::scene::Lists::default();
        scene
            .synchronize(
                None,
                &description,
                &Default::default(),
                &mut motion,
                Duration::from_millis(100),
            )
            .unwrap();
        assert!(scene.lists.contains_key("rows"));
        let mut lists = crate::shell::screens::top::animation::WidgetLists::default();
        crate::shell::screens::top::animation::ListContext {
            lists: &mut lists,
            motion: &mut motion,
            duration: Duration::from_millis(100),
        }
        .synchronize("widget", None, &description)
        .unwrap();
        assert!(lists.contains_key("widget/rows"));
    }
    #[test]
    fn example_targets_toggle_without_reexecuting_lua_on_native_rebuilds() {
        let mut runtime = crate::lua::LuaRuntime::new().unwrap();
        runtime
            .load(include_str!("../../scripts/animations.lua"))
            .unwrap();
        let window = crate::lua::WindowId(1);
        let before = runtime.view(window).unwrap();
        runtime
            .update(&crate::lua::Message::Action(String::from("toggle").into()))
            .unwrap();
        let after = runtime.view(window).unwrap();
        assert_ne!(before, after);
        assert!(crate::ui::build::realize(&after, 13.0, Arc::new(|_| ()), true, 1.0).is_ok());
    }
}
