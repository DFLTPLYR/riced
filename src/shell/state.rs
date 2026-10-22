use iced::futures::Stream;
use iced::widget::Space;
use iced::{Element, Event, Point, Task as Command};
use iced_exwlshell::redraw::Scope;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use iced_wayland_subscriber::OutputId;
use iced_wayland_subscriber::shell::{ShellEvent, ShellReceiver};

use super::screens::{Background, Notification, Popup, Setting, Top};
use super::windows::PlotInfo;
use super::{
    BackgroundEvent, BarEvent, ConfigEvent, Corner, Edge, LandEvent, NotifyEvent, Plant,
    SettingEvent, StyleEvent, TopEvent, WidgetEvent,
};
use crate::config::{Config, ConfigPatch};

static LAST_MOUSE_MOVE: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));

fn throttled_graft(
    event: Event,
    _status: iced::event::Status,
    id: iced::window::Id,
) -> Option<Plant> {
    if let Event::Mouse(iced::mouse::Event::CursorMoved { position }) = &event {
        if let Ok(mut cursors) = super::screens::background::LAST_CURSOR_GLOBAL.lock() {
            cursors.insert(id, *position);
        }
        // always update global last cursor (throttled to 60fps) so press is accurate
        let now = Instant::now();
        let mut last = LAST_MOUSE_MOVE.lock().unwrap();
        if now.duration_since(*last) < Duration::from_millis(16) {
            return None;
        }
        *last = now;
        return Some(Plant::Graft(id, event));
    }
    if matches!(
        event,
        Event::Mouse(
            iced::mouse::Event::ButtonPressed(_)
                | iced::mouse::Event::ButtonReleased(_)
                | iced::mouse::Event::CursorEntered
                | iced::mouse::Event::CursorLeft
        )
    ) {
        return Some(Plant::Graft(id, event));
    }
    if matches!(
        event,
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        })
    ) {
        return Some(Plant::Graft(id, event));
    }
    None
}

#[derive(Debug)]
pub struct Plots {
    pub(crate) windows: super::windows::WindowState,
    pub(crate) shell_events: ShellReceiver,
    // per-window cursor
    pub(crate) input: super::input::InputState,
    pub(crate) desktop: super::desktop::DesktopState,
    // press target per bar window for click matching: (slot, widget
    // or gap-None, press time). Presses on a widget area record the
    // widget; gap presses record None; releases act only on a matching
    // target, so bubbled outer releases never double-fire widget clicks.
    // `usize::MAX` slot marks a press whose slot couldn't be resolved.
    // hot-reloaded config + last seen file mtime
    pub(crate) config: Config,
    pub(crate) config_mtime: Option<std::time::SystemTime>,
    pub(crate) catalog: super::catalog::WidgetCatalog,
    // Lua widget runtimes keyed by placement id (independent `self`
    // per instance), last rendered text, last run tick, and last error
    // (errors log only on change, never per tick). States are rebuilt
    // on definition rescan.
    // States are shared across bars; rendered trees/outputs are per
    // bar (keyed `(bar, placement)`) so `bar.output` can differ per bar.
    pub(crate) placements: crate::lua::widgets::WidgetState,
    // Script file mtimes by resolved path (live-reload on edit).
    // Animated lists (see layers::listview): one aura runtime shared
    // by all surfaces, one ListView per (bar, widget, list) — owners
    // are `"{bar:?}/{widget}/{list}"` strings — plus the notification
    // stack. Widget cell rows keyed (owner, key), notifications keyed
    // (output, id). Per-list runtimes let each list own its Lua
    // `transitions()` spec without bars animating each other.
    pub(crate) animation: super::animation::AnimationState,
    // Live system snapshot for the `system` Lua table (CPU + memory,
    // refreshed on every widget tick; usage needs the delta).
    pub(crate) services: crate::services::state::ServiceState,
    // Native toplevel listener backing `wayland.toplevels` (its own
    // thread blocks on the compositor socket; ticks just snapshot).
    // Native workspace listener backing `wayland.workspaces` (same
    // shape: own thread, tick snapshots).
    // Local-first staging: `Patch` mutates live memory every tick (smooth
    // previews, no disk I/O); the file write is coalesced via `SaveTimer`.
    // `dirty` marks unsaved staged edits, `seq` invalidates superseded timers.
    pub(crate) config_jobs: super::jobs::SaveState,
    // mtime of the active theme file (`theme::poll`); `None` tracks the
    // vendored fallback. A change re-emits the config so every view repaints.
    pub(crate) theme_mtime: Option<std::time::SystemTime>,
    // Dynamic-theme regen: discrete touches (drops, add/remove/scale,
    // file edits) arm a 2s one-shot timer; panel close fires immediately.
    // `seq` invalidates superseded timers, `running` guards overlap.
    pub(crate) theme_jobs: super::jobs::ThemeJobState,
    // Notification layer (D-Bus server + internal events): queued
    // plain-data notifications, one layer window per showing output,
    // last-known global cursor for mouse-output placement.
    pub(crate) notification: super::notifications::NotificationState,
    // Last mirrored TOML parse failure (`source: message`); a repeat of
    // the same message notifies only once.
}

impl Plots {
    /// Detach surface resources; native close commands and saved-bar changes
    /// remain host operations so compositor removal does not delete config.
    pub(crate) fn forget_window(&mut self, id: iced::window::Id) -> Option<PlotInfo> {
        super::desktop_widgets::forget_settings(self, id);
        if let Some(PlotInfo::Overlay(output)) = self.windows.ids.get(&id).copied()
            && self.windows.overlays.get(&output) == Some(&id)
        {
            self.desktop
                .overlays
                .forget(output, &mut self.animation.motion);
        }
        self.input.cursors.remove(&id);
        if let Ok(mut cursors) = super::screens::background::LAST_CURSOR_GLOBAL.lock() {
            cursors.remove(&id);
        }
        self.input.presses.remove(&id);
        self.placements.forget_window(id);
        self.animation.forget_window(id);
        self.windows.forget(id)
    }

    pub fn new(shell_events: ShellReceiver) -> Self {
        let (config, config_mtime) = Config::load();
        crate::icons::set_theme(&config.theme.icon_theme);
        crate::theme::ensure_user_themes();
        crate::theme_gen::ensure_user_templates();
        let theme_mtime = crate::theme::poll(&config.theme, &None).unwrap_or(None);
        crate::theme::sync(&config.theme);
        let desktop = super::desktop::DesktopState::default();
        let widgets = Top::discover_widget_defs();
        let mut plots = Self {
            windows: Default::default(),
            shell_events,
            input: Default::default(),
            desktop,
            config,
            config_mtime,
            catalog: super::catalog::WidgetCatalog::new(widgets),
            config_jobs: Default::default(),
            theme_mtime,
            theme_jobs: Default::default(),
            notification: Default::default(),
            placements: Default::default(),
            animation: Default::default(),
            services: crate::services::state::ServiceState::new(),
        };
        // Render Lua widgets once so bars populate on the first frame
        // instead of waiting out the first tick. Retired widgets.toml
        // values fold into placements first (needs discovered Lua
        // defaults), so migrated bars render correctly immediately.
        Top::migrate_widgets_toml(&mut plots);
        Top::init_widget_lua(&mut plots);
        plots
    }

