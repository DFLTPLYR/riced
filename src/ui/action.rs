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
}

impl Action {
    pub(crate) fn change(key: String, value: ActionValue) -> Self {
        Self {
            key,
            value: Some(value),
        }
    }

    pub(crate) fn arguments(&self, lua: &mlua::Lua) -> mlua::Result<mlua::MultiValue> {
        let mut args = vec![mlua::Value::String(lua.create_string(&self.key)?)];
        if let Some(value) = &self.value {
            args.push(match value {
                ActionValue::Boolean(value) => mlua::Value::Boolean(*value),
                ActionValue::Number(value) => mlua::Value::Number(f64::from(*value)),
                ActionValue::Text(value) => mlua::Value::String(lua.create_string(value)?),
            });
        }
        Ok(mlua::MultiValue::from_vec(args))
    }
}

impl From<String> for Action {
    fn from(key: String) -> Self {
        Self { key, value: None }
    }
}
impl From<&str> for Action {
    fn from(key: &str) -> Self {
        key.to_owned().into()
    }
}

pub(crate) type Sender<M = crate::shell::Plant> = std::sync::Arc<dyn Fn(Action) -> M + Send + Sync>;
