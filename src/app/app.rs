use iced::widget::{Space, button, canvas, column, container, stack, text};
use iced::{Color, Element, Event, Fill, Length, Point, Rectangle, Task as Command};
use iced_exwlshell::redraw::Scope;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use iced_wayland_subscriber::OutputId;
use iced_wayland_subscriber::shell::{ShellEvent, ShellReceiver};

use super::layers::{Background, Top};
use super::{LandEvent, Plant};
use iced_exwlshell::reexport::Anchor;
use iced_wayland_subscriber::OutputInfo;

/// Mirrors Quickshell QtObject selectionRect
#[derive(Debug, Clone, Default)]
struct SelectionRect {
    start_point: Option<Point>, // global
    selecting: bool,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl SelectionRect {
    fn set_selecting(&mut self, selecting: bool) {
        if self.selecting != selecting {
            self.selecting = selecting;
            if !selecting {
                // onSelectingChanged: if (!selecting) { width=0; height=0; startPoint=null }
                self.width = 0.0;
                self.height = 0.0;
                self.start_point = None;
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ContextMenu {
    x: f32,
    y: f32,
    open: bool,
    output: Option<OutputId>,
}

static LAST_CURSOR_GLOBAL: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<iced::window::Id, Point>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

// Lightweight canvas for selection rect — single draw call vs container+stack layout
#[derive(Debug)]
struct SelCanvas {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    opacity: f32,
}

impl canvas::Program<Plant> for SelCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        if self.w > 1.0 && self.h > 1.0 && self.opacity > 0.01 {
            let rect = Rectangle {
                x: self.x,
                y: self.y,
                width: self.w,
                height: self.h,
            };
            // fill
            frame.fill_rectangle(
                rect.position(),
                rect.size(),
                Color::from_rgba(0.55, 0.65, 1.0, 0.5 * self.opacity),
            );
            // border
            frame.stroke_rectangle(
                rect.position(),
                rect.size(),
                canvas::Stroke {
                    style: canvas::Style::Solid(Color::from_rgba(0.75, 0.8, 1.0, self.opacity)),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }
        vec![frame.into_geometry()]
    }
}

static LAST_MOUSE_MOVE: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));
// QML hoverEnabled: Background.selectionRect.selecting — only track moves while dragging
static SELECTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn throttled_graft(
    event: Event,
    _status: iced::event::Status,
    id: iced::window::Id,
) -> Option<Plant> {
    // Store last cursor globally for correct startPoint even when not selecting (fixes random startPoint)
    if let Event::Mouse(iced::mouse::Event::CursorMoved { position }) = &event {
        // always update global last cursor (throttled) so ButtonPressed has correct pos
        let now = Instant::now();
        let mut last = LAST_MOUSE_MOVE.lock().unwrap();
        let should_emit = SELECTING.load(std::sync::atomic::Ordering::Relaxed);
        // throttle to 60fps
        if now.duration_since(*last) < Duration::from_millis(16) {
            // still update global cursor even if throttled for emit, so press is accurate
            LAST_CURSOR_GLOBAL.lock().unwrap().insert(id, *position);
            return None;
        }
        *last = now;
        LAST_CURSOR_GLOBAL.lock().unwrap().insert(id, *position);
        if !should_emit {
            return None;
        }
    }
    if let Event::Mouse(_) = event {
        Some(Plant::Graft(id, event))
    } else {
        None
    }
}

#[derive(Debug)]
pub struct Plots {
    ids: HashMap<iced::window::Id, PlotInfo>,
    tops: HashMap<iced::window::Id, Top>,
    backgrounds: HashMap<OutputId, Background>,
    background_ids: HashMap<OutputId, iced::window::Id>,
    shell_events: ShellReceiver,
    // per-window cursor
    last_cursor: HashMap<iced::window::Id, Point>,
    // global selection rect (single, like Background.selectionRect)
    selection_rect: SelectionRect,
    // fade animation after select end (QML Behavior on opacity 150ms InOutQuad)
    fade_rect: Option<SelectionRect>,
    fade_start: Option<Instant>,
    // per-output geometry for clipping (panel.screen)
    output_infos: HashMap<OutputId, OutputInfo>,
    // context menu state (global, like Background contextMenu)
    context_menu: Option<ContextMenu>,
    // throttling for smooth 60fps selection updates
    last_selection_tick: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlotInfo {
    Background(OutputId),
    Top(OutputId),
}

impl Plots {
    pub fn new(shell_events: ShellReceiver) -> Self {
        Self {
            ids: HashMap::new(),
            tops: HashMap::new(),
            backgrounds: HashMap::new(),
            background_ids: HashMap::new(),
            shell_events,
            last_cursor: HashMap::new(),
            output_infos: HashMap::new(),
            selection_rect: SelectionRect::default(),
            fade_rect: None,
            fade_start: None,
            context_menu: None,
            last_selection_tick: None,
        }
    }

    pub fn id_info(&self, id: iced::window::Id) -> Option<PlotInfo> {
        self.ids.get(&id).copied()
    }

    pub fn namespace() -> String {
        String::from("Riced")
    }

    pub fn subscription(&self) -> iced::Subscription<Plant> {
        let shell_sub = self.shell_events.listen().filter_map(|event| match event {
            ShellEvent::NewShell(info) => Some(Plant::Wayland(LandEvent::NewShell(info))),
            ShellEvent::Closed(id) => Some(Plant::Wayland(LandEvent::Closed(id))),
            ShellEvent::WindowOutputChanged { window, output } => {
                Some(Plant::Wayland(LandEvent::WindowOutputChanged {
                    window,
                    output,
                }))
            }
            ShellEvent::OutputAdded(output) => Some(Plant::Wayland(LandEvent::OutputAdded(output))),
            ShellEvent::OutputUpdated(output) => {
                Some(Plant::Wayland(LandEvent::OutputUpdated(output)))
            }
            ShellEvent::OutputRemoved(output) => {
                Some(Plant::Wayland(LandEvent::OutputRemoved(output)))
            }
            ShellEvent::Locked => Some(Plant::Wayland(LandEvent::Locked)),
            ShellEvent::LockDenied => Some(Plant::Wayland(LandEvent::LockDenied)),
            ShellEvent::LockedFinished => Some(Plant::Wayland(LandEvent::LockedFinished)),
        });

        let mut subs = vec![iced::event::listen_with(throttled_graft), shell_sub];

        // Only tick for fade animation (selecting is driven by throttled mouse moves, not timer)
        // QML Behavior 150ms InOutQuad on opacity needs 60fps ticks only while fading
        if self.fade_start.is_some() {
            subs.push(iced::time::every(Duration::from_millis(16)).map(|_| Plant::SelectionTick));
        }

        iced::Subscription::batch(subs)
    }

    /// map local widget coords to global compositor coords (mapToGlobal)
    fn to_global(&self, id: iced::window::Id, local: Point) -> Point {
        if let Some(info) = self.id_info(id).and_then(|info| match info {
            PlotInfo::Top(o) | PlotInfo::Background(o) => self.output_infos.get(&o),
        }) {
            if let Some((lx, ly)) = info.logical_position {
                return Point::new(local.x + lx as f32, local.y + ly as f32);
            }
        }
        // fallback for daemon's tiny 1x1 window (no PlotInfo) - find output that contains cursor or first
        local
    }

    /// Utils.intersects(a,b) for global rect vs screen
    fn intersects(
        a: &SelectionRect,
        screen_x: f32,
        screen_y: f32,
        screen_w: f32,
        screen_h: f32,
    ) -> bool {
        if a.width <= 0.0 || a.height <= 0.0 {
            return false;
        }
        let ax2 = a.x + a.width;
        let ay2 = a.y + a.height;
        let bx2 = screen_x + screen_w;
        let by2 = screen_y + screen_h;
        !(ax2 <= screen_x || a.x >= bx2 || ay2 <= screen_y || a.y >= by2)
    }

    pub fn view(&self, id: iced::window::Id) -> Element<'_, Plant> {
        // For per-output Background windows, render fullscreen selectionRect clipped to that output's screen.
        // For Top windows, just render top bar. For daemon's tiny 1x1 window (None), empty.
        match self.id_info(id) {
            Some(PlotInfo::Background(output)) => {
                let info = self.output_infos.get(&output);
                // Use same geometry resolver as AddTop/hit-test (logical -> location+mode -> fallback)
                let (screen_x, screen_y, screen_w, screen_h) = if let Some(info) = info {
                    let (sx, sy) = info
                        .logical_position
                        .unwrap_or((info.location.0, info.location.1));
                    let (sw, sh) = info.logical_size.unwrap_or_else(|| {
                        info.modes
                            .iter()
                            .find(|m| m.current)
                            .map(|m| m.dimensions)
                            .unwrap_or((1920, 1080))
                    });
                    (sx as f32, sy as f32, sw as f32, sh as f32)
                } else {
                    (0.0, 0.0, 1920.0, 1080.0)
                };

                // active rect is either selecting rect or fading rect (150ms InOutQuad)
                let (active_rect, opacity) = if self.selection_rect.selecting {
                    (Some(&self.selection_rect), 1.0)
                } else if let (Some(fr), Some(start)) = (&self.fade_rect, &self.fade_start) {
                    let elapsed = start.elapsed().as_millis() as f32;
                    if elapsed >= 150.0 {
                        (None, 0.0)
                    } else {
                        let p = elapsed / 150.0;
                        let eased = if p < 0.5 {
                            2.0 * p * p
                        } else {
                            -1.0 + (4.0 - 2.0 * p) * p
                        };
                        (Some(fr), 1.0 - eased)
                    }
                } else {
                    (None, 0.0)
                };

                let (visible, clipped_w, clipped_h, local_x, local_y) =
                    if let Some(ar) = active_rect {
                        let inter = Self::intersects(ar, screen_x, screen_y, screen_w, screen_h);
                        let vis = inter && opacity > 0.01;
                        let cw = (ar.x + ar.width).min(screen_x + screen_w) - ar.x.max(screen_x);
                        let ch = (ar.y + ar.height).min(screen_y + screen_h) - ar.y.max(screen_y);
                        let lx = ar.x.max(screen_x) - screen_x;
                        let ly = ar.y.max(screen_y) - screen_y;
                        (vis, cw, ch, lx, ly)
                    } else {
                        (false, 0.0, 0.0, 0.0, 0.0)
                    };

                // QML Rectangle-like: container with background + border (not canvas lines)
                let (cx, cy, cw, ch, op) = if visible && clipped_w > 0.0 && clipped_h > 0.0 {
                    (local_x, local_y, clipped_w, clipped_h, opacity)
                } else {
                    (0.0, 0.0, 0.0, 0.0, 0.0)
                };
                let selection_overlay: Element<'_, Plant> = if cw > 1.0 && ch > 1.0 && op > 0.01 {
                    let bg = Color::from_rgba(0.55, 0.65, 1.0, 0.5 * op);
                    let border_col = Color::from_rgba(0.75, 0.8, 1.0, op);
                    container(
                        container(Space::new())
                            .width(Length::Fixed(cw))
                            .height(Length::Fixed(ch))
                            .style(move |_| container::Style {
                                background: Some(bg.into()),
                                border: iced::Border {
                                    color: border_col,
                                    width: 1.0,
                                    radius: 0.0.into(),
                                },
                                ..Default::default()
                            }),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(iced::Padding {
                        top: cy,
                        left: cx,
                        right: 0.0,
                        bottom: 0.0,
                    })
                    .into()
                } else {
                    // keep same widget type (container Fill) to avoid stack child swap leaving lines
                    container(Space::new()).width(Fill).height(Fill).into()
                };

                let context_menu_overlay: Element<'_, Plant> = if let Some(cm) = &self.context_menu
                {
                    if cm.open {
                        // QML Menu behavior: keep menu fully visible within screen (flip/clamp)
                        // Use fixed menu size so clamping and hit-test match rendered size (like QML popup)
                        const MENU_W: f32 = 180.0;
                        const MENU_H: f32 = 92.0;
                        let lx = cm.x - screen_x;
                        let ly = cm.y - screen_y;
                        // only show on the output that contains the click (like QML panel.screen)
                        let in_screen = lx >= 0.0 && ly >= 0.0 && lx < screen_w && ly < screen_h;
                        if in_screen {
                            // clamp to stay fully inside screen with no extra margin (QML Menu flips to stay viewable)
                            // Keep menu exactly viewable without the previous 4px/2px gap that caused "too much margin" on right
                            let clamped_lx = lx.clamp(0.0, (screen_w - MENU_W).max(0.0));
                            let clamped_ly = ly.clamp(0.0, (screen_h - MENU_H).max(0.0));
                            container(
                                container(
                                    column![
                                        text("Context Menu\n(Right clicked)\nLeft drag to select")
                                            .size(12)
                                            .color(Color::WHITE),
                                        button(text("Add Top").size(12).color(Color::WHITE))
                                            .on_press(Plant::AddTop)
                                            .padding(6)
                                            .style(|_, _| button::Style {
                                                background: Some(
                                                    Color::from_rgb(0.25, 0.25, 0.28).into()
                                                ),
                                                text_color: Color::WHITE,
                                                border: iced::Border {
                                                    color: Color::from_rgb(0.5, 0.5, 0.55),
                                                    width: 1.0,
                                                    radius: 4.0.into(),
                                                },
                                                ..Default::default()
                                            })
                                    ]
                                    .spacing(8),
                                )
                                .padding(8)
                                .width(Length::Fixed(MENU_W))
                                .height(Length::Fixed(MENU_H))
                                .style(|_| container::Style {
                                    background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
                                    border: iced::Border {
                                        color: Color::from_rgb(0.5, 0.5, 0.55),
                                        width: 1.0,
                                        radius: 6.0.into(),
                                    },
                                    ..Default::default()
                                }),
                            )
                            .width(Fill)
                            .height(Fill)
                            .padding(iced::Padding {
                                top: clamped_ly,
                                left: clamped_lx,
                                right: 0.0,
                                bottom: 0.0,
                            })
                            .into()
                        } else {
                            Space::new().width(0).height(0).into()
                        }
                    } else {
                        Space::new().width(0).height(0).into()
                    }
                } else {
                    Space::new().width(0).height(0).into()
                };

                let cursor = self.last_cursor.get(&id).copied();
                let sr = &self.selection_rect;
                let bg_label = if sr.selecting {
                    format!(
                        "selecting {}x{} at {:.0},{:.0} | screen {:.0},{:.0} {}x{}",
                        sr.width as i32,
                        sr.height as i32,
                        sr.x,
                        sr.y,
                        screen_x,
                        screen_y,
                        screen_w as i32,
                        screen_h as i32
                    )
                } else if let Some(p) = cursor {
                    format!(
                        "BG click+drag  cursor local {p:?} global {:.0},{:.0}",
                        screen_x + p.x,
                        screen_y + p.y
                    )
                } else {
                    "BG click+drag  (move cursor)  | right click for menu".to_string()
                };
                let bg_view = container(
                    text(bg_label)
                        .size(13)
                        .color(Color::from_rgba(1.0, 1.0, 1.0, 0.7)),
                )
                .width(Fill)
                .height(Fill)
                .center_x(Fill)
                .center_y(Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.10).into()),
                    ..Default::default()
                })
                .into();

                stack(vec![bg_view, selection_overlay, context_menu_overlay]).into()
            }
            Some(PlotInfo::Top(_output)) => self
                .tops
                .get(&id)
                .map(|t| t.view())
                .unwrap_or_else(|| Space::new().into()),
            None => Space::new().into(), // daemon's 1x1 tiny window
        }
    }

