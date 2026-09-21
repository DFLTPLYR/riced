use iced::widget::{container, mouse_area, text};
use iced::{Color, Element, Fill, Point, Task as Command};
use iced_exwlshell::redraw::Scope;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::collections::HashMap;

use iced_wayland_subscriber::OutputId;
use iced_wayland_subscriber::shell::{ShellEvent, ShellReceiver};

use super::layers::Top;
use super::{Plant, WayEvent};

#[derive(Debug)]
pub struct Plots {
    ids: HashMap<iced::window::Id, PlotInfo>,
    /// one Top per output
    tops: HashMap<OutputId, Top>,
    top_ids: HashMap<OutputId, iced::window::Id>,
    shell_events: ShellReceiver,
    // mouse drag state per window
    last_cursor: HashMap<iced::window::Id, Point>,
    drag_start: HashMap<iced::window::Id, Point>,
    dragging: HashMap<iced::window::Id, bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlotInfo {
    Top(OutputId),
}

impl Plots {
    pub fn new(shell_events: ShellReceiver) -> Self {
        Self {
            ids: HashMap::new(),
            tops: HashMap::new(),
            top_ids: HashMap::new(),
            shell_events,
            last_cursor: HashMap::new(),
            drag_start: HashMap::new(),
            dragging: HashMap::new(),
        }
    }

    pub fn id_info(&self, id: iced::window::Id) -> Option<PlotInfo> {
        self.ids.get(&id).copied()
    }

    pub fn namespace() -> String {
        String::from("Riced")
    }

    pub fn subscription(&self) -> iced::Subscription<Plant> {
        iced::Subscription::batch(vec![
            iced::event::listen_with(|event, _status, id| Some(Plant::Graft(id, event))),
            self.shell_events.listen().filter_map(|event| match event {
                ShellEvent::OutputAdded(output) => {
                    Some(Plant::Wayland(WayEvent::OutputInsert(output)))
                }
                ShellEvent::OutputRemoved(output) => Some(Plant::Wayland(WayEvent::OutputRemoved(
                    OutputId::from(&output),
                ))),
                _ => None,
            }),
        ])
    }

    pub fn view(&self, id: iced::window::Id) -> Element<'_, Plant> {
        // dragging overlay for every surface (background + Top)
        let is_dragging = self.dragging.get(&id).copied().unwrap_or(false);
        let cursor = self.last_cursor.get(&id).copied();
        let drag_start = self.drag_start.get(&id).copied();

