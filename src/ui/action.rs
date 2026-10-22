//! Owned interaction messages. No callback or Lua value crosses the VM boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionValue {
    Boolean(bool),
    Number(f32),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub key: String,
    pub value: Option<ActionValue>,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Event {
    /// Native UI refresh only. Never delivered to a Lua action handler.
    AnimationFrame,
    #[default]
    Action,
    Mouse {
        state: String,
        button: Option<String>,
        position: Option<iced::Point>,
        /// Original widget-local press point, retained for host pickup geometry.
        origin: Option<iced::Point>,
        /// Placement rect in window-local px, so hosts can anchor popups to
        /// the widget that reported the event (not the slot index).
        bounds: Option<iced::Rectangle>,
    },
    Key {
        state: String,
        modifiers: iced::keyboard::Modifiers,
    },
}

impl Action {
    pub(crate) fn animation_frame() -> Self {
        Self {
            key: String::new(),
            value: None,
            event: Event::AnimationFrame,
        }
    }

    pub(crate) fn is_animation_frame(&self) -> bool {
        matches!(self.event, Event::AnimationFrame)
    }
    pub(crate) fn change(key: String, value: ActionValue) -> Self {
        Self {
            key,
            value: Some(value),
            event: Event::Action,
        }
    }

    pub(crate) fn arguments(&self, lua: &mlua::Lua) -> mlua::Result<mlua::MultiValue> {
        if self.is_animation_frame() {
            return Err(mlua::Error::RuntimeError(
                "native animation frames cannot enter Lua".into(),
            ));
        }
        let events = lua.create_table()?;
        events.set(
            "type",
            match &self.event {
                Event::Action | Event::AnimationFrame => "action",
                Event::Mouse { state, .. } => state,
                Event::Key { .. } => "key",
            },
        )?;
        if !self.key.is_empty() {
            events.set("key", self.key.as_str())?;
        }
        match &self.event {
            Event::Mouse {
                button, position, ..
            } => {
                events.set("button", button.as_deref())?;
                if let Some(position) = position {
                    events.set("x", position.x)?;
                    events.set("y", position.y)?;
                }
            }
            Event::Key { state, modifiers } => {
                events.set("state", state.as_str())?;
                let flags = lua.create_table()?;
                flags.set("shift", modifiers.shift())?;
                flags.set("control", modifiers.control())?;
                flags.set("alt", modifiers.alt())?;
                flags.set("super", modifiers.logo())?;
                events.set("modifiers", flags)?;
            }
            Event::Action | Event::AnimationFrame => {}
        }
        if let Some(value) = &self.value {
            events.set(
                "value",
                match value {
                    ActionValue::Boolean(value) => mlua::Value::Boolean(*value),
                    ActionValue::Number(value) => mlua::Value::Number(f64::from(*value)),
                    ActionValue::Text(value) => mlua::Value::String(lua.create_string(value)?),
                },
            )?;
        }
        Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Table(events)]))
    }

    pub(crate) fn click() -> Self {
        Self {
            key: String::new(),
            value: None,
            event: Event::Mouse {
                state: "click".into(),
                button: Some("left".into()),
                position: None,
                origin: None,
                bounds: None,
            },
        }
    }
}

impl From<String> for Action {
    fn from(key: String) -> Self {
        Self {
            key,
            value: None,
            event: Event::Action,
        }
    }
}
impl From<&str> for Action {
    fn from(key: &str) -> Self {
        key.to_owned().into()
    }
}

pub(crate) type Sender<M = crate::shell::Plant> = std::sync::Arc<dyn Fn(Action) -> M + Send + Sync>;

/// Hover enter/exit reports: `(list id, item key, entered)`. Unlike [`Sender`],
/// these never invoke Lua `actions` — hosts update their hover sets and
/// re-render the affected view with the new flags.
pub(crate) type HoverSender<M = crate::shell::Plant> =
    std::sync::Arc<dyn Fn(String, String, bool) -> M + Send + Sync>;