    /// sys `change_theme` equivalent as an iced command: render the
    /// configured templates dir with the newly selected theme object on a
    /// blocking worker (template hooks shell out and must not stall the
    /// update loop or freeze every output).
    pub(crate) fn retemplate_command(dir: std::path::PathBuf, config: &Config) -> Command<Plant> {
        let name = config.theme.name.clone();
        let darkmode = config.theme.darkmode;
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let text = match crate::theme_gen::stored_theme_text(&name) {
                        Ok(t) => t,
                        Err(e) => return vec![e],
                    };
                    crate::theme_gen::render_theme_templates(&dir, &text, darkmode)
                })
                .await
                .unwrap_or_else(|e| vec![format!("template task failed: {e}")])
            },
            |errors| Plant::Config(ConfigEvent::TemplatesDone(errors)),
        )
    }

    /// Quiet delay between a wallpaper touch and its regen: long enough
    /// that a drop-then-tweak lands in one run, short enough to feel live.
    const REGEN_DELAY: Duration = Duration::from_secs(2);

    /// Idle delay before staged config edits hit the disk: long enough that
    /// a slider drag coalesces into one write, short enough that a pause
    /// persists without waiting for panel close.
    pub(crate) const CONFIG_SAVE_DELAY: Duration = Duration::from_millis(800);

    /// Arm a coalesced config-file write for staged edits (bar panel
    /// drags): memory updates every tick, the file once idle. Panel close
    /// flushes synchronously via `flush_config_save`.
    pub(crate) fn arm_config_save(&mut self) -> Command<Plant> {
        let save_seq = self.config_jobs.stage();
        Command::perform(tokio::time::sleep(Self::CONFIG_SAVE_DELAY), move |_| {
            Plant::Config(ConfigEvent::SaveTimer(save_seq))
        })
    }

    /// Persist staged config edits, if any. Idempotent no-op when clean.
    /// Called by the coalescing save timer and synchronously on panel close.
    pub(crate) fn flush_config_save(&mut self) {
        if !self.config_jobs.dirty {
            return;
        }
        self.config_jobs.invalidate();
        // Update mtime so the poll tick doesn't echo our own write back.
        self.config_mtime = self.config.save().or(self.config_mtime);
    }

    /// Schedule a follow-up for an explicit Apply accepted during generation.
    /// Supersedes pending timers via `seq`; no-ops while a run is in flight.
    pub(crate) fn arm_regen_theme(&mut self) -> Command<Plant> {
        self.theme_jobs.dirty = true;
        if self.theme_jobs.running {
            return Command::none();
        }
        if !crate::theme_gen::wants_regen(&self.config.theme.name) {
            self.theme_jobs.dirty = false;
            return Command::none();
        }
        self.theme_jobs.sequence += 1;
        let seq = self.theme_jobs.sequence;
        Command::perform(tokio::time::sleep(Self::REGEN_DELAY), move |_| {
            Plant::Config(ConfigEvent::RegenTimer(seq))
        })
    }

    /// Consume a generation request from a successful explicit Apply.
    /// Batch-safe: `none` unless dirty, idle, and wanted.
    pub(crate) fn fire_regen_theme(&mut self) -> Command<Plant> {
        if !self.theme_jobs.dirty || self.theme_jobs.running {
            return Command::none();
        }
        if !crate::theme_gen::wants_regen(&self.config.theme.name) {
            self.theme_jobs.dirty = false;
            return Command::none();
        }
        self.spawn_regen()
    }

    /// Snapshot live outputs + images and run the generation on a blocking
    /// worker. Caller gates dirty/running/policy; this marks running and
    /// invalidates pending timers.
    fn spawn_regen(&mut self) -> Command<Plant> {
        // Live output rects (same avail math wallpaper_views paints),
        // sorted for a deterministic seed; empty headless → per-image
        // rects, like the CLI.
        let mut rects: Vec<(f32, f32, f32, f32)> = self
            .windows
            .backgrounds
            .keys()
            .filter_map(|o| Background::available_rect(*o, &self.windows.output_infos))
            .collect();
        rects.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        if rects.is_empty() {
            rects = self
                .config
                .background
                .image
                .iter()
                .filter_map(crate::ui::widgets::display_map::MapLayer::resolved)
                .collect();
        }
        let images = self.config.background.image.clone();
        let mut applied: Vec<_> = self
            .desktop
            .wallpaper
            .applied
            .values()
            .filter_map(|tile| match &tile.handle {
                iced::widget::image::Handle::Rgba {
                    width,
                    height,
                    pixels,
                    ..
                } => Some((
                    tile.spec.key.clone(),
                    crate::theme_gen::views::PixelView {
                        width: *width,
                        height: *height,
                        pixels: pixels.clone(),
                    },
                )),
                _ => None,
            })
            .collect();
        applied.sort_by(|a, b| a.0.cmp(&b.0));
        let variant = self.config.theme.variant.clone();
        let darkmode = self.config.theme.darkmode;
        let templates = crate::theme_gen::effective_templates_dir(&self.config.theme);

        self.theme_jobs.dirty = false;
        self.theme_jobs.running = true;
        self.theme_jobs.sequence += 1;
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::theme_gen::worker::run(move || {
                        let generated = if applied.is_empty() {
                            let views = crate::theme_gen::render_views(&rects, &images);
                            crate::theme_gen::generate_from_views(&views, &variant, darkmode)
                        } else {
                            let views: Vec<_> =
                                applied.into_iter().map(|(_, pixels)| pixels).collect();
                            crate::theme_gen::generate_from_pixels(&views, &variant, darkmode)
                        };
                        match generated {
                            Ok(g) => {
                                let mut errors = Vec::new();
                                if let Err(e) = crate::theme_gen::write_dynamic_theme(&g.payload) {
                                    errors.push(format!("cannot write dynamic.json: {e}"));
                                } else if let Some(dir) = templates {
                                    errors.extend(crate::theme_gen::process_templates(
                                        &dir,
                                        &g.variables,
                                    ));
                                }
                                errors
                            }
                            Err(e) => vec![e],
                        }
                    })
                    .unwrap_or_else(|error| vec![error])
                })
                .await
                .unwrap_or_else(|e| vec![format!("theme regen failed: {e}")])
            },
            |errors| Plant::Config(ConfigEvent::ThemeRegenerated(errors)),
        )
    }

    /// Delayed full redraws after a Background surface is created (see
    /// `BackgroundEvent::Repaint`). The new surface doesn't exist yet when
    /// its own creation message's redraw is processed, so schedule
    /// follow-ups covering late configures and pending uploads.
    pub(crate) fn repaint_after(millis: u64) -> Command<Plant> {
        Command::perform(tokio::time::sleep(Duration::from_millis(millis)), |()| {
            Plant::BackgroundPlot(BackgroundEvent::Repaint)
        })
    }

    /// Burst of delayed heals covering late configures, slow GPU init, and
    /// pending image uploads on slow startups. Fixed timers are inherently
    /// racy, so this is (re)scheduled at every signal that the surface or
    /// its geometry may have (re)appeared: `OutputAdded`, geometry-changing
    /// `OutputUpdated`, and `NewShell` for a Background window.
    pub(crate) fn repaint_burst() -> Command<Plant> {
        Command::batch(vec![
            Self::repaint_after(80),
            Self::repaint_after(500),
            Self::repaint_after(1500),
        ])
    }

    pub fn id_info(&self, id: iced::window::Id) -> Option<PlotInfo> {
        self.windows.ids.get(&id).copied()
    }

    pub fn namespace() -> String {
        String::from("Riced Main Runtime")
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

        let mut subs = vec![
            iced::event::listen_with(throttled_graft)
                .with(super::desktop_widgets::held_source(self))
                .map(|(source, message)| match message {
                    Plant::Graft(
                        id,
                        Event::Mouse(iced::mouse::Event::CursorMoved { position }),
                    ) if Some(id) == source => {
                        Plant::Desktop(super::desktop_widgets::DesktopEvent::SourceMotion {
                            source: id,
                            position,
                        })
                    }
                    message => message,
                }),
            shell_sub,
            iced::Subscription::run_with(
                self.services.wayland_changes.clone(),
                Self::wayland_stream,
            ),
            // XDG base windows (Settings) report close so `ids` stays in sync.
            // This is native Closed, not CloseRequested. The backend has
            // already detached the renderer; never queue another native close.
            iced::window::close_events().map(|id| Plant::Wayland(LandEvent::Closed(id))),
            // hot-reload poll for config.lua (mtime check only, cheap)
            iced::time::every(Duration::from_millis(500))
                .map(|_| Plant::Config(ConfigEvent::ConfigTick)),
            // file IPC poll for CLI requests (`riced open-settings`)
            iced::time::every(Duration::from_millis(250)).map(|_| Plant::IpcPoll),
        ];

        if !self.config.desktop.is_empty() || self.desktop.drag.is_some() {
            subs.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Plant::Desktop(super::desktop_widgets::DesktopEvent::Tick)),
            );
        }

        // List enter/exit transitions tick at 60fps only while a
        // motion is active (aura runtime idles to zero wakeups).
        if self.animation.motion.has_active()
            || self
                .animation
                .widgets
                .values()
                .any(crate::ui::listview::ListView::has_callbacks)
            || self.desktop.overlays.has_callbacks()
        {
            subs.push(
                iced::time::every(Duration::from_millis(16))
                    .map(|_| Plant::TopPlot(TopEvent::Widget(WidgetEvent::Anim))),
            );
        }

        // Hidden history must not keep the popup deadline timer alive.
        if self.notification.has_deadlines() {
            subs.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Plant::Notify(NotifyEvent::Tick)),
            );
        }
        if (self.config.notifications.enabled && !self.config.notifications.source.is_empty())
            || !self.config.overlays.is_empty()
        {
            subs.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Plant::Overlay(super::overlay::OverlayEvent::Tick)),
            );
        }

        subs.push(iced::Subscription::run_with(
            self.services.tray.clone(),
            Self::tray_stream,
        ));

        // D-Bus notification server: the configured default timeout
        // rides the subscription identity, so editing it restarts the
        // server (re-requests the bus name).
        if self.config.notifications.enabled {
            subs.push(iced::Subscription::run_with(
                self.config.notifications.timeout_ms,
                Self::notif_stream,
            ));
        }

        // Only tick for fade animation (selecting is driven by throttled mouse moves, not timer)
        // QML Behavior InOutQuad on opacity (over the animation speed) needs
        // 60fps ticks only while fading
        if self.desktop.fade_start.is_some() {
            subs.push(
                iced::time::every(Duration::from_millis(16))
                    .map(|_| Plant::BackgroundPlot(BackgroundEvent::SelectionTick)),
            );
        }

        // Lua widgets re-render on their own intervals (250ms cadence,
        // each script runs only when due). No timer at all without them.
        if !self.catalog.definitions.is_empty() {
            subs.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Plant::TopPlot(TopEvent::Widget(WidgetEvent::Tick))),
            );
        }

        iced::Subscription::batch(subs)
    }

    fn wayland_stream(
        changes: &crate::services::changes::Changes,
    ) -> impl iced::futures::Stream<Item = Plant> + use<> {
        iced::futures::stream::unfold(changes.clone(), |changes| async move {
            changes.next().await;
            Some((Plant::WaylandChanged, changes))
        })
    }

    /// D-Bus server stream builder (bare fn pointer for
    /// `Subscription::run_with` identity; boxed so no input lifetime
    /// leaks into the higher-ranked signature).
    fn notif_stream(default_ms: &u64) -> std::pin::Pin<Box<dyn Stream<Item = Plant> + Send>> {
        let ms = *default_ms;
        Box::pin(iced::stream::channel(16, async move |tx| {
            crate::notify::serve(tx, ms).await;
        }))
    }

    fn tray_stream(
        cache: &crate::tray::TrayCache,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Plant> + Send>> {
        let cache = cache.clone();
        Box::pin(iced::stream::channel(32, async move |tx| {
            crate::tray::serve(cache, tx).await;
        }))
    }

    pub fn title(&self, id: iced::window::Id) -> Option<String> {
        match self.id_info(id) {
            Some(PlotInfo::Setting) => Some(Setting::title()),
            _ => None,
        }
    }

    pub fn view(&self, id: iced::window::Id) -> Element<'_, Plant> {
        crate::ui::animations::reduced_motion(self.config.animation.reduced_motion);
        // Background windows render their own selection/context overlays clipped
        // Top windows render the bar. Daemon's tiny 1x1 window: empty.
        // Settings (XDG toplevel) renders its own panel (see layers/setting.rs).
        if let Some(setting) = self.windows.settings.get(&id) {
            return setting.view(id, self);
        }
        match self.id_info(id) {
            Some(PlotInfo::Drag(output)) => super::desktop_widgets::view(self, id, output),
            Some(PlotInfo::Background(output)) => Background::view(self, id, output),
            Some(PlotInfo::Top(_output)) => self
                .windows
                .tops
                .get(&id)
                .map(|t| {
                    t.view(
                        id,
                        &self.catalog.definitions,
                        &self.placements,
                        &self.animation.motion,
                        &self.animation.widgets,
                        self.config.interaction.hold.duration(),
                    )
                })
                .unwrap_or_else(|| Space::new().into()),
            Some(PlotInfo::Popup(_output)) => self
                .windows
                .popups
                .get(&id)
                .map(|p| p.view(self))
                .unwrap_or_else(|| Space::new().into()),
            Some(PlotInfo::Overlay(output)) => super::overlay::view(self, output),
            Some(PlotInfo::Setting) => Space::new().into(), // unreachable: handled above
            None => Space::new().into(),                    // daemon's 1x1 tiny window
        }
    }

    /// Mirror a fresh TOML parse failure as a critical notification
    /// (once per distinct message). No-op when nothing new failed.
    fn notify_parse_error(&mut self) -> Command<Plant> {
        let Some((source, msg)) = crate::config::take_parse_error() else {
            return Command::none();
        };
        let key = format!("{source}: {msg}");
        if self.config_jobs.last_error.as_deref() == Some(&key) {
            return Command::none();
        }
        self.config_jobs.last_error = Some(key);
        let n = Notification::internal(
            &self.config.notifications,
            &format!("{source} parse error"),
            msg,
            2,
        );
        Notification::handle_arrived(self, n)
    }

    fn close_surface(
        &mut self,
        id: iced::window::Id,
        request_native_close: bool,
    ) -> Command<Plant> {
        let info = self.windows.ids.get(&id).copied();
        // Compute before any detach drops the owning state. Only windows that
        // owned decoded images need the full-redraw reclaim; the rest stay
        // host-only (their renderers die with the window anyway).
        let owned_images = match info {
            Some(PlotInfo::Setting) => {
                super::desktop_widgets::settings_have_images(self, id)
                    || self.windows.settings.get(&id).is_some_and(|setting| {
                        setting
                            .wallpaper_draft
                            .as_ref()
                            .is_some_and(|draft| !draft.sources.is_empty())
                    })
            }
            Some(PlotInfo::Popup(..)) => self.windows.popups.get(&id).is_some_and(|popup| {
                popup
                    .tree
                    .as_ref()
                    .is_some_and(crate::ui::node::contains_images)
            }),
            Some(PlotInfo::Overlay(output)) => {
                self.notification.holds_images(output) || self.desktop.overlays.has_images(output)
            }
            _ => false,
        };
        let mut commands = Vec::new();
        if matches!(info, Some(PlotInfo::Top(_))) {
            let children: Vec<_> = self
                .windows
                .popups
                .iter()
                .filter_map(|(popup_id, popup)| (popup.bar_id == id).then_some(*popup_id))
                .collect();
            for child in children {
                if request_native_close {
                    commands.push(Popup::handle_dismiss(self, child));
                } else {
                    self.forget_window(child);
                }
            }
        }
        if let Some(PlotInfo::Overlay(output)) = info
            && self.windows.overlays.get(&output) == Some(&id)
        {
            self.desktop
                .overlays
                .forget(output, &mut self.animation.motion);
        }
        self.forget_window(id);
        let closed_last_panel =
            matches!(info, Some(PlotInfo::Setting)) && self.windows.settings.is_empty();
        self.flush_config_save();
        // Closing an already-removed native surface causes this backend to
        // queue an action for an ID which will never be registered again.
        if request_native_close && info.is_some() {
            commands.push(iced_runtime::task::effect(Action::Window(
                WindowAction::Close(id),
            )));
        }
        if closed_last_panel && self.theme_jobs.dirty {
            commands.push(self.fire_regen_theme());
        }
        // Explicit closes must reclaim too, even if native confirmation is
        // delayed or missed. The deferred task lets old view/renderer owners drop.
        commands.push(self.reclaim_image_memory(owned_images));
        Command::batch(commands)
    }

    fn refresh_theme(&mut self) -> Command<Plant> {
        crate::theme::sync(&self.config.theme);
        self.theme_mtime = crate::theme::poll(&self.config.theme, &None).unwrap_or(None);
        Top::refresh_services(self);
        Command::batch([
            super::overlay::refresh(self, true),
            Popup::refresh_bodies(self),
            self.reclaim_image_memory(false),
        ])
    }

    pub fn update(&mut self, message: Plant) -> Command<Plant> {
        // The Wayland redraw path does not rebuild Outdated widget trees.
        // Internal frame actions wake the normal UI rebuild path without
        // invoking handlers, refreshing Lua views, or mutating host state.
        let frame_action = match &message {
            Plant::TopPlot(TopEvent::Widget(
                WidgetEvent::CellAction(_, _, action) | WidgetEvent::PopupSelect(_, action),
            )) => Some(action),
            Plant::Desktop(super::desktop_widgets::DesktopEvent::Action { action, .. }) => {
                Some(action)
            }
            Plant::Overlay(super::overlay::OverlayEvent::Action { action, .. }) => Some(action),
            _ => None,
        };
        if frame_action.is_some_and(crate::ui::action::Action::is_animation_frame) {
            return Command::none();
        }
        let cancel_drag = self
            .desktop
            .drag
            .as_ref()
            .is_some_and(|drag| match &message {
                Plant::Uproot(id) | Plant::Wayland(LandEvent::Closed(id)) => {
                    *id == drag.source || self.windows.drags.values().any(|surface| surface == id)
                }
                Plant::Sprout
                | Plant::Wayland(
                    LandEvent::OutputAdded(_)
                    | LandEvent::OutputUpdated(_)
                    | LandEvent::OutputRemoved(_),
                )
                | Plant::Config(ConfigEvent::ConfigReloaded(_)) => true,
                _ => false,
            });
        if cancel_drag {
            if let Plant::Wayland(LandEvent::Closed(id)) = &message {
                self.windows.drags.retain(|_, surface| surface != id);
            }
            let cleanup = super::desktop_widgets::finish(self);
            return Command::batch([cleanup, self.update(message)]);
        }
        if matches!(
            &message,
            Plant::Wayland(
                LandEvent::OutputAdded(_)
                    | LandEvent::OutputUpdated(_)
                    | LandEvent::OutputRemoved(_)
            )
        ) {
            self.services.wayland_changes.notify();
        }
        match message {
            Plant::Desktop(event) => super::desktop_widgets::update(self, event),
            Plant::Overlay(event) => super::overlay::update(self, event),
            Plant::Uproot(id) => self.close_surface(id, true),
            Plant::Tend => Command::none(),
            Plant::Sprout => {
                // Toggle: spawn when none open, else close the current panel.
                Setting::handle_toggle(self)
            }
            Plant::IpcPoll => {
                // CLI client queued a request (`riced open-settings`).
                match crate::cli::take_queued_command() {
                    Some(crate::cli::QueuedCommand::OpenSettings) => {
                        let cleanup = super::desktop_widgets::finish(self);
                        Command::batch([cleanup, Setting::handle_toggle(self)])
                    }
                    None => Command::none(),
                }
            }
            Plant::Wayland(LandEvent::OutputAdded(output)) => {
                let output_id = OutputId::from(&output);
                // store geometry for panel.screen clipping
                self.windows.output_infos.insert(output_id, output.clone());
                let mut cmds = Vec::new();
                cmds.push(self.refresh_wallpapers());
                // Warm passive notification surfaces before any arrival. An
                // arrival then updates an existing renderer/layer instead of
                // mapping a new surface over the focused fullscreen app.
                cmds.push(super::overlay::refresh(self, true));
                // sentinel Top cleanup (delegated to Top layer)
                for sentinel_id in self.windows.sentinel_bars() {
                    self.forget_window(sentinel_id);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(sentinel_id),
                    )));
                }
                // Background fullscreen per output for selection (delegated to Background layer)
                if let Some(cmd) = Background::ensure_for_output(
                    &mut self.windows.backgrounds,
                    &mut self.windows.background_ids,
                    &mut self.windows.ids,
                    output_id,
                ) {
                    cmds.push(cmd);
                    cmds.push(Self::repaint_burst());
                }
                // Declarative bars: one per `[[bar]]` entry on this
                // output, skipped when that edge already has a bar (user
                // additions and re-added outputs never duplicate).
                // File order is spawn order (deterministic).
                let bars = self.config.bar.clone();
                for (index, cfg) in bars.iter().enumerate() {
                    let Some(anchor) = Top::parse_anchor(&cfg.anchor) else {
                        eprintln!(
                            "riced: [[bar]] #{index}: unknown anchor {:?}, skipping",
                            cfg.anchor
                        );
                        continue;
                    };
                    // Named output only (`""` = every output): compare
                    // against the connector name (`DP-1`, …).
                    if !cfg.output.trim().is_empty() {
                        let here = self
                            .windows
                            .output_infos
                            .get(&output_id)
                            .and_then(|info| info.name.clone())
                            .unwrap_or_default();
                        if !here.eq_ignore_ascii_case(cfg.output.trim()) {
                            continue;
                        }
                    }
                    let taken = self.windows.ids.iter().any(|(wid, info)| match info {
                        PlotInfo::Top(o) if *o == output_id => self
                            .windows
                            .tops
                            .get(wid)
                            .is_some_and(|t| t.anchor() == anchor),
                        _ => false,
                    });
                    if taken {
                        continue;
                    }
                    let top = Top::with_config(index, anchor, cfg.into());
                    let (sw, sh) = self
                        .windows
                        .output_infos
                        .get(&output_id)
                        .map(Background::output_geometry)
                        .map(|(_, _, w, h)| (w, h))
                        .unwrap_or((1920.0, 1080.0));
                    let (w, h) = top.local.px_size(sw, sh, top.is_horizontal());
                    let (win_id, settings) = top.open(output_id.0, w, h);
                    self.windows.tops.insert(win_id, top);
                    self.windows.ids.insert(win_id, PlotInfo::Top(output_id));
                    Top::render_bar_widgets(self, win_id);
                    cmds.push(Command::done(Plant::NewLayerShell {
                        settings,
                        id: win_id,
                    }));
                }
                if cmds.is_empty() {
                    Command::none()
                } else {
                    Command::batch(cmds)
                }
            }
            Plant::Wayland(LandEvent::OutputUpdated(output)) => {
                let output_id = OutputId::from(&output);
                // Real geometry (logical position/size) often arrives here,
                // after the Added-time burst already fired against placeholder
                // values and computed the wrong overlap rects — heal again.
                let geom_changed = self
                    .windows
                    .output_infos
                    .get(&output_id)
                    .map(Background::output_geometry)
                    .unwrap_or((0.0, 0.0, 0.0, 0.0))
                    != Background::output_geometry(&output);
                self.windows.output_infos.insert(output_id, output);
                let wallpaper = self.refresh_wallpapers();
                if geom_changed {
                    // `%` bar sizes resolve against this geometry: re-push
                    // every bar's px size so nothing goes stale, plus heals.
                    // Notification windows span the output height, so they
                    // re-push too.
                    Command::batch(vec![
                        wallpaper,
                        Self::repaint_burst(),
                        Top::reapply_for_output(self, output_id),
                        super::overlay::refresh(self, true),
                    ])
                } else {
                    wallpaper
                }
            }
            Plant::Wayland(LandEvent::OutputRemoved(output)) => {
                let output_id = OutputId::from(&output);
                let surfaces = self.windows.on_output(output_id);
                // Retire notifications while their window binding still exists.
                self.notification.forget_output(output_id);
                self.desktop
                    .overlays
                    .forget(output_id, &mut self.animation.motion);
                let mut cmds = Vec::new();
                for id in surfaces {
                    if self.forget_window(id).is_some() {
                        cmds.push(iced_runtime::task::effect(Action::Window(
                            WindowAction::Close(id),
                        )));
                    }
                }
                self.windows.output_infos.remove(&output_id);
                super::desktop_widgets::retain(self);
                self.desktop.wallpaper.applied.remove(&output_id);
                cmds.push(self.refresh_wallpapers());
                // clear global selection if it was on removed output (will hide via intersect check)
                if cmds.is_empty() {
                    Command::none()
                } else {
                    Command::batch(cmds)
                }
            }
            Plant::Wayland(LandEvent::NewShell(info)) => match self.id_info(info.window) {
                Some(PlotInfo::Background(_)) => Self::repaint_burst(),
                Some(PlotInfo::Overlay(output)) => {
                    // Fresh native surface: force the mask reinstall and
                    // assert the passive keyboard policy exactly once. Later
                    // reconciles only push when the card rects actually move.
                    self.desktop.overlays.masks.remove(&output);
                    Command::batch(vec![
                        Command::done(Plant::KeyboardInteractivityChange {
                            id: info.window,
                            keyboard_interactivity:
                                iced_exwlshell::reexport::KeyboardInteractivity::None,
                        }),
                        super::overlay::push_mask(self, output),
                    ])
                }
                _ => Command::none(),
            },
            Plant::Wayland(LandEvent::Closed(id)) => {
                // Backend confirmation: its renderer and UI cache are already
                // gone. Also forget app state if the close subscription was
                // missed, then reclaim only after those references are dropped.
                self.close_surface(id, false)
            }
            Plant::Wayland(LandEvent::WindowOutputChanged { .. }) => Command::none(),
            Plant::Wayland(LandEvent::Locked) => Command::none(),
            Plant::Wayland(LandEvent::LockDenied) => Command::none(),
            Plant::Wayland(LandEvent::LockedFinished) => Command::none(),
            Plant::Graft(id, event) => {
                if self.desktop.drag.is_some()
                    && matches!(
                        &event,
                        Event::Keyboard(iced::keyboard::Event::KeyPressed {
                            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                            ..
                        }) | Event::Mouse(iced::mouse::Event::ButtonPressed(
                            iced::mouse::Button::Right
                        ))
                    )
                {
                    return super::desktop_widgets::finish(self);
                }
                if let Event::Mouse(iced::mouse::Event::CursorMoved { position }) = &event
                    && super::desktop_widgets::held_source(self) == Some(id)
                {
                    return super::desktop_widgets::update(
                        self,
                        super::desktop_widgets::DesktopEvent::SourceMotion {
                            source: id,
                            position: *position,
                        },
                    );
                }
                if matches!(
                    event,
                    Event::Mouse(iced::mouse::Event::ButtonReleased(
                        iced::mouse::Button::Left
                    ))
                ) {
                    if let Some(task) =
                        super::desktop_widgets::request_release(self, id, iced::mouse::Button::Left)
                    {
                        self.input.presses.remove(&id);
                        return task;
                    }
                    let _ = Background::handle_left_release(self);
                    self.input.presses.remove(&id);
                    return Command::none();
                }
                match self.id_info(id) {
                    Some(PlotInfo::Top(_)) => Top::handle_graft(self, id, &event),
                    _ => Background::handle_graft(self, id, &event),
                }
            }
            Plant::BackgroundPlot(BackgroundEvent::SelectionTick) => {
                // drive fade animation (speed-scaled InOutQuad) — clear when done
                if let Some(start) = self.desktop.fade_start
                    && start.elapsed() >= self.config.animation.speed.duration()
                {
                    self.desktop.fade_rect = None;
                    self.desktop.fade_start = None;
                }
                Command::none()
            }
            Plant::BackgroundPlot(BackgroundEvent::Repaint) => Background::repaint(self),
            Plant::BackgroundPlot(BackgroundEvent::ContextMenuAction(output, action)) => {
                Background::handle_context_menu_action(self, output, &action)
            }
            Plant::BackgroundPlot(BackgroundEvent::PickWallpaper) => {
                Background::handle_pick_wallpaper()
            }
            Plant::BackgroundPlot(BackgroundEvent::WallpaperPicked(picked)) => {
                Background::handle_wallpaper_picked(self, picked)
            }
            Plant::BackgroundPlot(BackgroundEvent::Pressed(id, button)) => {
                Background::handle_panel_button(self, id, button, true)
            }
            Plant::BackgroundPlot(BackgroundEvent::Released(id, button)) => {
                Background::handle_panel_button(self, id, button, false)
            }
            Plant::BackgroundPlot(BackgroundEvent::Hover(id, instance, hovered)) => {
                Background::handle_hover(self, id, instance, hovered)
            }
            Plant::Config(ConfigEvent::ConfigTick) => {
                // mtime check only; the reload itself redraws via ConfigReloaded
                if let Some((cfg, mtime)) = Config::poll(&self.config_mtime) {
                    self.config_mtime = mtime;
                    self.theme_mtime = crate::theme::poll(&cfg.theme, &None).unwrap_or(None);
                    return Command::batch(vec![
                        Command::done(Plant::Config(ConfigEvent::ConfigReloaded(cfg))),
                        self.notify_parse_error(),
                    ]);
                }
                // Palette updates are not config-file edits and must not
                // invalidate pending saves or rebuild every app VM.
                if let Some(mtime) = crate::theme::poll(&self.config.theme, &self.theme_mtime) {
                    self.theme_mtime = mtime;
                    return Command::done(Plant::Config(ConfigEvent::ThemeReloaded));
                }
                // Widget definitions hot-rescan on widgets-dir change:
                // fresh defs repaint every bar on the next frame.
                if self.catalog.directory_changed() {
                    return Command::batch(vec![
                        Command::done(Plant::Config(ConfigEvent::WidgetsReloaded(
                            Top::discover_widget_defs(),
                        ))),
                        self.notify_parse_error(),
                    ]);
                }
                // Retired widgets.toml reappeared: warn once, never read.
                if crate::config::widgets_path().exists() {
                    if !self.catalog.retired_file_warned {
                        self.catalog.retired_file_warned = true;
                        eprintln!(
                            "widgets: {} is retired (migrated to placements/Lua defaults) and ignored; remove it",
                            crate::config::widgets_path().display()
                        );
                    }
                } else {
                    self.catalog.retired_file_warned = false;
                }
                // Shared components hot-reload the same way: any
                // `components/*.lua` change rebuilds every Lua state
                // (same defs, fresh runtimes) and drops the cached
                // notification renderer so it re-execs the library.
                if self.catalog.library_changed() {
                    self.desktop.overlays.invalidate();
                    return Command::batch(vec![
                        Command::done(Plant::Config(ConfigEvent::WidgetsReloaded(
                            self.catalog.definitions.clone(),
                        ))),
                        self.notify_parse_error(),
                    ]);
                }
                self.notify_parse_error()
            }
            Plant::Config(ConfigEvent::ConfigReloaded(mut cfg)) => {
                if serde_json::to_value(&cfg.desktop).ok()
                    == serde_json::to_value(&self.config.desktop).ok()
                {
                    for (next, current) in cfg.desktop.iter_mut().zip(&self.config.desktop) {
                        next.id.clone_from(&current.id);
                    }
                }
                let bars_changed = match (
                    serde_json::to_value(&cfg.bar),
                    serde_json::to_value(&self.config.bar),
                ) {
                    (Ok(next), Ok(current)) => next != current,
                    _ => true,
                };
                // The config file changed under us: the on-disk config wins and replaces
                // any staged local edits. Pending save timers are cancelled.
                if self.config_jobs.dirty {
                    println!("riced: config changed on disk — replacing staged local edits");
                }
                self.config_jobs.invalidate();
                let images_changed = cfg.background != self.config.background;
                self.config = cfg;
                if crate::icons::set_theme(&self.config.theme.icon_theme) {
                    self.services.tray.refresh_icons();
                }
                // Bars resolve [[bar]] entries once at spawn; re-resolve
                // here so slot/align/geometry edits apply live (anchor
                // changes still need a respawn). On-disk wins by contract.
                // Only changed bar recipes regenerate placement IDs and VMs;
                // palette-only changes retain existing app state and popups.
                let bars = if bars_changed {
                    Top::resync_bars_from_config(self)
                } else {
                    Vec::new()
                };
                let mut cmds = Vec::new();
                cmds.push(self.refresh_wallpapers());
                if images_changed {
                    let mut previews = Vec::new();
                    for (id, setting) in &mut self.windows.settings {
                        if let Some(draft) = &mut setting.wallpaper_draft {
                            if draft.dirty() {
                                draft.error = Some(
                                    "Wallpaper configuration changed. Discard to reload.".into(),
                                );
                            } else {
                                let assets = draft.assets_revision + 1;
                                let running = draft.preview_running;
                                *draft =
                                    crate::wallpaper::draft::Draft::new(&self.config.background);
                                draft.assets_revision = assets;
                                draft.preview_running = running;
                                previews.push(*id);
                            }
                        }
                    }
                    for id in previews {
                        cmds.push(self.load_wallpaper_preview(id));
                    }
                }
                for bar_id in bars {
                    cmds.push(Top::apply_layout(self, bar_id));
                }
                if bars_changed {
                    cmds.push(super::overlay::refresh(self, true));
                    Top::init_widget_lua(self);
                    for id in self.windows.popups.keys().copied().collect::<Vec<_>>() {
                        cmds.push(Popup::handle_dismiss(self, id));
                    }
                } else {
                    cmds.push(self.refresh_theme());
                }
                if cmds.is_empty() {
                    Command::none()
                } else {
                    Command::batch(cmds)
                }
            }
            Plant::Config(ConfigEvent::WidgetsReloaded(defs)) => {
                // Widgets dir changed under us: swap the live registry
                // and rebuild Lua states (scripts may have changed too).
                // Open menus reference dead states, so they close;
                // bars re-resolve placements on the next redraw (Scope::All).
                self.catalog.definitions = defs;
                Top::init_widget_lua(self);
                let stale: Vec<iced::window::Id> = self.windows.popups.keys().copied().collect();
                if stale.is_empty() {
                    Command::none()
                } else {
                    Command::batch(
                        stale
                            .into_iter()
                            .map(|id| Popup::handle_dismiss(self, id))
                            .collect::<Vec<_>>(),
                    )
                }
            }
            Plant::Config(ConfigEvent::Patch(patch)) => {
                let palette_changed = matches!(
                    patch,
                    ConfigPatch::ThemeName(_)
                        | ConfigPatch::ThemeDarkmode(_)
                        | ConfigPatch::ComponentDefaults(_)
                );
                // Local-first single source of truth: mutate the live
                // (in-memory) config and redraw All so every subscribed view
                // previews the value next frame. The disk write is staged,
                // not per-tick: slider drags fire dozens of patches a second
                // and must not rewrite config.lua each time. `SaveTimer`
                // persists once idle; panel close flushes synchronously.
                // Image add/remove re-syncs the pre-decoded wallpaper cache;
                // move/scale/z reuse the same pixels.
                let sync = matches!(
                    patch,
                    ConfigPatch::AddImage(_) | ConfigPatch::RemoveImage { .. }
                );
                // Live recipe patches rebuild tiles without generating a theme.
                // Settings wallpaper controls use their separate draft events.
                let wallpaper_touched = matches!(
                    patch,
                    ConfigPatch::AddImage(_)
                        | ConfigPatch::WallpaperMode(_)
                        | ConfigPatch::MonitorWallpaper(_)
                        | ConfigPatch::MoveImage { .. }
                        | ConfigPatch::RemoveImage { .. }
                        | ConfigPatch::SetImageScale { .. }
                        | ConfigPatch::SetImageSize { .. }
                        | ConfigPatch::ScaleImage { .. }
                        | ConfigPatch::SetImageZ { .. }
                );
                // Theme switches also re-render the templates dir (sys
                // `change_theme` equivalent) — off the update thread,
                // since hooks can block. Empty/`"off"` dirs resolve to
                // `None` and skip silently.
                let retemplate_dir = matches!(
                    patch,
                    ConfigPatch::ThemeName(_) | ConfigPatch::ThemeDarkmode(_)
                )
                .then(|| crate::theme_gen::effective_templates_dir(&self.config.theme))
                .flatten();
                // Removal shifts image indices: capture the index before
                // `apply` moves `patch`, then remap every panel's pick.
                let removed_index = match &patch {
                    ConfigPatch::RemoveImage { index } => Some(*index),
                    _ => None,
                };
                self.config.apply(patch);
                if crate::icons::set_theme(&self.config.theme.icon_theme) {
                    self.services.tray.refresh_icons();
                }
                if let Some(index) = removed_index {
                    for setting in self.windows.settings.values_mut() {
                        setting.image_removed(index);
                    }
                }
                let save_seq = self.config_jobs.stage();
                let save =
                    Command::perform(tokio::time::sleep(Self::CONFIG_SAVE_DELAY), move |_| {
                        Plant::Config(ConfigEvent::SaveTimer(save_seq))
                    });
                let mut commands = vec![save];
                if palette_changed {
                    commands.push(self.refresh_theme());
                }
                if let Some(dir) = retemplate_dir {
                    commands.push(Self::retemplate_command(dir, &self.config));
                }
                if wallpaper_touched || sync {
                    commands.push(self.refresh_wallpapers());
                }
                Command::batch(commands)
            }
            Plant::Config(ConfigEvent::SaveTimer(seq)) => {
                // Stale timers (superseded by a later patch) or a clean
                // state (flushed on panel close) never write twice.
                if !self.config_jobs.should_flush(seq) {
                    return Command::none();
                }
                self.flush_config_save();
                Command::none()
            }
            Plant::Config(ConfigEvent::SaveNow) => {
                // Slider released: drags preview in memory, release persists.
                self.flush_config_save();
                Command::none()
            }
            Plant::Config(ConfigEvent::TemplatesDone(errors)) => {
                for err in &errors {
                    eprintln!("riced: template: {err}");
                }
                if errors.is_empty() {
                    return self.reclaim_image_memory(false);
                }
                let n = Notification::internal(
                    &self.config.notifications,
                    "Templates failed",
                    errors.join("\n"),
                    2,
                );
                Notification::handle_arrived(self, n)
            }
            Plant::Config(ConfigEvent::RegenTimer(seq)) => {
                // Stale timers (superseded by a later arm, or already
                // consumed by a fire) never run twice.
                if seq != self.theme_jobs.sequence {
                    return Command::none();
                }
                self.fire_regen_theme()
            }
            Plant::Config(ConfigEvent::ThemeRegenerated(errors)) => {
                self.theme_jobs.running = false;
                let mut commands = vec![self.reclaim_image_memory(false)];
                if errors.is_empty() && self.config.theme.name == "dynamic" {
                    commands.push(self.refresh_theme());
                }
                // The completion itself is the notification (success or
                // failure); explicit Applies landing mid-flight chain a follow-up.
                let regen_cmd = if errors.is_empty() {
                    println!("riced: dynamic theme regenerated");
                    let n = Notification::internal(
                        &self.config.notifications,
                        "Theme regenerated",
                        "dynamic.json applied".to_string(),
                        1,
                    );
                    Notification::handle_arrived(self, n)
                } else {
                    for err in &errors {
                        eprintln!("riced: dynamic theme: {err}");
                    }
                    let n = Notification::internal(
                        &self.config.notifications,
                        "Theme regen failed",
                        errors.join("\n"),
                        2,
                    );
                    Notification::handle_arrived(self, n)
                };
                // Applies that landed mid-flight re-dirty the flag; chain one
                // follow-up regen instead of dropping them.
                if self.theme_jobs.dirty {
                    commands.push(self.arm_regen_theme());
                }
                commands.push(regen_cmd);
                Command::batch(commands)
            }
            Plant::Config(ConfigEvent::ThemeReloaded) => self.refresh_theme(),
            Plant::TopPlot(TopEvent::Pressed(id, button)) => Top::handle_press(self, id, button),
            Plant::TopPlot(TopEvent::Released(id, button)) => Top::handle_release(self, id, button),
            Plant::TopPlot(TopEvent::Widget(event)) => match event {
                WidgetEvent::Pressed(id, pos, widget) => {
                    Top::handle_widget_press(self, id, pos, widget)
                }
                WidgetEvent::Released(id, pos, widget) => {
                    Top::handle_widget_release(self, id, pos, widget)
                }
                WidgetEvent::Tick => Top::handle_widget_tick(self),
                WidgetEvent::Anim => Top::handle_anim_frame(self),
                WidgetEvent::PopupOpened(id, revision) => Popup::handle_opened(self, id, revision),
                WidgetEvent::PopupClosed(id, revision) => Popup::handle_closed(self, id, revision),
                WidgetEvent::Changed => Popup::refresh_bodies(self),
                WidgetEvent::PopupSelect(id, action) => Popup::handle_select(self, id, action),
                WidgetEvent::PopupMeasured(id, revision, height) => {
                    Popup::handle_measured(self, id, revision, height)
                }
                WidgetEvent::CellAction(bar, placement, action) => {
                    Top::handle_cell_action(self, bar, placement, action)
                }
                WidgetEvent::CellHover(_bar, placement, hovered) => {
                    Top::handle_cell_hover(self, placement, hovered)
                }
                WidgetEvent::PopupHover(id, list, key, hovered) => {
                    Popup::handle_hover(self, id, list, key, hovered)
                }
            },
            Plant::TopPlot(TopEvent::Bar(event)) => match event {
                BarEvent::Length(id, value) => Top::handle_set_length(self, id, value),
                BarEvent::Thickness(id, value) => Top::handle_set_thickness(self, id, value),
                BarEvent::Slots(id, value) => Top::handle_set_slots(self, id, value),
                BarEvent::SlotAlign(id, pos, align) => {
                    Top::handle_set_slot_align(self, id, pos, align)
                }
                BarEvent::WidgetLayout {
                    bar,
                    expected,
                    widgets,
                } => Top::handle_widget_layout(self, bar, expected, widgets),
                BarEvent::WidgetProp {
                    bar,
                    placement,
                    patch,
                } => Top::handle_widget_prop(self, bar, &placement, patch),
                BarEvent::SlotPadding(id, value) => Top::handle_set_slot_padding(self, id, value),
                BarEvent::SlotSpacing(id, value) => Top::handle_set_slot_spacing(self, id, value),
            },
            Plant::TopPlot(TopEvent::Style(event)) => match event {
                StyleEvent::Opacity(id, value) => Top::handle_set_opacity(self, id, value),
                StyleEvent::Floating(id, value) => Top::handle_set_floating(self, id, value),
                StyleEvent::Margin(id, Edge::Top, value) => {
                    Top::handle_set_margin_top(self, id, value)
                }
                StyleEvent::Margin(id, Edge::Right, value) => {
                    Top::handle_set_margin_right(self, id, value)
                }
                StyleEvent::Margin(id, Edge::Bottom, value) => {
                    Top::handle_set_margin_bottom(self, id, value)
                }
                StyleEvent::Margin(id, Edge::Left, value) => {
                    Top::handle_set_margin_left(self, id, value)
                }
                StyleEvent::Radius(id, Corner::TopLeft, value) => {
                    Top::handle_set_radius_tl(self, id, value)
                }
                StyleEvent::Radius(id, Corner::TopRight, value) => {
                    Top::handle_set_radius_tr(self, id, value)
                }
                StyleEvent::Radius(id, Corner::BottomLeft, value) => {
                    Top::handle_set_radius_bl(self, id, value)
                }
                StyleEvent::Radius(id, Corner::BottomRight, value) => {
                    Top::handle_set_radius_br(self, id, value)
                }
            },
            Plant::TopPlot(TopEvent::Remove(id)) => Top::handle_remove(self, id),
            Plant::TrayChanged => {
                Top::refresh_services(self);
                Popup::refresh_bodies(self)
            }
            Plant::WaylandChanged => {
                self.services.wayland_changes.acknowledge();
                Top::refresh_wayland(self);
                Command::batch([
                    Popup::refresh_service_bodies(self),
                    super::desktop_widgets::refresh_services(self),
                    super::overlay::refresh(self, true),
                ])
            }
            Plant::Notify(NotifyEvent::Arrived(n)) => Notification::handle_arrived(self, *n),
            Plant::Notify(NotifyEvent::Dismissed(id)) => Notification::handle_dismissed(self, id),
            Plant::Notify(NotifyEvent::PeerClosed(id)) => {
                Notification::handle_peer_closed(self, id)
            }
            Plant::Notify(NotifyEvent::Invoke(id, key)) => {
                Notification::handle_invoke(self, id, key)
            }
            Plant::Notify(NotifyEvent::Tick) => Notification::handle_tick(self),
            Plant::Notify(NotifyEvent::DBusUp) => {
                eprintln!("riced: notifications: D-Bus server up");
                Command::none()
            }
            Plant::SettingPlot(SettingEvent::Select(id, page)) => {
                Setting::handle_select(self, id, page)
            }
            Plant::SettingPlot(SettingEvent::IconPreviewsReady { id, result }) => {
                if let Some(setting) = self.windows.settings.get_mut(&id) {
                    setting.icon_previews_ready(result);
                }
                Command::none()
            }
            Plant::SettingPlot(SettingEvent::SelectBar(id, bar)) => {
                Setting::handle_select_bar(self, id, bar)
            }
            Plant::SettingPlot(SettingEvent::SelectImage(id, index)) => {
                Setting::handle_select_image(self, id, index)
            }
            Plant::SettingPlot(SettingEvent::SelectSlot(id, pos)) => {
                Setting::handle_select_slot(self, id, pos)
            }
            Plant::SettingPlot(SettingEvent::SelectPlacement(id, bar, placement)) => {
                Setting::handle_select_placement(self, id, bar, placement)
            }
            Plant::SettingPlot(SettingEvent::ComponentsEdit(id, edit)) => {
                crate::shell::screens::setting::components::handle_edit(self, id, edit)
            }
            Plant::SettingPlot(SettingEvent::MapViewChanged { id, view }) => {
                if let Some(setting) = self.windows.settings.get_mut(&id) {
                    setting.set_map_view(view);
                }
                Command::none()
            }
            Plant::SettingPlot(SettingEvent::WallpaperPatch(id, patch)) => {
                if let Some(setting) = self.windows.settings.get_mut(&id) {
                    if matches!(&patch, ConfigPatch::WallpaperMode(_)) {
                        setting.cancel_wallpaper_drag();
                    }
                    if let ConfigPatch::RemoveImage { index } = &patch {
                        setting.image_removed(*index);
                    }
                    if let Some(draft) = &mut setting.wallpaper_draft {
                        draft.patch(patch);
                    }
                }
                self.load_wallpaper_preview(id)
            }
            Plant::SettingPlot(SettingEvent::SelectMonitor(id, output)) => {
                if let Some(draft) = self
                    .windows
                    .settings
                    .get_mut(&id)
                    .and_then(|s| s.wallpaper_draft.as_mut())
                {
                    draft.selected_output = Some(output);
                }
                Command::none()
            }
            Plant::SettingPlot(SettingEvent::PickMonitorWallpaper(id, output)) => Command::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter(
                            "Images",
                            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff"],
                        )
                        .pick_file()
                        .await
                        .map(|file| file.path().to_string_lossy().into_owned())
                },
                move |path| {
                    Plant::SettingPlot(SettingEvent::MonitorWallpaperPicked(
                        id,
                        output.clone(),
                        path,
                    ))
                },
            ),
            Plant::SettingPlot(SettingEvent::MonitorWallpaperPicked(id, output, path)) => {
                if let Some(path) = path
                    && let Some(draft) = self
                        .windows
                        .settings
                        .get_mut(&id)
                        .and_then(|s| s.wallpaper_draft.as_mut())
                {
                    let mut wallpaper = draft
                        .recipe
                        .monitor
                        .iter()
                        .find(|m| m.output == output)
                        .cloned()
                        .unwrap_or(crate::config::MonitorWallpaper {
                            output,
                            ..Default::default()
                        });
                    wallpaper.path = path;
                    draft.patch(ConfigPatch::MonitorWallpaper(wallpaper));
                }
                self.load_wallpaper_preview(id)
            }
            Plant::SettingPlot(SettingEvent::PickWallpaper(id)) => Command::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter(
                            "Images",
                            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff"],
                        )
                        .pick_file()
                        .await
                        .map(|file| file.path().to_string_lossy().into_owned())
                },
                move |path| Plant::SettingPlot(SettingEvent::WallpaperPicked(id, path)),
            ),
            Plant::SettingPlot(SettingEvent::WallpaperPicked(id, path)) => {
                if let Some(path) = path
                    && let Some(draft) = self
                        .windows
                        .settings
                        .get_mut(&id)
                        .and_then(|setting| setting.wallpaper_draft.as_mut())
                {
                    let z = draft
                        .recipe
                        .image
                        .iter()
                        .map(|image| image.z)
                        .max()
                        .unwrap_or(-1)
                        + 1;
                    draft.patch(ConfigPatch::AddImage(crate::config::BackgroundImage {
                        path,
                        z,
                        ..Default::default()
                    }));
                }
                self.load_wallpaper_preview(id)
            }
            Plant::SettingPlot(SettingEvent::WallpaperPreviewReady {
                id,
                revision,
                result,
            }) => {
                if let Some(draft) = self
                    .windows
                    .settings
                    .get_mut(&id)
                    .and_then(|setting| setting.wallpaper_draft.as_mut())
                {
                    if draft.preview_running == Some(revision) {
                        draft.preview_running = None;
                    }
                    if draft.assets_revision == revision {
                        draft.loading = false;
                        match result {
                            Ok(sources) => {
                                draft.sources = sources;
                                draft.error = None;
                            }
                            Err(error) => draft.error = Some(error),
                        }
                    }
                }
                Command::batch([
                    self.load_wallpaper_preview(id),
                    self.reclaim_image_memory(false),
                ])
            }
            Plant::SettingPlot(SettingEvent::ApplyWallpaper(id)) => self.apply_wallpaper(id),
            Plant::SettingPlot(SettingEvent::DiscardWallpaper(id)) => self.discard_wallpaper(id),
            Plant::Config(ConfigEvent::WallpaperBaked { generation, result }) => {
                self.wallpaper_baked(generation, result)
            }
            Plant::Config(ConfigEvent::ReclaimImageMemory { .. }) => {
                // Host-side cleanup only; a redraw accompanies the message
                // when scheduled, via `redraw_scope`.
                crate::ui::assets::release_unused();
                crate::theme_gen::trim_memory();
                Command::none()
            }
            Plant::TopPlot(TopEvent::Sow) => {
                // delegate to Top layer (closest-edge detection and spawn)
                let menu_pos = self
                    .desktop
                    .context_menu
                    .as_ref()
                    .map(|cm| Point::new(cm.x, cm.y));
                let menu_output = self.desktop.context_menu.as_ref().and_then(|cm| cm.output);
                if let Some(cm) = &mut self.desktop.context_menu {
                    cm.open = false;
                }
                if let Some(cmd) = Top::handle_add(self, menu_pos, menu_output) {
                    return cmd;
                }
                Command::none()
            }
            _ => Command::none(),
        }
    }
}

