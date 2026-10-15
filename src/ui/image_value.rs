//! Opaque image tokens decode into owned native handles, never Lua references.
#[derive(Debug, Clone)]
pub(crate) struct ImageValue(pub iced::widget::image::Handle);
impl mlua::UserData for ImageValue {}
pub(crate) fn handle(value: mlua::Value) -> mlua::Result<iced::widget::image::Handle> {
    let mlua::Value::UserData(value) = value else {
        return Err(mlua::Error::RuntimeError(
            "expected an owned image token".into(),
        ));
    };
    Ok(value.borrow::<ImageValue>()?.0.clone())
}
