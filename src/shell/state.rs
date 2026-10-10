use iced::futures::Stream;
use iced::widget::Space;
use iced::widget::image::Handle;
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
    if let Event::Mouse(iced::mouse::Event::CursorMoved { .. }) = &event {
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
        self.input.cursors.remove(&id);
        self.input.presses.remove(&id);
        self.placements.forget_window(id);
        self.animation.forget_window(id);
        self.windows.forget(id)
    }

    pub fn new(shell_events: ShellReceiver) -> Self {
        let (config, config_mtime) = Config::load();
        crate::theme::ensure_user_themes();
        crate::theme_gen::ensure_user_templates();
        let theme_mtime = crate::theme::poll(&config.theme, &None).unwrap_or(None);
        crate::theme::sync(&config.theme);
        let mut desktop = super::desktop::DesktopState::default();
        desktop.sync_wallpapers(&config);
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

    /// Pre-warmed [`Handle`] for an image entry, or `None` when its file
    /// failed to decode (caller skips the entry, same as before).
    pub(crate) fn wallpaper_handle(&self, img: &crate::config::BackgroundImage) -> Option<Handle> {
        self.desktop.wallpapers.get(&img.local_path()).cloned()
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

    /// Arm a regen `REGEN_DELAY` out (drop / add / remove / scale / file
    /// edit paths). Per-move patches never call this — drags stay silent
    /// until drop or close. Supersedes pending timers via `seq`; no-ops
    /// while a run is in flight (completion re-arms if still dirty).
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

    /// Fire a regen now (last Settings panel just closed — the user is
    /// done editing). Batch-safe: `none` unless dirty, idle, and wanted.
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
        let variant = self.config.theme.variant.clone();
        let darkmode = self.config.theme.darkmode;
        let templates = crate::theme_gen::effective_templates_dir(&self.config.theme);

        self.theme_jobs.dirty = false;
        self.theme_jobs.running = true;
        self.theme_jobs.sequence += 1;
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let views = crate::theme_gen::render_views(&rects, &images);
                    let errors =
                        match crate::theme_gen::generate_from_views(&views, &variant, darkmode) {
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
                        };
                    // Views + quantization buffers peak in the tens of MB
                    // and are all dropped here; without a trim glibc
                    // arenas retain the high-water RSS forever on this
                    // pooled worker thread.
                    crate::theme_gen::trim_memory();
                    errors
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
            iced::event::listen_with(throttled_graft),
            shell_sub,
            // XDG base windows (Settings) report close so `ids` stays in sync.
            iced::window::close_events().map(Plant::Uproot),
            // hot-reload poll for config.toml (mtime check only, cheap)
            iced::time::every(Duration::from_millis(500))
                .map(|_| Plant::Config(ConfigEvent::ConfigTick)),
            // file IPC poll for CLI requests (`riced open-settings`)
            iced::time::every(Duration::from_millis(250)).map(|_| Plant::IpcPoll),
        ];

        // List enter/exit transitions tick at 60fps only while a
        // motion is active (aura runtime idles to zero wakeups).
        if self.animation.motion.has_active() {
            subs.push(
                iced::time::every(Duration::from_millis(16))
                    .map(|_| Plant::TopPlot(TopEvent::Widget(WidgetEvent::Anim))),
            );
        }

        // Notification expiry sweep, gated on a non-empty queue like
        // Widget(WidgetEvent::Tick) (repaint on expiry comes from
        // Scope::All below).
        if !self.notification.queue.is_empty() {
            subs.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Plant::Notify(NotifyEvent::Tick)),
            );
        }

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

    /// D-Bus server stream builder (bare fn pointer for
    /// `Subscription::run_with` identity; boxed so no input lifetime
    /// leaks into the higher-ranked signature).
    fn notif_stream(default_ms: &u64) -> std::pin::Pin<Box<dyn Stream<Item = Plant> + Send>> {
        let ms = *default_ms;
        Box::pin(iced::stream::channel(16, async move |tx| {
            crate::notify::serve(tx, ms).await;
        }))
    }

    pub fn title(&self, id: iced::window::Id) -> Option<String> {
        match self.id_info(id) {
            Some(PlotInfo::Setting) => Some(Setting::title()),
            _ => None,
        }
    }

    pub fn view(&self, id: iced::window::Id) -> Element<'_, Plant> {
        // Background windows render their own selection/context overlays clipped
        // Top windows render the bar. Daemon's tiny 1x1 window: empty.
        // Settings (XDG toplevel) renders its own panel (see layers/setting.rs).
        if let Some(setting) = self.windows.settings.get(&id) {
            return setting.view(id, self);
        }
        match self.id_info(id) {
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
                    )
                })
                .unwrap_or_else(|| Space::new().into()),
            Some(PlotInfo::Popup(_output)) => self
                .windows
                .popups
                .get(&id)
                .map(|p| p.view(self))
                .unwrap_or_else(|| Space::new().into()),
            Some(PlotInfo::Notification(output)) => super::screens::notification::view(
                super::screens::notification::ViewContext {
                    config: &self.config.notifications,
                    state: &self.notification,
                    motion: &self.animation.motion,
                },
                output,
            ),
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

    pub fn update(&mut self, message: Plant) -> Command<Plant> {
        match message {
            Plant::Uproot(id) => {
                let info = self.windows.ids.get(&id).copied();
                let mut commands = Vec::new();
                if matches!(info, Some(PlotInfo::Top(_))) {
                    let children: Vec<_> = self
                        .windows
                        .popups
                        .iter()
                        .filter_map(|(popup_id, popup)| (popup.bar_id == id).then_some(*popup_id))
                        .collect();
                    for child in children {
                        commands.push(Popup::handle_dismiss(self, child));
                    }
                }
                if let Some(PlotInfo::Notification(output)) = info
                    && self.windows.notifications.get(&output) == Some(&id)
                {
                    commands.push(Notification::remove_for_output(self, output));
                }
                self.forget_window(id);
                let closed_last_panel =
                    matches!(info, Some(PlotInfo::Setting)) && self.windows.settings.is_empty();
                // Idempotent close: covers both the in-window close button
                // and the compositor's X button (via close_events -> Uproot).
                // Any window close also flushes staged config edits, so a
                // drag-then-close without an idle gap still persists.
                self.flush_config_save();
                commands.push(iced_runtime::task::effect(Action::Window(
                    WindowAction::Close(id),
                )));
                // Panel edits are done when the last panel closes: regen
                // immediately instead of waiting out the countdown.
                if closed_last_panel && self.theme_jobs.dirty {
                    commands.push(self.fire_regen_theme());
                }
                Command::batch(commands)
            }
            Plant::Tend => Command::none(),
            Plant::Sprout => {
                // Toggle: spawn when none open, else close the current panel.
                Setting::handle_toggle(self)
            }
            Plant::IpcPoll => {
                // CLI client queued a request (`riced open-settings`).
                match crate::cli::take_queued_command() {
                    Some(crate::cli::QueuedCommand::OpenSettings) => Setting::handle_toggle(self),
                    None => Command::none(),
                }
            }
            Plant::Wayland(LandEvent::OutputAdded(output)) => {
                let output_id = OutputId::from(&output);
                // store geometry for panel.screen clipping
                self.windows.output_infos.insert(output_id, output.clone());
                let mut cmds = Vec::new();
                // Warm passive notification surfaces before any arrival. An
                // arrival then updates an existing renderer/layer instead of
                // mapping a new surface over the focused fullscreen app.
                if self.config.notifications.enabled {
                    let config = self.config.notifications.clone();
                    if let Some(command) = Notification::ensure_window(self, output_id, &config) {
                        cmds.push(command);
                    }
                }
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
                if geom_changed {
                    // `%` bar sizes resolve against this geometry: re-push
                    // every bar's px size so nothing goes stale, plus heals.
                    // Notification windows span the output height, so they
                    // re-push too.
                    Command::batch(vec![
                        Self::repaint_burst(),
                        Top::reapply_for_output(self, output_id),
                        Notification::reapply_for_output(self, output_id),
                    ])
                } else {
                    Command::none()
                }
            }
            Plant::Wayland(LandEvent::OutputRemoved(output)) => {
                let output_id = OutputId::from(&output);
                let surfaces = self.windows.on_output(output_id);
                // Retire notifications while their window binding still exists.
                let mut cmds = vec![Notification::remove_for_output(self, output_id)];
                for id in surfaces {
                    if self.forget_window(id).is_some() {
                        cmds.push(iced_runtime::task::effect(Action::Window(
                            WindowAction::Close(id),
                        )));
                    }
                }
                self.windows.output_infos.remove(&output_id);
                // clear global selection if it was on removed output (will hide via intersect check)
                if cmds.is_empty() {
                    Command::none()
                } else {
                    Command::batch(cmds)
                }
            }
            Plant::Wayland(LandEvent::NewShell(info)) => match self.id_info(info.window) {
                Some(PlotInfo::Background(_)) => Self::repaint_burst(),
                Some(PlotInfo::Notification(output)) => {
                    // Fresh native surface: force the mask reinstall and
                    // assert the passive keyboard policy exactly once. Later
                    // reconciles only push when the card rects actually move.
                    self.notification.masks.remove(&output);
                    Command::batch(vec![
                        Command::done(Plant::KeyboardInteractivityChange {
                            id: info.window,
                            keyboard_interactivity:
                                iced_exwlshell::reexport::KeyboardInteractivity::None,
                        }),
                        Notification::push_input_region(self, output),
                    ])
                }
                _ => Command::none(),
            },
            Plant::Wayland(LandEvent::Closed(_)) => Command::none(),
            Plant::Wayland(LandEvent::WindowOutputChanged { .. }) => Command::none(),
            Plant::Wayland(LandEvent::Locked) => Command::none(),
            Plant::Wayland(LandEvent::LockDenied) => Command::none(),
            Plant::Wayland(LandEvent::LockedFinished) => Command::none(),
            Plant::Graft(id, event) => {
                if matches!(
                    event,
                    Event::Mouse(iced::mouse::Event::ButtonReleased(
                        iced::mouse::Button::Left
                    ))
                ) {
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
                // Theme files hot-reload on the same tick: re-emit the live
                // config so every view repaints with the new palette. The
                // actual re-parse happens in `theme::sync` on next view.
                if let Some(mtime) = crate::theme::poll(&self.config.theme, &self.theme_mtime) {
                    self.theme_mtime = mtime;
                    return Command::done(Plant::Config(ConfigEvent::ConfigReloaded(
                        self.config.clone(),
                    )));
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
                    self.notification.renderer.lua = None;
                    return Command::batch(vec![
                        Command::done(Plant::Config(ConfigEvent::WidgetsReloaded(
                            self.catalog.definitions.clone(),
                        ))),
                        self.notify_parse_error(),
                    ]);
                }
                self.notify_parse_error()
            }
            Plant::Config(ConfigEvent::ConfigReloaded(cfg)) => {
                // The file changed under us (external edit, or a theme file
                // hot-reload re-emit): the on-disk config wins and replaces
                // any staged local edits. Pending save timers are cancelled.
                if self.config_jobs.dirty {
                    println!("riced: config changed on disk — replacing staged local edits");
                }
                self.config_jobs.invalidate();
                let images_changed = cfg.background.image != self.config.background.image;
                let switched_to_dynamic =
                    cfg.theme.name == "dynamic" && self.config.theme.name != "dynamic";
                self.config = cfg;
                self.desktop.sync_wallpapers(&self.config);
                // Bars resolve [[bar]] entries once at spawn; re-resolve
                // here so slot/align/geometry edits apply live (anchor
                // changes still need a respawn). On-disk wins by contract.
                // Placement ids regenerate, so Lua states rebuild below
                // (like WidgetsReloaded) and open popups dismiss.
                let bars = Top::resync_bars_from_config(self);
                let mut cmds = Vec::new();
                for bar_id in bars {
                    cmds.push(Top::apply_layout(self, bar_id));
                }
                Top::init_widget_lua(self);
                for id in self.windows.popups.keys().copied().collect::<Vec<_>>() {
                    cmds.push(Popup::handle_dismiss(self, id));
                }
                if images_changed || switched_to_dynamic {
                    cmds.push(self.arm_regen_theme());
                    return Command::batch(cmds);
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
                // Local-first single source of truth: mutate the live
                // (in-memory) config and redraw All so every subscribed view
                // previews the value next frame. The disk write is staged,
                // not per-tick: slider drags fire dozens of patches a second
                // and must not rewrite config.toml each time. `SaveTimer`
                // persists once idle; panel close flushes synchronously.
                // Image add/remove re-syncs the pre-decoded wallpaper cache;
                // move/scale/z reuse the same pixels.
                let sync = matches!(
                    patch,
                    ConfigPatch::AddImage(_) | ConfigPatch::RemoveImage { .. }
                );
                // Discrete wallpaper edits (add/remove/scale/size/z) arm a regen
                // 2s out. Per-move patches are deliberately excluded: drags
                // stay silent until drop or panel close.
                let wallpaper_touched = matches!(
                    patch,
                    ConfigPatch::AddImage(_)
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
                if sync {
                    self.desktop.sync_wallpapers(&self.config);
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
                if let Some(dir) = retemplate_dir {
                    return Command::batch(vec![save, Self::retemplate_command(dir, &self.config)]);
                }
                if wallpaper_touched {
                    return Command::batch(vec![save, self.arm_regen_theme()]);
                }
                save
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
                    return Command::none();
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
                // The completion itself is the notification (success or
                // failure); edits landing mid-flight chain a follow-up.
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
                // Edits that landed mid-flight re-dirty the flag; chain one
                // follow-up regen instead of dropping them.
                if self.theme_jobs.dirty {
                    return Command::batch(vec![regen_cmd, self.arm_regen_theme()]);
                }
                regen_cmd
            }
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
                WidgetEvent::Changed => Popup::refresh_bodies(self),
                WidgetEvent::PopupSelect(id, action) => Popup::handle_select(self, id, action),
                WidgetEvent::CellAction(bar, placement, action) => {
                    Top::handle_cell_action(self, bar, placement, action)
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
            Plant::Notify(NotifyEvent::Arrived(n)) => Notification::handle_arrived(self, n),
            Plant::Notify(NotifyEvent::Dismissed(id)) => Notification::handle_dismissed(self, id),
            Plant::Notify(NotifyEvent::PeerClosed(id)) => {
                Notification::handle_peer_closed(self, id)
            }
            Plant::Notify(NotifyEvent::Invoke(id, key)) => {
                Notification::handle_invoke(self, id, key)
            }
            Plant::Notify(NotifyEvent::Tick) => Notification::handle_tick(self),
            Plant::Notify(NotifyEvent::Scrolled(output, offset)) => {
                self.notification.scroll.insert(output, offset);
                Notification::push_input_region(self, output)
            }
            Plant::Notify(NotifyEvent::DBusUp) => {
                eprintln!("riced: notifications: D-Bus server up");
                Command::none()
            }
            Plant::SettingPlot(SettingEvent::Select(id, page)) => {
                Setting::handle_select(self, id, page)
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
            Plant::SettingPlot(SettingEvent::MapViewChanged { id, view }) => {
                // Mouse drop ends the drag: the incoming view has drag None,
                // so an image drag in the stored view means wallpapers moved.
                // (Per-move patches never dirty the theme — only the drop
                // arms the 2s regen countdown.)
                let dropped_image = self
                    .windows
                    .settings
                    .get(&id)
                    .is_some_and(|s| s.map_view().drag.is_some_and(|d| d.image.is_some()));
                if let Some(setting) = self.windows.settings.get_mut(&id) {
                    setting.set_map_view(view);
                }
                if dropped_image {
                    return self.arm_regen_theme();
                }
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
        Plant::Notify(NotifyEvent::DBusUp | NotifyEvent::Scrolled(..)) => Scope::None,
        // Widget press only records; every other widget event repaints
        // (animation frames while a motion runs, releases opening a
        // popup, actions refreshing tiles, changed outputs).
        Plant::TopPlot(TopEvent::Widget(WidgetEvent::Pressed(..))) => Scope::None,
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
        | Plant::IpcPoll => Scope::None,
        Plant::Config(ConfigEvent::ConfigReloaded(_))
        | Plant::Config(ConfigEvent::Patch(_))
        | Plant::Config(ConfigEvent::WidgetsReloaded(_)) => Scope::All,
        // Fresh dynamic.json on disk: repaint so the new palette applies
        // (theme::sync picks the new mtime up during the redraw).
        Plant::Config(ConfigEvent::ThemeRegenerated(_)) => Scope::All,
        // Settings page/bar select / map pan-zoom only affects its own window.
        Plant::SettingPlot(
            SettingEvent::Select(id, _)
            | SettingEvent::SelectBar(id, _)
            | SettingEvent::SelectImage(id, _)
            | SettingEvent::SelectSlot(id, _)
            | SettingEvent::SelectPlacement(id, _, _)
            | SettingEvent::MapViewChanged { id, .. },
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
                slot: 0,
                placement: "clock".into(),
                body: String::new(),
                items: Vec::new(),
                tree: None,
                size: 13.0,
                w: 100,
                h: 100,
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