    pub fn update(&mut self, message: Plant) -> Command<Plant> {
        match message {
            Plant::Grow => {
                if self.tops.is_empty() {
                    let top = Top::new();
                    let (id, settings) = top.open_active();
                    let sentinel = OutputId(u32::MAX);
                    self.tops.insert(id, top);
                    self.ids.insert(id, PlotInfo::Top(sentinel));
                    return Command::done(Plant::NewLayerShell { settings, id });
                }
                Command::none()
            }
            Plant::Uproot(id) => {
                self.last_cursor.remove(&id);
                if let Some(info) = self.ids.remove(&id) {
                    match info {
                        PlotInfo::Top(_) => {
                            self.tops.remove(&id);
                        }
                        PlotInfo::Background(output) => {
                            self.backgrounds.remove(&output);
                            self.background_ids.remove(&output);
                        }
                    }
                }
                Command::none()
            }
            Plant::Tend => Command::none(),
            Plant::Wayland(LandEvent::OutputAdded(output)) => {
                let output_id = OutputId::from(&output);
                // store geometry for panel.screen clipping
                self.output_infos.insert(output_id, output.clone());
                let mut cmds = Vec::new();
                // sentinel Top cleanup (from new_with_boot) - remove any window with sentinel OutputId
                let sentinel = OutputId(u32::MAX);
                let sentinel_ids: Vec<iced::window::Id> = self
                    .ids
                    .iter()
                    .filter_map(|(wid, info)| match info {
                        PlotInfo::Top(o) if *o == sentinel => Some(*wid),
                        _ => None,
                    })
                    .collect();
                for sentinel_id in sentinel_ids {
                    self.tops.remove(&sentinel_id);
                    self.ids.remove(&sentinel_id);
                    self.last_cursor.remove(&sentinel_id);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(sentinel_id),
                    )));
                }
                // Background fullscreen per output for selection (like Quickshell Background)
                if !self.backgrounds.contains_key(&output_id) {
                    let bg = Background;
                    let (id, settings) = bg.open(output_id.0);
                    self.backgrounds.insert(output_id, bg);
                    self.background_ids.insert(output_id, id);
                    self.ids.insert(id, PlotInfo::Background(output_id));
                    cmds.push(Command::done(Plant::NewLayerShell { settings, id }));
                }
                // No auto Top bar on start - user creates via Add Top context menu
                if cmds.is_empty() {
                    Command::none()
                } else {
                    Command::batch(cmds)
                }
            }
            Plant::Wayland(LandEvent::OutputUpdated(output)) => {
                let output_id = OutputId::from(&output);
                self.output_infos.insert(output_id, output);
                Command::none()
            }
            Plant::Wayland(LandEvent::OutputRemoved(output)) => {
                let output_id = OutputId::from(&output);
                let mut cmds = Vec::new();
                if let Some(id) = self.background_ids.remove(&output_id) {
                    self.backgrounds.remove(&output_id);
                    self.ids.remove(&id);
                    self.last_cursor.remove(&id);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(id),
                    )));
                }
                // remove all tops for this output (support multiple per output)
                let top_ids_to_remove: Vec<iced::window::Id> = self
                    .ids
                    .iter()
                    .filter_map(|(wid, info)| match info {
                        PlotInfo::Top(o) if *o == output_id => Some(*wid),
                        _ => None,
                    })
                    .collect();
                for wid in top_ids_to_remove {
                    self.tops.remove(&wid);
                    self.ids.remove(&wid);
                    self.last_cursor.remove(&wid);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(wid),
                    )));
                }
                self.output_infos.remove(&output_id);
                // clear global selection if it was on removed output (will hide via intersect check)
                if cmds.is_empty() {
                    Command::none()
                } else {
                    Command::batch(cmds)
                }
            }
            Plant::Wayland(LandEvent::NewShell(_)) => Command::none(),
            Plant::Wayland(LandEvent::Closed(_)) => Command::none(),
            Plant::Wayland(LandEvent::WindowOutputChanged { .. }) => Command::none(),
            Plant::Wayland(LandEvent::Locked) => Command::none(),
            Plant::Wayland(LandEvent::LockDenied) => Command::none(),
            Plant::Wayland(LandEvent::LockedFinished) => Command::none(),
            Plant::Graft(id, event) => {
                use iced::Event;
                use iced::mouse::Button;

                // track cursor — throttled to 60fps for smoothness, prevents TrySendError Full
                if let Event::Mouse(iced::mouse::Event::CursorMoved { position }) = &event {
                    self.last_cursor.insert(id, *position);
                    // onPositionChanged equivalent - while selecting, update global rect
                    if self.selection_rect.selecting {
                        // throttle to ~60fps (16ms) — avoids flooding subscription channel
                        let now = Instant::now();
                        if let Some(last) = self.last_selection_tick {
                            if now.duration_since(last) < Duration::from_millis(16) {
                                return Command::none();
                            }
                        }
                        self.last_selection_tick = Some(now);
                        if let Some(sp) = self.selection_rect.start_point {
                            let gp = self.to_global(id, *position);
                            let min_x = sp.x.min(gp.x);
                            let min_y = sp.y.min(gp.y);
                            let max_x = sp.x.max(gp.x);
                            let max_y = sp.y.max(gp.y);
                            // skip tiny moves <1px to reduce choppy updates
                            if (self.selection_rect.x - min_x).abs() < 0.5
                                && (self.selection_rect.y - min_y).abs() < 0.5
                                && (self.selection_rect.width - (max_x - min_x)).abs() < 0.5
                                && (self.selection_rect.height - (max_y - min_y)).abs() < 0.5
                            {
                                return Command::none();
                            }
                            self.selection_rect.x = min_x;
                            self.selection_rect.y = min_y;
                            self.selection_rect.width = max_x - min_x;
                            self.selection_rect.height = max_y - min_y;
                        }
                        // trigger All redraw via SelectionTick (Graft itself is None to avoid flood)
                        return Command::done(Plant::SelectionTick);
                    }
                    return Command::none();
                }

                match &event {
                    // onPressed
                    Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Right)) => {
                        // only open context menu on Background (like QML Background MouseArea) – ignore Top layer
                        if !matches!(self.id_info(id), Some(PlotInfo::Background(_))) {
                            return Command::none();
                        }
                        let pos = LAST_CURSOR_GLOBAL
                            .lock()
                            .unwrap()
                            .get(&id)
                            .copied()
                            .unwrap_or_else(|| {
                                self.last_cursor
                                    .get(&id)
                                    .copied()
                                    .unwrap_or(Point::new(0.0, 0.0))
                            });
                        let gp = self.to_global(id, pos);
                        // capture output where the menu was opened for later AddTop (Background only)
                        let output = match self.id_info(id) {
                            Some(PlotInfo::Background(o)) => Some(o),
                            _ => None,
                        };
                        // contextMenu.x = mouse.x; y = mouse.y; open()
                        self.context_menu = Some(ContextMenu {
                            x: gp.x,
                            y: gp.y,
                            open: true,
                            output,
                        });
                        println!(
                            "right click context menu at {gp:?} (local {pos:?}) output {output:?}"
                        );
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Left)) => {
                        // only start selection on Background (like QML Background MouseArea)
                        if !matches!(self.id_info(id), Some(PlotInfo::Background(_))) {
                            return Command::none();
                        }
                        // need global click position for menu hit-test before starting selection
                        let pos = LAST_CURSOR_GLOBAL
                            .lock()
                            .unwrap()
                            .get(&id)
                            .copied()
                            .unwrap_or_else(|| {
                                self.last_cursor
                                    .get(&id)
                                    .copied()
                                    .unwrap_or(Point::new(0.0, 0.0))
                            });
                        let gp = self.to_global(id, pos);

                        // QML Menu hit-test: use clamped menu rect so edge-flipped menu still captures clicks (must match view's MENU_W/H and margin 0)
                        let inside_menu = if let Some(cm) = &self.context_menu {
                            if cm.open {
                                const MENU_W: f32 = 180.0;
                                const MENU_H: f32 = 92.0;
                                // find the screen this menu is displayed on (stored output or containing)
                                let menu_screen = cm
                                    .output
                                    .and_then(|o| self.output_infos.get(&o))
                                    .or_else(|| {
                                        self.output_infos.iter().find_map(|(_, info)| {
                                            let (sx, sy, sw, sh) = {
                                                let (sx, sy) = info
                                                    .logical_position
                                                    .unwrap_or((info.location.0, info.location.1));
                                                let (sw, sh) =
                                                    info.logical_size.unwrap_or_else(|| {
                                                        info.modes
                                                            .iter()
                                                            .find(|m| m.current)
                                                            .map(|m| m.dimensions)
                                                            .unwrap_or((1920, 1080))
                                                    });
                                                (sx as f32, sy as f32, sw as f32, sh as f32)
                                            };
                                            if cm.x >= sx
                                                && cm.x < sx + sw
                                                && cm.y >= sy
                                                && cm.y < sy + sh
                                            {
                                                Some(info)
                                            } else {
                                                None
                                            }
                                        })
                                    });
                                let (menu_x, menu_y) = if let Some(info) = menu_screen {
                                    let (sx, sy, sw, sh) = {
                                        let (sx, sy) = info
                                            .logical_position
                                            .unwrap_or((info.location.0, info.location.1));
                                        let (sw, sh) = info.logical_size.unwrap_or_else(|| {
                                            info.modes
                                                .iter()
                                                .find(|m| m.current)
                                                .map(|m| m.dimensions)
                                                .unwrap_or((1920, 1080))
                                        });
                                        (sx as f32, sy as f32, sw as f32, sh as f32)
                                    };
                                    let lx = cm.x - sx;
                                    let ly = cm.y - sy;
                                    let clamped_lx = lx.clamp(0.0, (sw - MENU_W).max(0.0));
                                    let clamped_ly = ly.clamp(0.0, (sh - MENU_H).max(0.0));
                                    (sx + clamped_lx, sy + clamped_ly)
                                } else {
                                    // fallback: no output info, use raw cm position
                                    (cm.x, cm.y)
                                };
                                gp.x >= menu_x
                                    && gp.x <= menu_x + MENU_W
                                    && gp.y >= menu_y
                                    && gp.y <= menu_y + MENU_H
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if inside_menu {
                            // click was on context menu (e.g. Add Top button) — suppress selection drag and keep menu open for button release
                            return Command::none();
                        }
                        if let Some(cm) = &mut self.context_menu {
                            if cm.open {
                                cm.open = false;
                            }
                        }

                        // clear any fading rect from previous select
                        self.fade_rect = None;
                        self.fade_start = None;
                        // selecting = true; startPoint = mapToGlobal(mouse.x, mouse.y); x = startPoint.x; y = startPoint.y
                        self.selection_rect.selecting = true;
                        SELECTING.store(true, std::sync::atomic::Ordering::Relaxed);
                        self.selection_rect.start_point = Some(gp);
                        self.selection_rect.x = gp.x;
                        self.selection_rect.y = gp.y;
                        self.selection_rect.width = 0.0;
                        self.selection_rect.height = 0.0;
                        println!("select start {gp:?} (local {pos:?})");
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::ButtonPressed(Button::Middle)) => {
                        Command::none()
                    }
                    // onReleased
                    Event::Mouse(iced::mouse::Event::ButtonReleased(Button::Left)) => {
                        // if (mouse.button !== Left) return; selecting = false
                        // Keep last rect for 150ms fade (QML Behavior on opacity)
                        if self.selection_rect.selecting {
                            self.fade_rect = Some(self.selection_rect.clone());
                            self.fade_start = Some(Instant::now());
                        }
                        self.selection_rect.set_selecting(false);
                        SELECTING.store(false, std::sync::atomic::Ordering::Relaxed);
                        println!("select end -> reset rect (fade 150ms)");
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::ButtonReleased(Button::Right)) => {
                        Command::none()
                    }
                    Event::Mouse(iced::mouse::Event::ButtonReleased(_)) => Command::none(),
                    _ => Command::none(),
                }
            }
            Plant::SelectionTick => {
                // drive fade animation (150ms InOutQuad) — clear when done
                if let Some(start) = self.fade_start {
                    if start.elapsed() >= Duration::from_millis(150) {
                        self.fade_rect = None;
                        self.fade_start = None;
                    }
                }
                Command::none()
            }
            Plant::AddTop => {
                println!("Add Top clicked");
                // capture menu pos+output before closing (right-click output is most reliable)
                let menu_pos = self.context_menu.as_ref().map(|cm| Point::new(cm.x, cm.y));
                let menu_output = self.context_menu.as_ref().and_then(|cm| cm.output);
                if let Some(cm) = &mut self.context_menu {
                    cm.open = false;
                }

                // Helper: resolve output geometry with fallbacks (logical -> location+mode -> 1920x1080)
                fn output_geometry(info: &OutputInfo) -> (f32, f32, f32, f32) {
                    let (sx, sy) = info
                        .logical_position
                        .unwrap_or((info.location.0, info.location.1));
                    let (sw, sh) = info.logical_size.unwrap_or_else(|| {
                        // fallback: current mode dimensions or 1920x1080
                        info.modes
                            .iter()
                            .find(|m| m.current)
                            .map(|m| m.dimensions)
                            .map(|(w, h)| (w, h))
                            .unwrap_or((1920, 1080))
                    });
                    (sx as f32, sy as f32, sw as f32, sh as f32)
                }

                fn closest_anchor(mp: Point, info: &OutputInfo) -> Anchor {
                    let (sx, sy, sw, sh) = output_geometry(info);
                    let left_dist = mp.x - sx;
                    let right_dist = (sx + sw) - mp.x;
                    let top_dist = mp.y - sy;
                    let bottom_dist = (sy + sh) - mp.y;
                    // find minimal distance; tie-breaking Top > Bottom > Left > Right
                    let mut best = (top_dist, Anchor::Top);
                    if bottom_dist < best.0 {
                        best = (bottom_dist, Anchor::Bottom);
                    }
                    if left_dist < best.0 {
                        best = (left_dist, Anchor::Left);
                    }
                    if right_dist < best.0 {
                        best = (right_dist, Anchor::Right);
                    }
                    best.1
                }

                fn anchor_name(a: Anchor) -> &'static str {
                    if a == Anchor::Top {
                        "Top"
                    } else if a == Anchor::Bottom {
                        "Bottom"
                    } else if a == Anchor::Left {
                        "Left"
                    } else if a == Anchor::Right {
                        "Right"
                    } else {
                        "Unknown"
                    }
                }

                // Try to determine which output the context menu was on
                // Priority: stored menu_output (from right-click window) -> containing -> closest center -> first
                let (target_output, target_anchor): (Option<OutputId>, Anchor) = {
                    if let Some(output) = menu_output {
                        // menu was opened from a known background/top window, use that output directly
                        if let Some(info) = self.output_infos.get(&output) {
                            let anchor = if let Some(mp) = menu_pos {
                                closest_anchor(mp, info)
                            } else {
                                Anchor::Top
                            };
                            (Some(output), anchor)
                        } else {
                            // output not yet in output_infos (rare sentinel), fallback to stored output with Top
                            (Some(output), Anchor::Top)
                        }
                    } else if let Some(mp) = menu_pos {
                        // no stored output (e.g., daemon window), search by geometry
                        let mut found = self.output_infos.iter().find(|(_, info)| {
                            let (sx, sy, sw, sh) = output_geometry(info);
                            mp.x >= sx && mp.x < sx + sw && mp.y >= sy && mp.y < sy + sh
                        });
                        if found.is_none() && !self.output_infos.is_empty() {
                            let mut best: Option<(&OutputId, &OutputInfo, f32)> = None;
                            for (oid, info) in &self.output_infos {
                                let (sx, sy, sw, sh) = output_geometry(info);
                                let cx = sx + sw / 2.0;
                                let cy = sy + sh / 2.0;
                                let dx = mp.x - cx;
                                let dy = mp.y - cy;
                                let dist2 = dx * dx + dy * dy;
                                if best.is_none() || dist2 < best.unwrap().2 {
                                    best = Some((oid, info, dist2));
                                }
                            }
                            found = best.map(|(oid, info, _)| (oid, info));
                        }
                        if let Some((oid, info)) = found {
                            let anchor = closest_anchor(mp, info);
                            (Some(*oid), anchor)
                        } else {
                            (None, Anchor::Top)
                        }
                    } else if let Some(oid) = self.output_infos.keys().next().copied() {
                        (Some(oid), Anchor::Top)
                    } else {
                        (None, Anchor::Top)
                    }
                };

                if let Some(output_id) = target_output {
                    let anchor = target_anchor;

                    // Always spawn a new bar (allow multiple per output/anchor) — log if duplicate
                    let duplicate = self.ids.iter().any(|(wid, info)| match info {
                        PlotInfo::Top(o) if *o == output_id => {
                            self.tops.get(wid).is_some_and(|t| t.anchor() == anchor)
                        }
                        _ => false,
                    });
                    if duplicate {
                        println!(
                            "Note: {} bar already exists for output {output_id:?}, spawning another at {:?}",
                            anchor_name(anchor),
                            anchor
                        );
                    }

                    let top = Top::with_anchor(anchor);
                    let (win_id, settings) = top.open(output_id.0);
                    self.tops.insert(win_id, top);
                    self.ids.insert(win_id, PlotInfo::Top(output_id));
                    println!(
                        "Added {} bar for output {output_id:?} window {win_id:?} (closest to {:?} @ {menu_pos:?} stored_output {menu_output:?}) — calling top.open() and spawning NewLayerShell",
                        anchor_name(anchor),
                        anchor
                    );
                    // This spawns the layer shell via iced_exwlshell: id + settings -> compositor creates surface
                    return Command::done(Plant::NewLayerShell {
                        settings,
                        id: win_id,
                    });
                } else {
                    // No output info yet (startup before OutputInsert) — fallback sentinel like Grow
                    // Always allow sentinel spawn even if tops non-empty? but keep Grow-like guard to avoid spamming
                    let top = Top::new();
                    let (win_id, settings) = top.open_active();
                    let sentinel = OutputId(u32::MAX);
                    self.tops.insert(win_id, top);
                    self.ids.insert(win_id, PlotInfo::Top(sentinel));
                    println!(
                        "Added sentinel Top window {win_id:?} (no output yet) — calling top.open_active()"
                    );
                    return Command::done(Plant::NewLayerShell {
                        settings,
                        id: win_id,
                    });
                }
            }
            _ => Command::none(),
        }
    }
}

pub fn redraw_scope(message: &Plant) -> Scope {
    match message {
        // SelectionTick is throttled drag update — must redraw all outputs
        Plant::SelectionTick => Scope::All,
        // Button press/release changes selecting/context_menu → All
        Plant::Graft(_, Event::Mouse(iced::mouse::Event::ButtonPressed(_)))
        | Plant::Graft(_, Event::Mouse(iced::mouse::Event::ButtonReleased(_))) => Scope::All,
        // CursorMoved is handled via throttled SelectionTick; no direct redraw to avoid flood
        Plant::Graft(_, Event::Mouse(iced::mouse::Event::CursorMoved { .. })) => Scope::None,
        Plant::Graft(_, Event::Mouse(_)) => Scope::None,
        Plant::Graft(_, _) => Scope::None,
        Plant::Wayland(_) => Scope::All,
        _ => Scope::All,
    }
}