        // helper to build a fullscreen interactive layer for daemon background
        let background_view = {
            let label = if is_dragging {
                if let (Some(start), Some(cur)) = (drag_start, cursor) {
                    format!(
                        "DRAG {start:?} -> {cur:?} (dx:{:.0} dy:{:.0})",
                        cur.x - start.x,
                        cur.y - start.y
                    )
                } else {
                    "dragging...".to_string()
                }
            } else if let Some(pos) = cursor {
                format!("click+drag me  cursor {pos:?}")
            } else {
                "click+drag me  (move cursor)".to_string()
            };
            let bg = if is_dragging {
                Color::from_rgba(0.2, 0.5, 0.9, 0.35)
            } else {
                Color::from_rgba(0.0, 0.0, 0.0, 0.15)
            };
            mouse_area(
                container(text(label).size(16).color(Color::WHITE))
                    .width(Fill)
                    .height(Fill)
                    .center_x(Fill)
                    .center_y(Fill)
                    .style(move |_| container::Style {
                        background: Some(bg.into()),
                        border: iced::Border {
                            color: if is_dragging {
                                Color::from_rgb(0.3, 0.7, 1.0)
                            } else {
                                Color::TRANSPARENT
                            },
                            width: 2.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .on_press(Plant::Grow)
            .into()
        };

        match self.id_info(id) {
            Some(PlotInfo::Top(output)) => {
                // Wrap Top view with drag indicator border
                if is_dragging || cursor.is_some() {
                    // show Top plus drag overlay below it
                    self.tops
                        .get(&output)
                        .map(|t| {
                            // keep Top's own view, but indicate drag state via border
                            let top_el = t.view();
                            // For Top bar itself, overlay drag feedback as container
                            container(top_el)
                                .style(move |_| container::Style {
                                    border: iced::Border {
                                        color: if is_dragging {
                                            Color::from_rgb(1.0, 0.6, 0.2)
                                        } else {
                                            Color::TRANSPARENT
                                        },
                                        width: 2.0,
                                        radius: 0.0.into(),
                                    },
                                    ..Default::default()
                                })
                                .into()
                        })
                        .unwrap_or(background_view)
                } else {
                    self.tops
                        .get(&output)
                        .map(|t| t.view())
                        .unwrap_or(background_view)
                }
            }
            None => background_view,
        }
    }

    pub fn update(&mut self, message: Plant) -> Command<Plant> {
        match message {
            Plant::Grow => {
                if self.tops.is_empty() {
                    let top = Top::new();
                    let (id, settings) = top.open_active();
                    let sentinel = OutputId(u32::MAX);
                    self.tops.insert(sentinel, top);
                    self.top_ids.insert(sentinel, id);
                    self.ids.insert(id, PlotInfo::Top(sentinel));
                    return Command::done(Plant::NewLayerShell { settings, id });
                }
                Command::none()
            }
            Plant::Uproot(id) => {
                self.last_cursor.remove(&id);
                self.drag_start.remove(&id);
                self.dragging.remove(&id);
                if let Some(info) = self.ids.remove(&id) {
                    match info {
                        PlotInfo::Top(output) => {
                            self.tops.remove(&output);
                            self.top_ids.remove(&output);
                        }
                    }
                }
                Command::none()
            }
            Plant::Tend => Command::none(),
            Plant::Wayland(WayEvent::OutputInsert(output)) => {
                let output_id = OutputId::from(&output);
                if self.tops.contains_key(&output_id) {
                    return Command::none();
                }
                if let Some(sentinel_id) = self.top_ids.remove(&OutputId(u32::MAX)) {
                    self.tops.remove(&OutputId(u32::MAX));
                    self.ids.remove(&sentinel_id);
                    self.last_cursor.remove(&sentinel_id);
                    self.drag_start.remove(&sentinel_id);
                    self.dragging.remove(&sentinel_id);
                    let mut cmds = vec![iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(sentinel_id),
                    ))];
                    let top = Top::new();
                    let (id, settings) = top.open(output_id.0);
                    self.tops.insert(output_id, top);
                    self.top_ids.insert(output_id, id);
                    self.ids.insert(id, PlotInfo::Top(output_id));
                    cmds.push(Command::done(Plant::NewLayerShell { settings, id }));
                    return Command::batch(cmds);
                }
                let top = Top::new();
                let (id, settings) = top.open(output_id.0);
                self.tops.insert(output_id, top);
                self.top_ids.insert(output_id, id);
                self.ids.insert(id, PlotInfo::Top(output_id));
                Command::done(Plant::NewLayerShell { settings, id })
            }
            Plant::Wayland(WayEvent::OutputRemoved(output)) => {
                if let Some(id) = self.top_ids.remove(&output) {
                    self.tops.remove(&output);
                    self.ids.remove(&id);
                    self.last_cursor.remove(&id);
                    self.drag_start.remove(&id);
                    self.dragging.remove(&id);
                    return iced_runtime::task::effect(Action::Window(WindowAction::Close(id)));
                }
                Command::none()
            }
            Plant::Graft(id, event) => {
                use iced::Event;
                // track cursor for drag
                if let Event::Mouse(iced::mouse::Event::CursorMoved { position }) = &event {
                    self.last_cursor.insert(id, *position);
                    if self.dragging.get(&id).copied().unwrap_or(false) {
                        println!("drag {id:?} -> {position:?}");
                    }
                    return Command::none();
                }
                match &event {
                    Event::Mouse(iced::mouse::Event::ButtonPressed(btn)) => {
                        let pos = self
                            .last_cursor
                            .get(&id)
                            .copied()
                            .unwrap_or(Point::new(0.0, 0.0));
                        self.drag_start.insert(id, pos);
                        self.dragging.insert(id, true);
                        println!("click {id:?} {btn:?} at {pos:?}");
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::ButtonReleased(btn)) => {
                        let pos = self
                            .last_cursor
                            .get(&id)
                            .copied()
                            .unwrap_or(Point::new(0.0, 0.0));
                        let start = self.drag_start.remove(&id);
                        self.dragging.insert(id, false);
                        if let Some(s) = start {
                            println!(
                                "release {id:?} {btn:?} start {s:?} -> {pos:?} delta ({:.0},{:.0})",
                                pos.x - s.x,
                                pos.y - s.y
                            );
                        } else {
                            println!("release {id:?} {btn:?} at {pos:?}");
                        }
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::CursorEntered) => {
                        println!("enter {id:?}");
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::CursorLeft) => {
                        println!("leave {id:?}");
                        Command::none()
                    }
                    _ => Command::none(),
                }
            }
            // layershell actions generated by macro (NewLayerShell etc) are
            // intercepted by iced_exwlshell via TryInto, never reach here
            _ => Command::none(),
        }
    }
}

pub fn redraw_scope(message: &Plant) -> Scope {
    match message {
        Plant::Graft(id, _) => Scope::Window(*id),
        Plant::Wayland(_) => Scope::All,
        _ => Scope::All,
    }
}
