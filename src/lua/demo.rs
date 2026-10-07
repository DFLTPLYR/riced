//! Runnable M0 plain-window demo. iced view is pure: Lua renders dirty IR
//! in the host update/boot, then view realizes the cached owned tree.
use super::{LuaRuntime, Message, WindowId};
use crate::app::layers::top::build_node;
use iced::{
    Element, Length, Task,
    widget::{button, column, container, text},
};
use include_dir::{Dir, include_dir};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

static SCRIPTS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/scripts");
const MAIN_WINDOW: WindowId = WindowId(1);

struct Demo {
    runtime: Option<LuaRuntime>,
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    bootstrap_error: Option<String>,
}

impl Demo {
    fn new(path: Option<PathBuf>) -> Self {
        let runtime = LuaRuntime::new();
        let (runtime, bootstrap_error) = match runtime {
            Ok(runtime) => (Some(runtime), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut demo = Self {
            runtime,
            path,
            mtime: None,
            bootstrap_error,
        };
        demo.reload();
        demo
    }

    fn reload(&mut self) {
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        let loaded = if let Some(path) = &self.path {
            self.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            runtime.load_path(path)
        } else {
            runtime.load(
                SCRIPTS
                    .get_file("main.lua")
                    .and_then(|file| file.contents_utf8())
                    .expect("embedded main.lua"),
            )
        };
        if loaded.is_ok() {
            // M0 only has the host reload/GC timer. User subscription
            // descriptors are added at the M4 boundary.
            let _subscriptions = runtime.subscriptions();
            let _ = runtime.view(MAIN_WINDOW);
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match &message {
            Message::Reload => self.reload(),
            Message::Tick => {
                if let Some(path) = &self.path {
                    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
                    if mtime != self.mtime {
                        self.reload();
                    }
                }
                if let Some(runtime) = &mut self.runtime {
                    runtime.tick();
                    // Optional named host event, not a user subscription:
                    // static apps with no on_tick stay dirty-gated.
                    if runtime.has_handler("on_tick") {
                        let event = Message::LuaEvent {
                            handler: "on_tick".into(),
                            payload: serde_json::json!({"interval_ms": 500}),
                        };
                        if runtime.update(&event).is_ok() {
                            let _ = runtime.view(MAIN_WINDOW);
                        }
                    }
                }
            }
            Message::LuaEvent { .. } => {
                if let Some(runtime) = &mut self.runtime
                    && runtime.update(&message).is_ok()
                {
                    let _ = runtime.view(MAIN_WINDOW);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut content = column![].spacing(12);
        if let Some(error) = self.bootstrap_error.as_deref() {
            content = content.push(text(format!("Lua bootstrap error: {error}")));
        }
        if let Some(runtime) = &self.runtime {
            if let Some(error) = &runtime.last_error {
                content = content.push(
                    container(
                        column![
                            text(format!("Lua error in {}", error.entry)).size(18),
                            text(error.message.clone()),
                            button("Reload Lua").on_press(Message::Reload),
                        ]
                        .spacing(8),
                    )
                    .padding(12)
                    .width(Length::Fill)
                    .style(iced::widget::container::danger),
                );
            }
            if let Some(node) = runtime.cached_view(MAIN_WINDOW) {
                match build_node(node, 14.0, None) {
                    // M0 is static. M2 routes CallbackId messages instead.
                    Ok(element) => content = content.push(element.map(|_| Message::Tick)),
                    Err(error) => content = content.push(text(format!("IR error: {error}"))),
                }
            }
        }
        iced::widget::scrollable(container(content).padding(16).width(Length::Fill))
            .height(Length::Fill)
            .into()
    }
}

pub fn run(path: Option<PathBuf>) -> iced::Result {
    let override_path = path.or_else(|| {
        let path = crate::config::config_path().with_file_name("main.lua");
        path.is_file().then_some(path)
    });
    iced::application(
        move || Demo::new(override_path.clone()),
        Demo::update,
        Demo::view,
    )
    .title("riced — Lua runtime M0")
    .window_size((560.0, 360.0))
    .subscription(|_: &Demo| iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick))
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_app_builds_the_entire_window_content() {
        let demo = Demo::new(None);
        let runtime = demo.runtime.as_ref().unwrap();
        assert!(runtime.last_error.is_none());
        assert!(runtime.cached_view(MAIN_WINDOW).is_some());
        let _element = demo.view();
    }
    #[test]
    fn missing_override_has_a_visible_error_view() {
        let demo = Demo::new(Some(PathBuf::from("/nonexistent/riced-m0/main.lua")));
        assert!(demo.runtime.as_ref().unwrap().last_error.is_some());
        let _element = demo.view();
    }
}