pub fn redraw_scope(message: &Plant) -> Scope {
    match message {
        Plant::TopPlot(TopEvent::Widget(WidgetEvent::CellAction(id,_,action)
            | WidgetEvent::PopupSelect(id,action))) if action.is_animation_frame() => Scope::Window(*id),
        Plant::Overlay(super::overlay::OverlayEvent::Action {surface,action,..})
            if action.is_animation_frame() => Scope::Window(*surface),
        Plant::Desktop(super::desktop_widgets::DesktopEvent::Tick) => Scope::None,
        // Source motion resolves the destination before requesting its redraw.
        Plant::Desktop(super::desktop_widgets::DesktopEvent::SourceMotion { .. }) => Scope::None,
        // Continuous drag motion only changes its preview surface. Output
        // transitions emit Changed as well to clear the previous preview.
        Plant::Desktop(super::desktop_widgets::DesktopEvent::Move { surface, .. }) => Scope::Window(*surface),
        Plant::Desktop(super::desktop_widgets::DesktopEvent::PreviewChanged(surface)) => Scope::Window(*surface),
        // Background selection tick is throttled drag update — must redraw all outputs
        Plant::BackgroundPlot(BackgroundEvent::SelectionTick) => Scope::All,
        // Post-creation heal (see `repaint_burst`): full redraw; the handler
        // also bumps `repaint_seq` so the heal is a real state transition.
        Plant::BackgroundPlot(BackgroundEvent::Repaint) => Scope::All,
        // PanelWindow press/release changes selecting/context_menu/hold → All.
        // A Graft release also ends selection via the safety net → All.
        Plant::BackgroundPlot(BackgroundEvent::Pressed(..))
        | Plant::BackgroundPlot(BackgroundEvent::ContextMenuAction(..))
        | Plant::BackgroundPlot(BackgroundEvent::Released(..))
        | Plant::BackgroundPlot(BackgroundEvent::Hover(..))
        // Accept re-emits as a Patch (already Scope::All); cancel is silent.
        | Plant::BackgroundPlot(BackgroundEvent::WallpaperPicked(Some(_))) => Scope::All,
        Plant::BackgroundPlot(BackgroundEvent::PickWallpaper)
        | Plant::BackgroundPlot(BackgroundEvent::WallpaperPicked(None)) => Scope::None,
        // Lua-widget timer only runs due scripts (repaint goes through
        // Widget(Changed) when an output actually moved).
        Plant::TopPlot(TopEvent::Widget(WidgetEvent::Tick)) => Scope::None,
        // Notification arrivals/dismissals repaint (transient layer);
        // the gated 250ms sweep tick repaints too (short-lived), the
        // D-Bus-up note never does.
        Plant::Notify(NotifyEvent::Arrived(_))
        | Plant::Notify(NotifyEvent::Dismissed(_))
        | Plant::Notify(NotifyEvent::PeerClosed(_))
        | Plant::Notify(NotifyEvent::Invoke(..))
        | Plant::Notify(NotifyEvent::Tick) => Scope::All,
        Plant::Notify(NotifyEvent::DBusUp) => Scope::None,
        Plant::TrayChanged | Plant::WaylandChanged => Scope::All,
        Plant::Overlay(super::overlay::OverlayEvent::Tick | super::overlay::OverlayEvent::Measured { .. }) => Scope::None,
        Plant::Overlay(_) => Scope::All,
        // Widget press only records; every other widget event repaints
        // (animation frames while a motion runs, releases opening a
        // popup, actions refreshing tiles, changed outputs).
        Plant::TopPlot(TopEvent::Widget(WidgetEvent::Pressed(..))) => Scope::None,
        Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupMeasured(id, _, _))) => Scope::Window(*id),
        Plant::TopPlot(TopEvent::Widget(_))
        | Plant::TopPlot(TopEvent::Bar(_))
        | Plant::TopPlot(TopEvent::Style(_))
        | Plant::TopPlot(TopEvent::Pressed(..))
        | Plant::TopPlot(TopEvent::Released(..))
        | Plant::TopPlot(TopEvent::Remove(..)) => Scope::All,
        Plant::Graft(_, Event::Mouse(iced::mouse::Event::ButtonReleased(_))) => Scope::All,
        // Pointer movement is throttled by the subscription; redraw only
        // its own surface so native hover/pressed status survives rebuilds.
        Plant::Graft(id, Event::Mouse(
            iced::mouse::Event::CursorMoved { .. }
            | iced::mouse::Event::CursorEntered
            | iced::mouse::Event::CursorLeft
            | iced::mouse::Event::ButtonPressed(_)
        )) => Scope::Window(*id),
        // ConfigTick is a cheap mtime check — redraw only on actual reload.
        // IpcPoll just stats an (usually absent) file — same, no redraw.
        // TemplatesDone only logs hook/template errors; the theme itself
        // already repainted via the Patch that triggered it.
        Plant::Config(ConfigEvent::ConfigTick)
        | Plant::Config(ConfigEvent::TemplatesDone(_))
        | Plant::Config(ConfigEvent::RegenTimer(_))
        | Plant::Config(ConfigEvent::SaveTimer(_))
        | Plant::Config(ConfigEvent::SaveNow)
        | Plant::Config(ConfigEvent::ReclaimImageMemory { redraw: false })
        | Plant::IpcPoll => Scope::None,
        // Closing an image-owning window: repaint every surface so the
        // live renderers run their insert-gated image-cache trim once the
        // dropped handles are gone (per-renderer caches die with windows).
        Plant::Config(ConfigEvent::ReclaimImageMemory { redraw: true }) => Scope::All,
        Plant::Config(ConfigEvent::ConfigReloaded(_))
        | Plant::Config(ConfigEvent::Patch(_))
        | Plant::Config(ConfigEvent::WidgetsReloaded(_)) => Scope::All,
        // Fresh dynamic.json on disk: repaint so the new palette applies
        // (theme::sync picks the new mtime up during the redraw).
        Plant::Config(ConfigEvent::ThemeRegenerated(_)) => Scope::All,
        Plant::Config(ConfigEvent::ThemeReloaded) => Scope::All,
        // Settings page/bar select / map pan-zoom only affects its own window.
        Plant::SettingPlot(
            SettingEvent::Select(id, _)
            | SettingEvent::IconPreviewsReady { id, .. }
            | SettingEvent::SelectBar(id, _)
            | SettingEvent::SelectImage(id, _)
            | SettingEvent::SelectMonitor(id, _)
            | SettingEvent::PickMonitorWallpaper(id, _)
            | SettingEvent::MonitorWallpaperPicked(id, _, _)
            | SettingEvent::SelectSlot(id, _)
            | SettingEvent::SelectPlacement(id, _, _)
            | SettingEvent::ComponentsEdit(id, _)
            | SettingEvent::MapViewChanged { id, .. }
            | SettingEvent::WallpaperPatch(id, _)
            | SettingEvent::PickWallpaper(id)
            | SettingEvent::WallpaperPicked(id, _)
            | SettingEvent::WallpaperPreviewReady {id, ..}
            | SettingEvent::ApplyWallpaper(id)
            | SettingEvent::DiscardWallpaper(id),
        ) => Scope::Window(*id),
        Plant::Graft(_, Event::Mouse(_)) => Scope::None,
        Plant::Graft(_, _) => Scope::None,
        Plant::Wayland(_) => Scope::All,
        _ => Scope::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallpaper_test_plots() -> Plots {
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        Plots {
            windows: Default::default(),
            shell_events: receiver,
            input: Default::default(),
            desktop: Default::default(),
            config: Config::default(),
            config_mtime: None,
            catalog: super::super::catalog::WidgetCatalog::new(Vec::new()),
            placements: Default::default(),
            animation: Default::default(),
            services: crate::services::state::ServiceState::new(),
            config_jobs: Default::default(),
            theme_mtime: None,
            theme_jobs: Default::default(),
            notification: Default::default(),
        }
    }

    #[test]
    fn desktop_capture_cancel_is_idempotent_and_stale_surface_events_are_ignored() {
        use super::super::desktop_widgets::{DesktopEvent, DragSession};
        use iced::futures::StreamExt;
        let mut plots = wallpaper_test_plots();
        let source = iced::window::Id::unique();
        plots.windows.settings.insert(source, Setting::default());
        plots.windows.ids.insert(source, PlotInfo::Setting);
        let captures = [
            (
                iced_wayland_subscriber::OutputId(1),
                iced::window::Id::unique(),
            ),
            (
                iced_wayland_subscriber::OutputId(2),
                iced::window::Id::unique(),
            ),
        ];
        for (output, surface) in captures {
            plots.windows.drags.insert(output, surface);
            plots.windows.ids.insert(surface, PlotInfo::Drag(output));
        }
        plots.desktop.drag = Some(DragSession {
            source,
            widget: "rectangle".into(),
            placement: None,
            cursor: None,
            pickup: Default::default(),
        });
        let stale = iced::window::Id::unique();
        let _ = plots.update(Plant::Desktop(DesktopEvent::Move {
            surface: stale,
            output: captures[0].0,
            position: Point::new(20.0, 30.0),
        }));
        assert!(plots.desktop.drag.as_ref().unwrap().cursor.is_none());
        let _ = plots.update(Plant::Desktop(DesktopEvent::Cancel { surface: stale }));
        assert!(plots.desktop.drag.is_some());
        let _ = plots.update(Plant::Desktop(DesktopEvent::Move {
            surface: captures[0].1,
            output: captures[0].0,
            position: Point::new(20.0, 30.0),
        }));
        assert_eq!(
            plots.desktop.drag.as_ref().unwrap().cursor.unwrap().1,
            Point::new(20.0, 30.0)
        );
        let task = plots.update(Plant::Desktop(DesktopEvent::Cancel {
            surface: captures[0].1,
        }));
        assert!(plots.desktop.drag.is_none());
        assert!(plots.windows.drags.is_empty());
        assert!(plots.windows.settings.contains_key(&source));
        let actions = iced::futures::executor::block_on(
            iced_runtime::task::into_stream(task)
                .unwrap()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, Action::Window(WindowAction::Close(_))))
                .count(),
            2
        );
        assert!(
            iced_runtime::task::into_stream(plots.update(Plant::Desktop(DesktopEvent::Cancel {
                surface: captures[0].1
            })))
            .is_none()
        );
        // A native close from the old session must not cancel a newer one.
        plots.desktop.drag = Some(DragSession {
            source,
            widget: "rectangle".into(),
            placement: None,
            cursor: None,
            pickup: Default::default(),
        });
        let _ = plots.update(Plant::Wayland(LandEvent::Closed(captures[0].1)));
        assert!(plots.desktop.drag.is_some());
        let _ = plots.update(Plant::Wayland(LandEvent::Closed(source)));
        assert!(plots.desktop.drag.is_none());
        assert!(!plots.windows.settings.contains_key(&source));
        assert_eq!(
            redraw_scope(&Plant::Desktop(DesktopEvent::Tick)),
            Scope::None
        );
        assert_eq!(
            redraw_scope(&Plant::Desktop(DesktopEvent::Changed)),
            Scope::All
        );
        assert_eq!(
            redraw_scope(&Plant::Desktop(DesktopEvent::Cancel { surface: stale })),
            Scope::All
        );
    }

    #[test]
    fn desktop_drag_motion_redraws_only_its_surface_until_crossing_outputs() {
        use super::super::desktop_widgets::{self, DesktopEvent, DragSession};
        use iced::futures::StreamExt;

        let mut plots = wallpaper_test_plots();
        let first = iced_wayland_subscriber::OutputId(1);
        let second = iced_wayland_subscriber::OutputId(2);
        let first_surface = iced::window::Id::unique();
        let second_surface = iced::window::Id::unique();
        plots.windows.drags.insert(first, first_surface);
        plots.windows.drags.insert(second, second_surface);
        plots.desktop.drag = Some(DragSession {
            source: iced::window::Id::unique(),
            widget: "rectangle".into(),
            placement: None,
            cursor: Some((first, Point::ORIGIN)),
            pickup: Default::default(),
        });

        let motion = DesktopEvent::Move {
            surface: first_surface,
            output: first,
            position: Point::new(10.0, 20.0),
        };
        assert_eq!(
            redraw_scope(&Plant::Desktop(DesktopEvent::SourceMotion {
                source: plots.desktop.drag.as_ref().unwrap().source,
                position: Point::new(10.0, 20.0),
            })),
            Scope::None
        );
        assert_eq!(
            redraw_scope(&Plant::Desktop(DesktopEvent::PreviewChanged(first_surface))),
            Scope::Window(first_surface)
        );
        assert_eq!(
            redraw_scope(&Plant::Desktop(motion.clone())),
            Scope::Window(first_surface)
        );
        assert!(
            iced_runtime::task::into_stream(desktop_widgets::update(&mut plots, motion)).is_none()
        );

        let crossing = DesktopEvent::Move {
            surface: second_surface,
            output: second,
            position: Point::new(30.0, 40.0),
        };
        let actions = iced::futures::executor::block_on(
            iced_runtime::task::into_stream(desktop_widgets::update(&mut plots, crossing))
                .unwrap()
                .collect::<Vec<_>>(),
        );
        let [Action::Output(message @ Plant::Desktop(DesktopEvent::Changed))] = actions.as_slice()
        else {
            panic!("crossing outputs must clear the previous preview: {actions:?}");
        };
        assert_eq!(redraw_scope(message), Scope::All);
        assert_eq!(
            plots.desktop.drag.as_ref().unwrap().cursor,
            Some((second, Point::new(30.0, 40.0)))
        );

        let motion = DesktopEvent::Move {
            surface: second_surface,
            output: second,
            position: Point::new(50.0, 60.0),
        };
        assert_eq!(
            redraw_scope(&Plant::Desktop(motion.clone())),
            Scope::Window(second_surface)
        );
        assert!(
            iced_runtime::task::into_stream(desktop_widgets::update(&mut plots, motion)).is_none()
        );
    }

    #[test]
    fn desktop_capture_routes_motion_and_drop_in_surface_coordinates() {
        use super::super::desktop_widgets::{self, DesktopEvent};
        use iced::advanced::{layout, renderer::Headless, widget::Tree};
        let plots = wallpaper_test_plots();
        let surface = iced::window::Id::unique();
        let output = iced_wayland_subscriber::OutputId(2);
        let mut element = desktop_widgets::view(&plots, surface, output);
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut tree = Tree::new(&element);
        let size = iced::Size::new(1080.0, 1920.0);
        let layout = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, size),
        );
        let position = Point::new(300.0, 500.0);
        let mut messages = Vec::new();
        for event in [
            iced::mouse::Event::CursorMoved { position },
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Right),
        ] {
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(event),
                iced::advanced::Layout::new(&layout),
                iced::mouse::Cursor::Available(position),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &iced::Rectangle::with_size(size),
            );
        }
        assert!(messages.iter().any(|message| matches!(message, Plant::Desktop(DesktopEvent::Move { surface:target, output:owner, position:point }) if *target==surface && *owner==output && *point==position)));
        assert!(messages.iter().any(|message| matches!(message, Plant::Desktop(DesktopEvent::Drop { surface:target, output:owner, position:point, session: None }) if *target==surface && *owner==output && *point==position)));
        assert!(messages.iter().any(|message| matches!(message, Plant::Desktop(DesktopEvent::Cancel { surface:target }) if *target==surface)));
    }

    #[test]
    fn session_popup_auto_height_measures_fill_content_and_rejects_stale_results() {
        use crate::ui::node::{NodeLength, WidgetNode};
        use iced::advanced::{layout, renderer::Headless, widget::Tree};
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(13.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        crate::lua::test_support::load_seed_components(&lua);
        let value = lua.load(r#"return iced.popup(iced.card({title='Session',icon='power',body=iced.menu({id='session-actions',items={
            {label='Suspend',action='suspend'}, {label='Hibernate',action='hibernate'},
            {label='Reboot',action='reboot'}, {label='Power off',action='poweroff'}
        }})}):height('fill')):width(220)"#).eval().unwrap();
        let content = Popup::parse_popup_content(value, &Default::default()).unwrap();
        let mut plots = wallpaper_test_plots();
        let id = iced::window::Id::unique();
        plots.windows.popups.insert(
            id,
            Popup {
                win_id: id,
                bar_id: iced::window::Id::unique(),
                placement: "session".into(),
                auto_width: false,
                max_width: 1920.0,
                tree: content.tree,
                hovered: Default::default(),
                size: 13.0,
                w: 220,
                h: 64,
                auto_height: true,
                max_height: 1080.0,
                sizing_revision: 7,
                measured_revision: None,
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    iced::window::Id::unique(),
                    iced_exwlshell::reexport::PixelSize::try_px(220, 64).unwrap(),
                    (0, 0),
                ),
                on_open: None,
                on_close: None,
                opening: None,
                closing: None,
                phase: 0,
            },
        );
        let measure = |plots: &Plots| {
            let popup = &plots.windows.popups[&id];
            let mut element = popup.view(plots);
            let mut tree = Tree::new(&element);
            let size = iced::Size::new(popup.w as f32, popup.h as f32);
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, size),
            );
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &Event::Window(iced::window::Event::RedrawRequested(Instant::now())),
                iced::advanced::Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &iced::Rectangle::with_size(size),
            );
            messages
        };
        let messages = measure(&plots);
        assert_eq!(messages.len(), 1);
        let Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupMeasured(_, revision, height))) =
            &messages[0]
        else {
            panic!("measurement expected");
        };
        assert_eq!(*revision, 7);
        let session_height = height.height;
        assert!(
            (100..300).contains(&session_height),
            "four rows should fit: {session_height}"
        );
        assert_eq!(redraw_scope(&messages[0]), Scope::Window(id));
        use iced::futures::StreamExt;
        let command = plots.update(messages[0].clone());
        let actions = iced::futures::executor::block_on(
            iced_runtime::task::into_stream(command)
                .unwrap()
                .collect::<Vec<_>>(),
        );
        let [
            Action::Output(Plant::PopUpReposition {
                id: target,
                settings,
            }),
        ] = actions.as_slice()
        else {
            panic!("popup sizing must use the backend's positioner action");
        };
        assert_eq!(*target, id);
        assert_eq!(
            settings.size,
            iced_exwlshell::reexport::PixelSize::try_px(220, session_height).unwrap()
        );
        assert_eq!(plots.windows.popups[&id].h, session_height);
        let popup = &plots.windows.popups[&id];
        let reopen = crate::shell::screens::popup::PopupContent {
            tree: popup.tree.clone(),
            width: Some(220.0),
            height: None,
            on_open: popup.on_open.clone(),
            on_close: popup.on_close.clone(),
        };
        assert_eq!(
            Popup::initial_size(
                &mut plots.windows.popup_heights,
                1920.0,
                1080.0,
                &reopen,
                13.0
            ),
            (220, session_height)
        );
        assert!(
            measure(&plots).is_empty(),
            "no resize feedback loop after accepting measurement"
        );

        // The measuring layer is invisible/inert; real menu buttons retain
        // their layout and action routing inside the resized surface.
        let mut element = plots.windows.popups[&id].view(&plots);
        let mut tree = Tree::new(&element);
        let size = iced::Size::new(220.0, session_height as f32);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, size),
        );
        let root = iced::advanced::Layout::new(&node);
        let card = root.children().next().unwrap().children().next().unwrap();
        let menu = card.children().nth(2).unwrap();
        let mut clicked = Vec::new();
        for row in menu.children() {
            assert!(
                row.bounds().y + row.bounds().height <= size.height,
                "clipped menu row"
            );
            let cursor = iced::mouse::Cursor::Available(row.bounds().center());
            for event in [
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
            ] {
                element.as_widget_mut().update(
                    &mut tree,
                    &Event::Mouse(event),
                    root,
                    cursor,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut iced::advanced::Shell::new(&mut clicked),
                    &iced::Rectangle::with_size(size),
                );
            }
        }
        let keys: Vec<_> = clicked
            .iter()
            .filter_map(|message| match message {
                Plant::TopPlot(TopEvent::Widget(WidgetEvent::PopupSelect(target, key)))
                    if *target == id =>
                {
                    Some(key.key.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(keys, ["suspend", "hibernate", "reboot", "poweroff"]);

        // Fixed descendants remain authoritative; measurement may grow beyond
        // the old native surface and then shrink when content disappears.
        for (content_height, expected) in [(600.0, 600), (30.0, 30)] {
            let popup = plots.windows.popups.get_mut(&id).unwrap();
            popup.tree = Some(WidgetNode::Space {
                width: NodeLength::Fill,
                height: NodeLength::Fixed(content_height),
            });
            popup.sizing_revision += 1;
            let messages = measure(&plots);
            assert_eq!(messages.len(), 1);
            let _ = plots.update(messages[0].clone());
            assert_eq!(plots.windows.popups[&id].h, expected);
            assert!(measure(&plots).is_empty());
        }
        let revision = plots.windows.popups[&id].sizing_revision;
        let _ = plots.update(Plant::TopPlot(TopEvent::Widget(
            WidgetEvent::PopupMeasured(id, revision - 1, iced::Size::new(220, 900)),
        )));
        assert_eq!(plots.windows.popups[&id].h, 30);
        let _ = plots.update(Plant::TopPlot(TopEvent::Widget(
            WidgetEvent::PopupMeasured(id, revision, iced::Size::new(220, 9000)),
        )));
        assert_eq!(plots.windows.popups[&id].h, 1080);
        plots.windows.popups.get_mut(&id).unwrap().auto_height = false;
        assert!(measure(&plots).is_empty());
        let _ = plots.update(Plant::TopPlot(TopEvent::Widget(
            WidgetEvent::PopupMeasured(id, revision, iced::Size::new(220, 54)),
        )));
        assert_eq!(
            plots.windows.popups[&id].h, 1080,
            "explicit height ignores auto measurements"
        );
        lua.globals()
            .set(
                "_popup_shader",
                concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/shaders/gradient.wgsl"),
            )
            .unwrap();
        lua.globals()
            .set(
                "_popup_image",
                lua.create_userdata(crate::ui::image_value::ImageValue(
                    iced::widget::image::Handle::from_rgba(8, 6, vec![255; 8 * 6 * 4]),
                ))
                .unwrap(),
            )
            .unwrap();
        for (source, expected) in [
            ("iced.popup(iced.space():width(90):height(30))", (90, 30)),
            (
                "iced.popup(iced.space():width(90):height(30)):padding({top=2,right=3,bottom=5,left=7}):radius({top_left=12,bottom_right=8})",
                (100, 37),
            ),
            (
                "iced.popup(iced.space():width(90):height(30)):padding(7):align_x('end')",
                (104, 44),
            ),
            (
                "iced.popup(iced.space():width(90):height(30)):width(120):padding(7)",
                (120, 44),
            ),
            (
                "iced.popup(iced.column({iced.space():width('fill'):height(30),iced.space():width(90):height(10)}):spacing(0))",
                (90, 40),
            ),
            (
                "iced.popup(iced.scrollable(iced.space():width(90):height(100)):width('fill'):height(40))",
                (90, 40),
            ),
            (
                "iced.popup(iced.shader(_popup_shader):animated(false))",
                (100, 100),
            ),
            ("iced.popup(iced.image(_popup_image))", (8, 6)),
        ] {
            let content = Popup::parse_popup_content(
                lua.load(format!("return {source}")).eval().unwrap(),
                &Default::default(),
            )
            .unwrap();
            let popup = plots.windows.popups.get_mut(&id).unwrap();
            popup.tree = content.tree;
            popup.auto_width = content.width.is_none();
            popup.auto_height = content.height.is_none();
            if let Some(width) = content.width {
                popup.w = width as u32;
            }
            popup.sizing_revision += 1;
            let messages = measure(&plots);
            assert_eq!(messages.len(), 1);
            let _ = plots.update(messages[0].clone());
            let popup = &plots.windows.popups[&id];
            assert_eq!((popup.w, popup.h), expected, "{source}");
            assert!(measure(&plots).is_empty(), "popup size must settle");
        }
        let popup = plots.windows.popups.get_mut(&id).unwrap();
        popup.tree = Popup::parse_popup_content(lua.load("return iced.popup(iced.text('A long label which must wrap across several lines'):wrap('word')):width(80):padding(5)").eval().unwrap(), &Default::default()).unwrap().tree;
        popup.w = 80;
        popup.sizing_revision += 1;
        let messages = measure(&plots);
        let _ = plots.update(messages[0].clone());
        assert!(
            plots.windows.popups[&id].h > 40,
            "wrapped text must grow vertically"
        );
        plots.windows.popups.remove(&id);
        let _ = plots.update(Plant::TopPlot(TopEvent::Widget(
            WidgetEvent::PopupMeasured(id, revision, iced::Size::new(220, 54)),
        )));
    }

    #[test]
    fn selecting_dynamic_or_changing_variant_does_not_queue_generation_without_apply() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let mut plots = wallpaper_test_plots();
        let mut restored = plots.config.clone();
        restored.theme.name = "dynamic".into();
        let _ = plots.update(Plant::Config(ConfigEvent::ConfigReloaded(restored)));
        assert!(!plots.theme_jobs.dirty);
        assert!(!plots.theme_jobs.running);
        assert_eq!(plots.theme_jobs.sequence, 0);
        let _ = plots.update(Plant::Config(ConfigEvent::Patch(ConfigPatch::ThemeName(
            "gruvbox".into(),
        ))));
        let _ = plots.update(Plant::Config(ConfigEvent::Patch(ConfigPatch::ThemeName(
            "dynamic".into(),
        ))));
        let _ = plots.update(Plant::Config(ConfigEvent::Patch(
            ConfigPatch::ThemeVariant("tonal-spot".into()),
        )));
        assert!(!plots.theme_jobs.dirty);
        assert!(!plots.theme_jobs.running);
        assert_eq!(plots.theme_jobs.sequence, 0);
        plots.config_jobs.invalidate(); // No fixture edits may reach the user's config file.
        let _ = plots.update(Plant::Sprout);
        let _ = plots.update(Plant::Sprout);
        assert!(!plots.theme_jobs.running);
        assert_eq!(plots.theme_jobs.sequence, 0);
    }

    fn theme_test_plots() -> (Plots, iced::window::Id) {
        let mut plots = wallpaper_test_plots();
        let bar = iced::window::Id::unique();
        let placement = crate::config::WidgetPlacement {
            id: "keep-self".into(),
            name: "theme-memory-test".into(),
            file: Some("/not-a-real-theme-memory-test.lua".into()),
            ..Default::default()
        };
        let mut top = Top::new();
        top.bar_index = 0;
        top.local.widgets = vec![vec![placement.clone()]];
        plots.windows.tops.insert(bar, top);
        plots.config.bar = vec![crate::config::TopConfig {
            widgets: vec![crate::config::SlotWidgets::Many(vec![
                crate::config::SlotEntry::Full(placement),
            ])],
            ..Default::default()
        }];
        plots
            .catalog
            .definitions
            .push(crate::config::WidgetDefinition {
                name: "theme-memory-test".into(),
                file: "/not-a-real-theme-memory-test.lua".into(),
                defaults: Default::default(),
                schema: Default::default(),
            });
        let lua = crate::lua::widgets::new_widget_lua().unwrap();
        crate::lua::widgets::load_widget_script(&lua,"theme-test","return {count=41,view=function(self) self.count=self.count+1; return iced.text(theme.primary) end}").unwrap();
        plots.placements.instances.insert("keep-self".into(), lua);
        (plots, bar)
    }

    #[test]
    fn palette_reload_preserves_pending_save_and_widget_self_state() {
        let (mut plots, bar) = theme_test_plots();
        let save_sequence = plots.config_jobs.stage();
        let message = Plant::Config(ConfigEvent::ThemeReloaded);
        assert_eq!(redraw_scope(&message), Scope::All);
        let _ = plots.update(message);
        assert!(plots.config_jobs.should_flush(save_sequence));
        assert_eq!(plots.windows.tops[&bar].local.widgets[0][0].id, "keep-self");
        let app: mlua::Table = plots.placements.instances["keep-self"]
            .named_registry_value("riced.widget.app")
            .unwrap();
        assert_eq!(app.get::<u32>("count").unwrap(), 42);
        let _ = plots.update(Plant::Config(ConfigEvent::ThemeReloaded));
        assert_eq!(app.get::<u32>("count").unwrap(), 43);
    }

    #[test]
    fn theme_only_disk_config_reload_ignores_runtime_ids_and_keeps_widget_instances() {
        let (mut plots, bar) = theme_test_plots();
        let mut next = plots.config.clone();
        next.theme.darkmode = !next.theme.darkmode;
        if let crate::config::SlotWidgets::Many(entries) = &mut next.bar[0].widgets[0]
            && let crate::config::SlotEntry::Full(placement) = &mut entries[0]
        {
            placement.id.clear();
        }
        let _ = plots.update(Plant::Config(ConfigEvent::ConfigReloaded(next)));
        assert_eq!(plots.windows.tops[&bar].local.widgets[0][0].id, "keep-self");
        let app: mlua::Table = plots.placements.instances["keep-self"]
            .named_registry_value("riced.widget.app")
            .unwrap();
        assert_eq!(app.get::<u32>("count").unwrap(), 42);
    }

    #[test]
    fn every_settings_close_path_releases_preview_pixels_and_schedules_reclamation() {
        use iced::futures::StreamExt;
        use std::sync::Arc;
        struct Pixels {
            data: Vec<u8>,
            _lifetime: Arc<()>,
        }
        impl AsRef<[u8]> for Pixels {
            fn as_ref(&self) -> &[u8] {
                &self.data
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        for close in ["request", "native", "toggle"] {
            let mut plots = wallpaper_test_plots();
            let id = iced::window::Id::unique();
            let lifetime = Arc::new(());
            let weak_pixels = Arc::downgrade(&lifetime);
            let pixels = bytes::Bytes::from_owner(Pixels {
                data: vec![127; 512 * 256 * 4],
                _lifetime: lifetime,
            });
            let source = Arc::new(crate::wallpaper::raster::Source {
                width: 512,
                height: 256,
                handle: iced::widget::image::Handle::from_rgba(512, 256, pixels.clone()),
                pixels,
                revision: None,
            });
            let weak_source = Arc::downgrade(&source);
            // Canvas handles may outlive the draft until native UI teardown.
            let preview: Element<'static, Plant> =
                crate::ui::widgets::display_map::MapLayer::images(
                    id,
                    vec![(0.0, 0.0, 512.0, 256.0)],
                    vec![crate::config::BackgroundImage {
                        path: "memory-only.png".into(),
                        ..Default::default()
                    }],
                    vec![Some(source.handle.clone())],
                    Default::default(),
                )
                .with_file_sizes(vec![Some((512.0, 256.0))])
                .into();
            let mut draft = crate::wallpaper::draft::Draft::new(&plots.config.background);
            draft.loading = false;
            draft.sources.insert("memory-only.png".into(), source);
            let mut setting = Setting::default();
            setting.wallpaper_draft = Some(draft);
            plots.windows.settings.insert(id, setting);
            plots.windows.ids.insert(id, PlotInfo::Setting);
            let task = match close {
                "request" => plots.update(Plant::Uproot(id)),
                "native" => plots.update(Plant::Wayland(LandEvent::Closed(id))),
                _ => plots.update(Plant::Sprout),
            };
            assert!(
                weak_source.upgrade().is_none(),
                "{close}: draft still owns the source"
            );
            assert!(
                weak_pixels.upgrade().is_some(),
                "renderer view must own its pixels until teardown"
            );
            drop(preview);
            assert!(
                weak_pixels.upgrade().is_none(),
                "{close}: original pixel buffer survived teardown"
            );
            assert!(plots.windows.settings.is_empty());
            let actions = runtime.block_on(
                iced_runtime::task::into_stream(task)
                    .unwrap()
                    .collect::<Vec<_>>(),
            );
            let mut reclaimed = 0;
            let mut native_closes = 0;
            for action in actions {
                match action {
                    Action::Output(
                        message @ Plant::Config(ConfigEvent::ReclaimImageMemory { redraw: true }),
                    ) => {
                        // This Settings draft owned decoded sources: the close
                        // must schedule the full-redraw reclaim.
                        assert_eq!(redraw_scope(&message), Scope::All);
                        let _ = plots.update(message);
                        reclaimed += 1;
                    }
                    Action::Window(WindowAction::Close(target)) => {
                        assert_eq!(target, id);
                        native_closes += 1;
                    }
                    _ => {}
                }
            }
            assert_eq!(
                reclaimed, 1,
                "{close}: cleanup must not depend on another close notification"
            );
            assert_eq!(
                native_closes,
                usize::from(close != "native"),
                "native confirmation must not re-close a dead ID"
            );
        }
    }

    #[test]
    fn reclamation_redraw_is_gated_to_image_owning_closes() {
        use crate::ui::node::{NodeLength, WidgetNode};
        use iced::futures::StreamExt;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let reclaim_scope = |plots: &mut Plots, close: Plant| -> Option<(bool, Scope)> {
            let task = plots.update(close);
            let actions = runtime.block_on(
                iced_runtime::task::into_stream(task)
                    .unwrap()
                    .collect::<Vec<_>>(),
            );
            actions.into_iter().find_map(|action| match action {
                Action::Output(Plant::Config(ConfigEvent::ReclaimImageMemory { redraw })) => {
                    let message = Plant::Config(ConfigEvent::ReclaimImageMemory { redraw });
                    Some((redraw, redraw_scope(&message)))
                }
                _ => None,
            })
        };
        let popup = |tree: Option<WidgetNode>| Popup {
            win_id: iced::window::Id::unique(),
            bar_id: iced::window::Id::unique(),
            placement: "clock".into(),
            auto_width: false,
            max_width: 1920.0,
            tree,
            hovered: Default::default(),
            size: 13.0,
            w: 100,
            h: 100,
            auto_height: false,
            max_height: 1080.0,
            sizing_revision: 0,
            measured_revision: None,
            positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                iced::window::Id::unique(),
                iced_exwlshell::reexport::PixelSize::try_px(100, 100).unwrap(),
                (0, 0),
            ),
            on_open: None,
            on_close: None,
            opening: None,
            closing: None,
            phase: 0,
        };
        let output = OutputId(7);

        // Popup without image nodes: host-only cleanup, no full redraw.
        let mut plots = wallpaper_test_plots();
        let id = iced::window::Id::unique();
        plots.windows.popups.insert(id, popup(None));
        plots.windows.ids.insert(id, PlotInfo::Popup(output));
        assert_eq!(
            reclaim_scope(&mut plots, Plant::Uproot(id)),
            Some((false, Scope::None))
        );

        // Popup whose tree draws an image: full-redraw reclaim.
        let id = iced::window::Id::unique();
        plots.windows.popups.insert(
            id,
            popup(Some(WidgetNode::Image {
                path: "memory-only.png".into(),
                width: NodeLength::Shrink,
                height: NodeLength::Shrink,
            })),
        );
        plots.windows.ids.insert(id, PlotInfo::Popup(output));
        assert_eq!(
            reclaim_scope(&mut plots, Plant::Uproot(id)),
            Some((true, Scope::All))
        );

        // Notification surface whose output holds a decoded image.
        let id = iced::window::Id::unique();
        plots.windows.ids.insert(id, PlotInfo::Overlay(output));
        plots.windows.overlays.insert(output, id);
        plots.notification.queue.push_back(Notification {
            generation: 0,
            popup_visible: true,
            closed: false,
            id: 5,
            output: Some(output),
            app: "test".into(),
            title: "image".into(),
            body: String::new(),
            icon: String::new(),
            urgency: 1,
            received_at: Instant::now(),
            timeout: Some(Duration::from_secs(4)),
            actions: Vec::new(),
            image: Some(iced::widget::image::Handle::from_rgba(
                4,
                4,
                bytes::Bytes::from(vec![255; 4 * 4 * 4]),
            )),
        });
        assert_eq!(
            reclaim_scope(&mut plots, Plant::Uproot(id)),
            Some((true, Scope::All))
        );
        assert!(
            plots.notification.holds_images(output),
            "closing a shared overlay must not delete service notifications"
        );

        // A bar close without images stays host-only.
        let id = iced::window::Id::unique();
        plots.windows.ids.insert(id, PlotInfo::Top(output));
        assert_eq!(
            reclaim_scope(&mut plots, Plant::Uproot(id)),
            Some((false, Scope::None))
        );
    }

    #[test]
    fn icon_preview_jobs_are_coalesced_and_closed_windows_release_results() {
        use crate::shell::screens::SettingPage;
        use std::sync::Arc;
        let mut plots = wallpaper_test_plots();
        let id = iced::window::Id::unique();
        plots.windows.settings.insert(id, Setting::default());
        plots.windows.ids.insert(id, PlotInfo::Setting);
        let select = || Plant::SettingPlot(SettingEvent::Select(id, SettingPage::Theme));
        // Dropping the unpolled stream avoids real filesystem work in this lifecycle test.
        assert!(iced_runtime::task::into_stream(plots.update(select())).is_some());
        assert!(iced_runtime::task::into_stream(plots.update(select())).is_none());
        let owner: Arc<[u8]> = vec![127; 4 * 4 * 4].into();
        let weak = Arc::downgrade(&owner);
        let handle = iced::widget::image::Handle::from_rgba(4, 4, bytes::Bytes::from_owner(owner));
        let message = Plant::SettingPlot(SettingEvent::IconPreviewsReady {
            id,
            result: Ok(vec![("Test".into(), vec![Some(handle)])]),
        });
        assert_eq!(redraw_scope(&message), Scope::Window(id));
        let _ = plots.update(message);
        assert!(weak.upgrade().is_some());
        assert!(iced_runtime::task::into_stream(plots.update(select())).is_none());
        let _ = plots.update(Plant::Uproot(id));
        assert!(weak.upgrade().is_none());

        let owner: Arc<[u8]> = vec![127; 4 * 4 * 4].into();
        let weak = Arc::downgrade(&owner);
        let handle = iced::widget::image::Handle::from_rgba(4, 4, bytes::Bytes::from_owner(owner));
        let _ = plots.update(Plant::SettingPlot(SettingEvent::IconPreviewsReady {
            id,
            result: Ok(vec![("Late".into(), vec![Some(handle)])]),
        }));
        assert!(weak.upgrade().is_none());
        assert!(!plots.windows.settings.contains_key(&id));
    }

    #[test]
    fn late_and_stale_preview_results_release_source_pixels() {
        use std::sync::Arc;
        for closed in [false, true] {
            let mut plots = wallpaper_test_plots();
            let id = iced::window::Id::unique();
            let mut setting = Setting::default();
            let mut draft = crate::wallpaper::draft::Draft::new(&plots.config.background);
            draft.assets_revision = 2;
            draft.preview_running = Some(1);
            draft.loading = false;
            setting.wallpaper_draft = Some(draft);
            plots.windows.settings.insert(id, setting);
            plots.windows.ids.insert(id, PlotInfo::Setting);
            if closed {
                let _ = plots.update(Plant::Uproot(id));
            }
            let owner: Arc<[u8]> = vec![127; 512 * 256 * 4].into();
            let weak_pixels = Arc::downgrade(&owner);
            let pixels = bytes::Bytes::from_owner(owner);
            let source = Arc::new(crate::wallpaper::raster::Source {
                width: 512,
                height: 256,
                handle: iced::widget::image::Handle::from_rgba(512, 256, pixels.clone()),
                pixels,
                revision: None,
            });
            let weak_source = Arc::downgrade(&source);
            let sources = [("memory-only.png".into(), source)].into_iter().collect();
            let _ = plots.update(Plant::SettingPlot(SettingEvent::WallpaperPreviewReady {
                id,
                revision: 1,
                result: Ok(sources),
            }));
            assert!(
                weak_source.upgrade().is_none(),
                "closed={closed}: source retained"
            );
            assert!(
                weak_pixels.upgrade().is_none(),
                "closed={closed}: pixels retained"
            );
            if !closed {
                let draft = plots.windows.settings[&id]
                    .wallpaper_draft
                    .as_ref()
                    .unwrap();
                assert!(draft.sources.is_empty());
                assert!(draft.preview_running.is_none());
            }
        }
    }

    #[test]
    fn closing_settings_during_apply_releases_sources_when_the_job_finishes() {
        use std::sync::Arc;
        let mut plots = wallpaper_test_plots();
        let id = iced::window::Id::unique();
        let owner: Arc<[u8]> = vec![127; 512 * 256 * 4].into();
        let weak_pixels = Arc::downgrade(&owner);
        let pixels = bytes::Bytes::from_owner(owner);
        let source = Arc::new(crate::wallpaper::raster::Source {
            width: 512,
            height: 256,
            handle: iced::widget::image::Handle::from_rgba(512, 256, pixels.clone()),
            pixels,
            revision: None,
        });
        let sources: crate::wallpaper::raster::Sources =
            [("memory-only.png".into(), source)].into_iter().collect();
        let mut draft = crate::wallpaper::draft::Draft::new(&plots.config.background);
        draft.sources = sources.clone();
        draft.loading = false;
        draft.applying = true;
        let mut setting = Setting::default();
        setting.wallpaper_draft = Some(draft);
        plots.windows.settings.insert(id, setting);
        plots.windows.ids.insert(id, PlotInfo::Setting);
        plots.desktop.wallpaper.running = Some(1);
        plots.desktop.wallpaper.pending = Some(crate::wallpaper::Request {
            generation: 1,
            editor: Some((id, 0)),
            images: plots.config.background.clone(),
            base: plots.config.background.clone(),
            outputs: Vec::new(),
            sources,
        });
        let _ = plots.update(Plant::Uproot(id));
        assert!(plots.windows.settings.is_empty());
        assert!(
            weak_pixels.upgrade().is_some(),
            "Apply still owns its accepted snapshot"
        );
        // Failure finishes the accepted job without publishing files/config.
        let _ = plots.wallpaper_baked(1, Err("test job finished".into()));
        assert!(plots.desktop.wallpaper.pending.is_none());
        assert!(plots.desktop.wallpaper.running.is_none());
        assert!(
            weak_pixels.upgrade().is_none(),
            "finished Apply retained the source buffer"
        );
    }

    #[test]
    fn confirmed_native_close_releases_editor_sources_even_without_uproot() {
        let mut plots = wallpaper_test_plots();
        let id = iced::window::Id::unique();
        let pixels = bytes::Bytes::from(vec![127; 512 * 256 * 4]);
        let source = std::sync::Arc::new(crate::wallpaper::raster::Source {
            width: 512,
            height: 256,
            handle: iced::widget::image::Handle::from_rgba(512, 256, pixels.clone()),
            pixels,
            revision: None,
        });
        let weak = std::sync::Arc::downgrade(&source);
        let mut draft = crate::wallpaper::draft::Draft::new(&Default::default());
        draft.loading = false;
        draft
            .sources
            .insert(std::path::PathBuf::from("memory-only.png"), source);
        let mut setting = Setting::default();
        setting.wallpaper_draft = Some(draft);
        plots.windows.settings.insert(id, setting);
        plots.windows.ids.insert(id, PlotInfo::Setting);
        assert!(weak.upgrade().is_some());
        let _ = plots.update(Plant::Wayland(LandEvent::Closed(id)));
        assert!(weak.upgrade().is_none());
        assert!(!plots.windows.settings.contains_key(&id));
        let _ = plots.update(Plant::Wayland(LandEvent::Closed(id)));
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn wallpaper_edits_are_preview_only_and_closing_editor_does_not_apply_them() {
        let mut plots = wallpaper_test_plots();
        plots.config.background.image = vec![crate::config::BackgroundImage {
            path: "original.png".into(),
            ..Default::default()
        }];
        let id = iced::window::Id::unique();
        let mut setting = Setting::default();
        let mut draft = crate::wallpaper::draft::Draft::new(&plots.config.background);
        draft.loading = false;
        setting.wallpaper_draft = Some(draft);
        plots.windows.settings.insert(id, setting);
        plots.windows.ids.insert(id, PlotInfo::Setting);
        let message = Plant::SettingPlot(SettingEvent::WallpaperPatch(
            id,
            ConfigPatch::MoveImage {
                index: 0,
                x: 100.0,
                y: -50.0,
            },
        ));
        assert_eq!(redraw_scope(&message), Scope::Window(id));
        let _ = plots.update(message);
        assert_eq!(plots.config.background.image[0].x, 0.0);
        assert_eq!(
            plots.windows.settings[&id]
                .wallpaper_draft
                .as_ref()
                .unwrap()
                .recipe
                .image[0]
                .x,
            100.0
        );
        assert!(!plots.config_jobs.dirty);
        assert!(!plots.theme_jobs.dirty);
        assert!(plots.desktop.wallpaper.pending.is_none());
        let _ = plots.update(Plant::Uproot(id));
        assert_eq!(plots.config.background.image[0].x, 0.0);
        assert!(plots.windows.settings.is_empty());
    }

    #[test]
    fn stale_wallpaper_result_never_publishes_and_latest_request_is_serialized() {
        let mut plots = wallpaper_test_plots();
        let request = crate::wallpaper::Request {
            generation: 9,
            editor: None,
            images: Default::default(),
            base: Default::default(),
            outputs: vec![],
            sources: Default::default(),
        };
        plots.desktop.wallpaper.pending = Some(request);
        plots.desktop.wallpaper.running = Some(8);
        let _ = plots.wallpaper_baked(
            8,
            Ok(crate::wallpaper::Batch {
                tiles: Default::default(),
            }),
        );
        assert!(plots.desktop.wallpaper.applied.is_empty());
        assert_eq!(plots.desktop.wallpaper.running, Some(9));
        assert_eq!(
            plots.desktop.wallpaper.pending.as_ref().unwrap().generation,
            9
        );
    }

    #[test]
    fn compositor_bar_close_releases_children_caches_and_motion_without_deleting_saved_bar() {
        use crate::ui::{
            listview::{Axis, ListView},
            node::WidgetNode,
        };
        let (_, receiver) = iced_wayland_subscriber::shell::channel();
        let mut plots = Plots {
            windows: Default::default(),
            shell_events: receiver,
            input: Default::default(),
            desktop: Default::default(),
            config: Config::default(),
            config_mtime: None,
            catalog: super::super::catalog::WidgetCatalog::new(Vec::new()),
            placements: Default::default(),
            animation: Default::default(),
            services: crate::services::state::ServiceState::new(),
            config_jobs: Default::default(),
            theme_mtime: None,
            theme_jobs: Default::default(),
            notification: Default::default(),
        };
        plots.config.bar.push(crate::config::TopConfig::default());
        let bar = iced::window::Id::unique();
        let popup = iced::window::Id::unique();
        let other = iced::window::Id::unique();
        plots.windows.ids.insert(bar, PlotInfo::Top(OutputId(1)));
        plots
            .windows
            .ids
            .insert(popup, PlotInfo::Popup(OutputId(1)));
        plots.windows.ids.insert(other, PlotInfo::Top(OutputId(2)));
        plots.windows.tops.insert(bar, Top::new());
        plots.windows.tops.insert(other, Top::new());
        plots.windows.popups.insert(
            popup,
            Popup {
                win_id: popup,
                bar_id: bar,
                placement: "clock".into(),
                auto_width: false,
                max_width: 1920.0,
                tree: None,
                hovered: Default::default(),
                size: 13.0,
                w: 100,
                h: 100,
                auto_height: false,
                max_height: 1080.0,
                sizing_revision: 0,
                measured_revision: None,
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    iced::window::Id::unique(),
                    iced_exwlshell::reexport::PixelSize::try_px(100, 100).unwrap(),
                    (0, 0),
                ),
                on_open: None,
                on_close: None,
                opening: None,
                closing: None,
                phase: 0,
            },
        );
        for id in [bar, popup, other] {
            plots.input.cursors.insert(id, Point::new(10.0, 20.0));
            plots.input.presses.insert(id, (0, None, Instant::now()));
            plots
                .placements
                .outputs
                .insert((id, "clock".into()), "tick".into());
            plots
                .placements
                .trees
                .insert((id, "clock".into()), WidgetNode::Spinner);
        }
        for scope in [format!("{bar:?}/clock"), format!("popup:{popup:?}/clock")] {
            let mut list = ListView::new(32.0);
            list.update(
                &mut plots.animation.motion,
                Duration::from_secs(1),
                Axis::Vertical,
                &[],
                &[(scope.clone(), "item".into())],
                &[],
            );
            plots.animation.widgets.insert(scope, list);
        }
        assert!(plots.animation.motion.motion_count() > 0);
        let _ = plots.update(Plant::Uproot(bar));
        assert!(!plots.windows.ids.contains_key(&bar));
        assert!(!plots.windows.ids.contains_key(&popup));
        assert!(plots.windows.popups.is_empty());
        assert!(plots.animation.widgets.is_empty());
        assert_eq!(plots.animation.motion.motion_count(), 0);
        for id in [bar, popup] {
            assert!(!plots.input.cursors.contains_key(&id));
            assert!(!plots.input.presses.contains_key(&id));
            assert!(
                !plots
                    .placements
                    .outputs
                    .keys()
                    .any(|(owner, _)| *owner == id)
            );
            assert!(!plots.placements.trees.keys().any(|(owner, _)| *owner == id));
        }
        assert_eq!(plots.config.bar.len(), 1);
        assert!(plots.windows.tops.contains_key(&other));
        assert!(plots.input.cursors.contains_key(&other));
        assert_eq!(plots.placements.outputs.len(), 1);
        let _ = plots.update(Plant::Uproot(bar));
        assert!(plots.windows.tops.contains_key(&other));
    }

    #[test]
    fn wayland_delivery_refreshes_long_interval_widgets_without_advancing_their_clock() {
        use crate::config::{WidgetDefinition, WidgetPlacement};
        use iced::futures::{StreamExt, executor::block_on};
        let dir =
            std::env::temp_dir().join(format!("riced-wayland-delivery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("workspaces.lua");
        std::fs::write(
            &script,
            r#"
            return { count=0, view=function(self)
                self.count = self.count + 1
                self.list = self.list or wayland.workspaces
                self.windows = self.windows or wayland.toplevels
                return self.count .. ':' .. #self.list .. ':' .. #self.windows
            end, popup=function(self)
                self.popup_seen = #self.list
                return iced.popup(iced.text(self.popup_seen)):width(120)
            end }
        "#,
        )
        .unwrap();
        let mut plots = wallpaper_test_plots();
        plots.services.workspaces = Default::default();
        plots.services.toplevels = Default::default();
        plots.services.wayland_changes = Default::default();
        plots.config.notifications.enabled = false;
        let bar = iced::window::Id::unique();
        let mut top = Top::new();
        top.local.widgets = vec![vec![WidgetPlacement {
            id: "ws".into(),
            name: "workspaces".into(),
            file: Some(script.to_string_lossy().into_owned()),
            interval: Some(60.0),
            ..Default::default()
        }]];
        plots.windows.tops.insert(bar, top);
        plots.catalog.definitions.push(WidgetDefinition {
            name: "workspaces".into(),
            file: script,
            defaults: Default::default(),
            schema: Default::default(),
        });
        Top::render_bar_widgets(&mut plots, bar);
        let popup = iced::window::Id::unique();
        plots.windows.popups.insert(
            popup,
            Popup {
                win_id: popup,
                bar_id: bar,
                placement: "ws".into(),
                tree: None,
                hovered: Default::default(),
                size: 13.0,
                w: 120,
                h: 100,
                auto_width: false,
                max_width: 1920.0,
                auto_height: false,
                max_height: 1080.0,
                sizing_revision: 0,
                measured_revision: None,
                positioner: iced_exwlshell::actions::IcedNewPopupSettings::at_position(
                    bar,
                    iced_exwlshell::reexport::PixelSize::try_px(120, 100).unwrap(),
                    (0, 0),
                ),
                on_open: None,
                on_close: None,
                opening: None,
                closing: None,
                phase: 0,
            },
        );
        let deadline = plots.placements.last_run["ws"];
        let sampled = plots.services.sampled();
        assert_eq!(plots.placements.outputs[&(bar, "ws".into())], "1:0:0");
        plots.services.workspaces =
            crate::services::WorkspaceCache::from_rows(vec![crate::services::Workspace {
                name: "code".into(),
                active: true,
                ..Default::default()
            }]);
        plots.services.toplevels =
            crate::services::ToplevelCache::from_rows(vec![crate::services::Toplevel {
                app_id: "kitty".into(),
                title: "shell".into(),
            }]);
        let stream = Plots::wayland_stream(&plots.services.wayland_changes);
        iced::futures::pin_mut!(stream);
        plots.services.wayland_changes.notify();
        let event = block_on(stream.next()).unwrap();
        assert_eq!(redraw_scope(&event), Scope::All);
        let _ = plots.update(event);
        assert_eq!(plots.placements.outputs[&(bar, "ws".into())], "2:1:1");
        assert_eq!(plots.placements.last_run["ws"], deadline);
        assert_eq!(plots.services.sampled(), sampled);
        let app: mlua::Table = plots.placements.instances["ws"]
            .named_registry_value("riced.widget.app")
            .unwrap();
        assert_eq!(app.get::<usize>("popup_seen").unwrap(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cell_hover_republishes_and_rerenders_only_the_entered_placement() {
        use crate::config::{WidgetDefinition, WidgetPlacement};
        let dir = std::env::temp_dir().join(format!("riced-cell-hover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hover.lua");
        std::fs::write(
            &script,
            "return {view=function(self) return self.hovered and 'hot' or 'cold' end}",
        )
        .unwrap();
        let placement = |id: &str| WidgetPlacement {
            id: id.into(),
            name: "hovercell".into(),
            file: Some(script.to_string_lossy().into()),
            ..Default::default()
        };
        let mut plots = wallpaper_test_plots();
        let bar = iced::window::Id::unique();
        let mut top = Top::new();
        top.local.widgets = vec![vec![placement("whover"), placement("whover2")]];
        plots.windows.tops.insert(bar, top);
        plots.catalog.definitions.push(WidgetDefinition {
            name: "hovercell".into(),
            file: script.clone(),
            defaults: Default::default(),
            schema: Default::default(),
        });
        let text = |plots: &Plots, id: &str| {
            plots
                .placements
                .outputs
                .get(&(bar, id.to_string()))
                .cloned()
        };
        // Stale placement: ignored without touching Lua or the map.
        let _ = Top::handle_cell_hover(&mut plots, "gone".into(), true);
        assert!(plots.placements.hovered.is_empty());
        assert!(plots.placements.outputs.is_empty());
        let _ = Top::handle_cell_hover(&mut plots, "whover".into(), true);
        assert_eq!(plots.placements.hovered.get("whover"), Some(&true));
        assert_eq!(text(&plots, "whover").as_deref(), Some("hot"));
        assert_eq!(text(&plots, "whover2"), None);
        // Entering the sibling clears the first (single pointer) and
        // re-renders both; repeats are no-ops.
        let _ = Top::handle_cell_hover(&mut plots, "whover2".into(), true);
        assert!(!plots.placements.hovered.contains_key("whover"));
        assert_eq!(plots.placements.hovered.get("whover2"), Some(&true));
        assert_eq!(text(&plots, "whover").as_deref(), Some("cold"));
        assert_eq!(text(&plots, "whover2").as_deref(), Some("hot"));
        let _ = Top::handle_cell_hover(&mut plots, "whover2".into(), true);
        assert_eq!(plots.placements.hovered.len(), 1);
        let _ = Top::handle_cell_hover(&mut plots, "whover2".into(), false);
        assert!(plots.placements.hovered.is_empty());
        assert_eq!(text(&plots, "whover2").as_deref(), Some("cold"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pointer_feedback_redraws_only_its_window() {
        let id = iced::window::Id::unique();
        for event in [
            iced::mouse::Event::CursorMoved {
                position: Point::new(1.0, 2.0),
            },
            iced::mouse::Event::CursorEntered,
            iced::mouse::Event::CursorLeft,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
        ] {
            assert_eq!(
                redraw_scope(&Plant::Graft(id, Event::Mouse(event))),
                Scope::Window(id)
            );
        }
        assert_eq!(
            redraw_scope(&Plant::Graft(
                id,
                Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left
                ))
            )),
            Scope::All
        );
    }

    #[test]
    fn captured_button_presses_still_schedule_visual_feedback() {
        let id = iced::window::Id::unique();
        let message = throttled_graft(
            Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::event::Status::Captured,
            id,
        )
        .expect("press event");
        assert_eq!(redraw_scope(&message), Scope::Window(id));
    }
}
