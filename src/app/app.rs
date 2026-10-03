use iced::widget::Space;
use iced::widget::image::Handle;
use iced::{Element, Event, Point, Task as Command};
use iced_exwlshell::redraw::Scope;
use iced_runtime::Action;
use iced_runtime::window::Action as WindowAction;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use iced_wayland_subscriber::OutputId;
use iced_wayland_subscriber::shell::{ShellEvent, ShellReceiver};

use super::layers::{Background, ContextMenu, Popup, SelectionRect, Setting, Top};
use super::{BackgroundEvent, ConfigEvent, LandEvent, Plant, SettingEvent, TopEvent};
use crate::config::{Config, ConfigPatch};
use iced_wayland_subscriber::OutputInfo;

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
    if matches!(event, Event::Mouse(iced::mouse::Event::ButtonReleased(_))) {
        return Some(Plant::Graft(id, event));
    }
    None
}

#[derive(Debug)]
pub struct Plots {
    pub(crate) ids: HashMap<iced::window::Id, PlotInfo>,
    pub(crate) tops: HashMap<iced::window::Id, Top>,
    pub(crate) popups: HashMap<iced::window::Id, Popup>,
    pub(crate) settings: HashMap<iced::window::Id, Setting>,
    pub(crate) backgrounds: HashMap<OutputId, Background>,
    pub(crate) background_ids: HashMap<OutputId, iced::window::Id>,
    pub(crate) shell_events: ShellReceiver,
    // per-window cursor
    pub(crate) last_cursor: HashMap<iced::window::Id, Point>,
    // global selection rect (single, like Background.selectionRect)
    pub(crate) selection_rect: SelectionRect,
    // fade animation after select end (QML Behavior on opacity, InOutQuad
    // over the global animation speed)
    pub(crate) fade_rect: Option<SelectionRect>,
    pub(crate) fade_start: Option<Instant>,
    // per-output geometry for clipping (panel.screen)
    pub(crate) output_infos: HashMap<OutputId, OutputInfo>,
    // context menu state (global, like Background contextMenu)
    pub(crate) context_menu: Option<ContextMenu>,
    // throttling for smooth 60fps selection updates
    pub(crate) last_selection_tick: Option<Instant>,
    // press start per window for hold detection (Top hold, shared via PanelWindow)
    pub(crate) press_starts: HashMap<iced::window::Id, Instant>,
    // hot-reloaded config + last seen file mtime
    pub(crate) config: Config,
    pub(crate) config_mtime: Option<std::time::SystemTime>,
    // Declarative widgets (`widgets.toml`) + last seen file mtime.
    // Bars reference entries by name; hot-reloaded like the config.
    pub(crate) widgets: Vec<crate::config::WidgetDef>,
    pub(crate) widgets_mtime: Option<std::time::SystemTime>,
    // Lua widget runtimes keyed by def name, last rendered text, last
    // run tick, and last error (errors log only on change, never per
    // tick). States are rebuilt on every widgets.toml hot-reload.
    pub(crate) widget_lua: HashMap<String, mlua::Lua>,
    // Basenames each widget runtime was built with (stale allowlists
    // rebuild on next render via ensure_widget_lua).
    pub(crate) widget_exec_allow: HashMap<String, Vec<String>>,
    pub(crate) widget_outputs: HashMap<String, String>,
    pub(crate) widget_trees: HashMap<String, crate::app::layers::top::WidgetNode>,
    pub(crate) widget_last_run: HashMap<String, Instant>,
    pub(crate) widget_last_error: HashMap<String, String>,
    // Script file mtimes per widget (live-reload on edit).
    pub(crate) widget_script_mtime: HashMap<String, std::time::SystemTime>,
    // Live system snapshot for the `sysinfo` Lua table (CPU + memory,
    // refreshed on every widget tick; usage needs the delta).
    pub(crate) sysinfo: sysinfo::System,
    // Local-first staging: `Patch` mutates live memory every tick (smooth
    // previews, no disk I/O); the file write is coalesced via `SaveTimer`.
    // `dirty` marks unsaved staged edits, `seq` invalidates superseded timers.
    pub(crate) config_dirty: bool,
    pub(crate) config_save_seq: u64,
    // mtime of the active theme file (`theme::poll`); `None` tracks the
    // vendored fallback. A change re-emits the config so every view repaints.
    pub(crate) theme_mtime: Option<std::time::SystemTime>,
    // Pre-decoded wallpaper pixels keyed by resolved path. File-backed
    // handles decode on a worker whose completion redraw the shell drops,
    // leaving first paint blank — serving `from_rgba` instead loads
    // synchronously, so pixels exist on the very first frame. Synced from
    // `config.background.image` on load/reload/patch (below).
    pub(crate) wallpapers: HashMap<PathBuf, Handle>,
    // Generation bumped by every `BackgroundEvent::Repaint` heal so the
    // delayed redraw is a real state transition, not a silent no-op.
    pub(crate) repaint_seq: u64,
    // Dynamic-theme regen: discrete touches (drops, add/remove/scale,
    // file edits) arm a 2s one-shot timer; panel close fires immediately.
    // `seq` invalidates superseded timers, `running` guards overlap.
    pub(crate) theme_regen_dirty: bool,
    pub(crate) theme_regen_running: bool,
    pub(crate) theme_regen_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlotInfo {
    Setting,
    Background(OutputId),
    Top(OutputId),
    Popup(OutputId),
}

impl Plots {
    pub fn new(shell_events: ShellReceiver) -> Self {
        let (config, config_mtime) = Config::load();
        crate::theme::ensure_user_themes();
        crate::colorgen::ensure_user_templates();
        let theme_mtime = crate::theme::poll(&config.theme, &None).unwrap_or(None);
        crate::theme::sync(&config.theme);
        let mut wallpapers = HashMap::new();
        Self::sync_wallpapers(&config, &mut wallpapers);
        let (widgets, widgets_mtime) = crate::config::WidgetsFile::load();
        let mut sysinfo = sysinfo::System::new();
        sysinfo.refresh_cpu_usage();
        sysinfo.refresh_memory();
        let mut plots = Self {
            ids: HashMap::new(),
            tops: HashMap::new(),
            popups: HashMap::new(),
            settings: HashMap::new(),
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
            press_starts: HashMap::new(),
            config,
            config_mtime,
            widgets,
            widgets_mtime,
            config_dirty: false,
            config_save_seq: 0,
            theme_mtime,
            wallpapers,
            repaint_seq: 0,
            theme_regen_dirty: false,
            theme_regen_running: false,
            theme_regen_seq: 0,
            widget_lua: HashMap::new(),
            widget_exec_allow: HashMap::new(),
            widget_outputs: HashMap::new(),
            widget_trees: HashMap::new(),
            widget_last_run: HashMap::new(),
            widget_last_error: HashMap::new(),
            widget_script_mtime: HashMap::new(),
            sysinfo,
        };
        // Render Lua widgets once so bars populate on the first frame
        // instead of waiting out the first tick.
        Top::init_widget_lua(&mut plots);
        plots
    }

    fn sync_wallpapers(config: &Config, wallpapers: &mut HashMap<PathBuf, Handle>) {
        let live: Vec<PathBuf> = config
            .background
            .image
            .iter()
            .map(|img| img.local_path())
            .collect();
        wallpapers.retain(|path, _| live.contains(path));
        let mut missing: Vec<PathBuf> = Vec::new();
        for img in &config.background.image {
            let path = img.local_path();
            if path.as_os_str().is_empty()
                || wallpapers.contains_key(&path)
                || missing.contains(&path)
            {
                continue;
            }
            missing.push(path);
        }
        if missing.is_empty() {
            return;
        }
        std::thread::scope(|s| {
            let jobs: Vec<_> = missing
                .into_iter()
                .map(|path| {
                    s.spawn(move || {
                        let decoded = crate::config::decode_handle(&path);
                        (path, decoded)
                    })
                })
                .collect();
            for job in jobs {
                match job.join() {
                    Ok((path, Some((_w, _h, handle)))) => {
                        wallpapers.insert(path, handle);
                    }
                    Ok((_, None)) => {}
                    Err(_) => eprintln!("riced: wallpaper decode thread failed, skipping"),
                }
            }
        });
    }

    /// Pre-warmed [`Handle`] for an image entry, or `None` when its file
    /// failed to decode (caller skips the entry, same as before).
    pub(crate) fn wallpaper_handle(&self, img: &crate::config::BackgroundImage) -> Option<Handle> {
        self.wallpapers.get(&img.local_path()).cloned()
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
                    let text = match crate::colorgen::stored_theme_text(&name) {
                        Ok(t) => t,
                        Err(e) => return vec![e],
                    };
                    crate::colorgen::render_theme_templates(&dir, &text, darkmode)
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
        self.config_dirty = true;
        self.config_save_seq += 1;
        let save_seq = self.config_save_seq;
        Command::perform(tokio::time::sleep(Self::CONFIG_SAVE_DELAY), move |_| {
            Plant::Config(ConfigEvent::SaveTimer(save_seq))
        })
    }

    /// Persist staged config edits, if any. Idempotent no-op when clean.
    /// Called by the coalescing save timer and synchronously on panel close.
    pub(crate) fn flush_config_save(&mut self) {
        if !self.config_dirty {
            return;
        }
        self.config_dirty = false;
        self.config_save_seq += 1;
        // Update mtime so the poll tick doesn't echo our own write back.
        self.config_mtime = self.config.save().or(self.config_mtime);
    }

    /// Arm a regen `REGEN_DELAY` out (drop / add / remove / scale / file
    /// edit paths). Per-move patches never call this — drags stay silent
    /// until drop or close. Supersedes pending timers via `seq`; no-ops
    /// while a run is in flight (completion re-arms if still dirty).
    pub(crate) fn arm_regen_theme(&mut self) -> Command<Plant> {
        self.theme_regen_dirty = true;
        if self.theme_regen_running {
            return Command::none();
        }
        if !crate::colorgen::wants_regen(&self.config.theme.name) {
            self.theme_regen_dirty = false;
            return Command::none();
        }
        self.theme_regen_seq += 1;
        let seq = self.theme_regen_seq;
        Command::perform(tokio::time::sleep(Self::REGEN_DELAY), move |_| {
            Plant::Config(ConfigEvent::RegenTimer(seq))
        })
    }

    /// Fire a regen now (last Settings panel just closed — the user is
    /// done editing). Batch-safe: `none` unless dirty, idle, and wanted.
    pub(crate) fn fire_regen_theme(&mut self) -> Command<Plant> {
        if !self.theme_regen_dirty || self.theme_regen_running {
            return Command::none();
        }
        if !crate::colorgen::wants_regen(&self.config.theme.name) {
            self.theme_regen_dirty = false;
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
            .backgrounds
            .keys()
            .filter_map(|o| Background::available_rect(*o, &self.output_infos))
            .collect();
        rects.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        if rects.is_empty() {
            rects = self
                .config
                .background
                .image
                .iter()
                .filter_map(crate::components::display_map::MapLayer::resolved)
                .collect();
        }
        let images = self.config.background.image.clone();
        let variant = self.config.theme.variant.clone();
        let darkmode = self.config.theme.darkmode;
        let templates = crate::colorgen::effective_templates_dir(&self.config.theme);

        self.theme_regen_dirty = false;
        self.theme_regen_running = true;
        self.theme_regen_seq += 1;
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let views = crate::colorgen::render_views(&rects, &images);
                    match crate::colorgen::generate_from_views(&views, &variant, darkmode) {
                        Ok(g) => {
                            let mut errors = Vec::new();
                            if let Err(e) = crate::colorgen::write_dynamic_theme(&g.payload) {
                                errors.push(format!("cannot write dynamic.json: {e}"));
                                return errors;
                            }
                            if let Some(dir) = templates {
                                errors
                                    .extend(crate::colorgen::process_templates(&dir, &g.variables));
                            }
                            errors
                        }
                        Err(e) => vec![e],
                    }
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
        self.ids.get(&id).copied()
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

        // Only tick for fade animation (selecting is driven by throttled mouse moves, not timer)
        // QML Behavior InOutQuad on opacity (over the animation speed) needs
        // 60fps ticks only while fading
        if self.fade_start.is_some() {
            subs.push(
                iced::time::every(Duration::from_millis(16))
                    .map(|_| Plant::BackgroundPlot(BackgroundEvent::SelectionTick)),
            );
        }

        // Lua widgets re-render on their own intervals (250ms cadence,
        // each script runs only when due). No timer at all without them.
        if !self.widgets.is_empty() {
            subs.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Plant::TopPlot(TopEvent::WidgetTick)),
            );
        }

        iced::Subscription::batch(subs)
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
        if let Some(setting) = self.settings.get(&id) {
            return setting.view(id, self);
        }
        match self.id_info(id) {
            Some(PlotInfo::Background(output)) => Background::view(self, id, output),
            Some(PlotInfo::Top(_output)) => self
                .tops
                .get(&id)
                .map(|t| t.view(id, &self.widgets, &self.widget_outputs, &self.widget_trees))
                .unwrap_or_else(|| Space::new().into()),
            Some(PlotInfo::Popup(_output)) => self
                .popups
                .get(&id)
                .map(|p| p.view())
                .unwrap_or_else(|| Space::new().into()),
            Some(PlotInfo::Setting) => Space::new().into(), // unreachable: handled above
            None => Space::new().into(),                    // daemon's 1x1 tiny window
        }
    }

    pub fn update(&mut self, message: Plant) -> Command<Plant> {
        match message {
            Plant::Uproot(id) => {
                self.last_cursor.remove(&id);
                self.press_starts.remove(&id);
                let mut closed_last_panel = false;
                if let Some(info) = self.ids.get(&id).copied() {
                    match info {
                        PlotInfo::Top(_) => {
                            self.ids.remove(&id);
                            self.tops.remove(&id);
                        }
                        PlotInfo::Popup(_) => {
                            self.ids.remove(&id);
                            self.popups.remove(&id);
                        }
                        PlotInfo::Background(output) => {
                            self.ids.remove(&id);
                            self.backgrounds.remove(&output);
                            self.background_ids.remove(&output);
                        }
                        PlotInfo::Setting => {
                            Setting::remove(&mut self.settings, &mut self.ids, id);
                            closed_last_panel = self.settings.is_empty();
                        }
                    }
                } else {
                    // Unknown id (e.g. duplicate close event): still drop tracking.
                    Setting::remove(&mut self.settings, &mut self.ids, id);
                }
                // Idempotent close: covers both the in-window close button
                // and the compositor's X button (via close_events -> Uproot).
                // Any window close also flushes staged config edits, so a
                // drag-then-close without an idle gap still persists.
                self.flush_config_save();
                let close = iced_runtime::task::effect(Action::Window(WindowAction::Close(id)));
                // Panel edits are done when the last panel closes: regen
                // immediately instead of waiting out the countdown.
                if closed_last_panel && self.theme_regen_dirty {
                    return Command::batch(vec![close, self.fire_regen_theme()]);
                }
                close
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
                self.output_infos.insert(output_id, output.clone());
                let mut cmds = Vec::new();
                // sentinel Top cleanup (delegated to Top layer)
                for sentinel_id in Top::cleanup_sentinels(&mut self.tops, &mut self.ids) {
                    self.last_cursor.remove(&sentinel_id);
                    self.press_starts.remove(&sentinel_id);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(sentinel_id),
                    )));
                }
                // Background fullscreen per output for selection (delegated to Background layer)
                if let Some(cmd) = Background::ensure_for_output(
                    &mut self.backgrounds,
                    &mut self.background_ids,
                    &mut self.ids,
                    output_id,
                ) {
                    cmds.push(cmd);
                    cmds.push(Self::repaint_burst());
                }
                // Declarative bars: one per `[[bar]]` entry on this
                // output, skipped when that edge already has a bar (user
                // additions and re-added outputs never duplicate).
                // File order is spawn order (deterministic).
                for (index, cfg) in self.config.bar.iter().enumerate() {
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
                            .output_infos
                            .get(&output_id)
                            .and_then(|info| info.name.clone())
                            .unwrap_or_default();
                        if !here.eq_ignore_ascii_case(cfg.output.trim()) {
                            continue;
                        }
                    }
                    let taken = self.ids.iter().any(|(wid, info)| match info {
                        PlotInfo::Top(o) if *o == output_id => {
                            self.tops.get(wid).is_some_and(|t| t.anchor() == anchor)
                        }
                        _ => false,
                    });
                    if taken {
                        continue;
                    }
                    let top = Top::with_config(index, anchor, cfg.into());
                    let (sw, sh) = self
                        .output_infos
                        .get(&output_id)
                        .map(Background::output_geometry)
                        .map(|(_, _, w, h)| (w, h))
                        .unwrap_or((1920.0, 1080.0));
                    let (w, h) = top.local.px_size(sw, sh, top.is_horizontal());
                    let (win_id, settings) = top.open(output_id.0, w, h);
                    self.tops.insert(win_id, top);
                    self.ids.insert(win_id, PlotInfo::Top(output_id));
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
                    .output_infos
                    .get(&output_id)
                    .map(Background::output_geometry)
                    .unwrap_or((0.0, 0.0, 0.0, 0.0))
                    != Background::output_geometry(&output);
                self.output_infos.insert(output_id, output);
                if geom_changed {
                    // `%` bar sizes resolve against this geometry: re-push
                    // every bar's px size so nothing goes stale, plus heals.
                    Command::batch(vec![
                        Self::repaint_burst(),
                        Top::reapply_for_output(self, output_id),
                    ])
                } else {
                    Command::none()
                }
            }
            Plant::Wayland(LandEvent::OutputRemoved(output)) => {
                let output_id = OutputId::from(&output);
                let mut cmds = Vec::new();
                if let Some(id) = Background::remove_for_output(
                    &mut self.backgrounds,
                    &mut self.background_ids,
                    &mut self.ids,
                    output_id,
                ) {
                    self.last_cursor.remove(&id);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(id),
                    )));
                }
                // remove all tops for this output (delegated to Top layer)
                for wid in Top::remove_for_output(&mut self.tops, &mut self.ids, output_id) {
                    self.last_cursor.remove(&wid);
                    self.press_starts.remove(&wid);
                    cmds.push(iced_runtime::task::effect(Action::Window(
                        WindowAction::Close(wid),
                    )));
                }
                // popups live on the same output (delegated to Popup layer)
                for wid in Popup::remove_for_output(&mut self.popups, &mut self.ids, output_id) {
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
            Plant::Wayland(LandEvent::NewShell(info)) => {
                // The layer surface actually exists now — the Added-time heals
                // may have fired before its configure. Heal only for our own
                // Background windows.
                if matches!(self.ids.get(&info.window), Some(PlotInfo::Background(_))) {
                    Self::repaint_burst()
                } else {
                    Command::none()
                }
            }
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
                    self.press_starts.remove(&id);
                    return Command::none();
                }
                match self.id_info(id) {
                    Some(PlotInfo::Top(_)) => Top::handle_graft(self, id, &event),
                    _ => Background::handle_graft(self, id, &event),
                }
            }
            Plant::BackgroundPlot(BackgroundEvent::SelectionTick) => {
                // drive fade animation (speed-scaled InOutQuad) — clear when done
                if let Some(start) = self.fade_start
                    && start.elapsed() >= self.config.animation.speed.duration()
                {
                    self.fade_rect = None;
                    self.fade_start = None;
                }
                Command::none()
            }
            Plant::BackgroundPlot(BackgroundEvent::Repaint) => Background::repaint(self),
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
                    return Command::done(Plant::Config(ConfigEvent::ConfigReloaded(cfg)));
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
                // Declarative widgets hot-reload on the same tick: fresh
                // defs repaint every bar on the next frame.
                if let Some((defs, mtime)) = crate::config::WidgetsFile::poll(&self.widgets_mtime) {
                    self.widgets_mtime = mtime;
                    return Command::done(Plant::Config(ConfigEvent::WidgetsReloaded(defs)));
                }
                Command::none()
            }
            Plant::Config(ConfigEvent::ConfigReloaded(cfg)) => {
                // The file changed under us (external edit, or a theme file
                // hot-reload re-emit): the on-disk config wins and replaces
                // any staged local edits. Pending save timers are cancelled.
                if self.config_dirty {
                    println!("riced: config changed on disk — replacing staged local edits");
                }
                self.config_dirty = false;
                self.config_save_seq += 1;
                let images_changed = cfg.background.image != self.config.background.image;
                let switched_to_dynamic =
                    cfg.theme.name == "dynamic" && self.config.theme.name != "dynamic";
                self.config = cfg;
                Self::sync_wallpapers(&self.config, &mut self.wallpapers);
                if images_changed || switched_to_dynamic {
                    return self.arm_regen_theme();
                }
                Command::none()
            }
            Plant::Config(ConfigEvent::WidgetsReloaded(defs)) => {
                // `widgets.toml` changed under us: swap the live registry
                // and rebuild Lua states (scripts may have changed too).
                // Open menus reference dead states, so they close;
                // bars re-resolve slot names on the next redraw (Scope::All).
                self.widgets = defs;
                Top::init_widget_lua(self);
                let stale: Vec<iced::window::Id> = self.popups.keys().copied().collect();
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
                .then(|| crate::colorgen::effective_templates_dir(&self.config.theme))
                .flatten();
                // Removal shifts image indices: capture the index before
                // `apply` moves `patch`, then remap every panel's pick.
                let removed_index = match &patch {
                    ConfigPatch::RemoveImage { index } => Some(*index),
                    _ => None,
                };
                self.config.apply(patch);
                if sync {
                    Self::sync_wallpapers(&self.config, &mut self.wallpapers);
                }
                if let Some(index) = removed_index {
                    for setting in self.settings.values_mut() {
                        setting.image_removed(index);
                    }
                }
                self.config_dirty = true;
                self.config_save_seq += 1;
                let save_seq = self.config_save_seq;
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
                if seq != self.config_save_seq || !self.config_dirty {
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
                Command::none()
            }
            Plant::Config(ConfigEvent::RegenTimer(seq)) => {
                // Stale timers (superseded by a later arm, or already
                // consumed by a fire) never run twice.
                if seq != self.theme_regen_seq {
                    return Command::none();
                }
                self.fire_regen_theme()
            }
            Plant::Config(ConfigEvent::ThemeRegenerated(errors)) => {
                self.theme_regen_running = false;
                if errors.is_empty() {
                    println!("riced: dynamic theme regenerated");
                } else {
                    for err in &errors {
                        eprintln!("riced: dynamic theme: {err}");
                    }
                }
                // Edits that landed mid-flight re-dirty the flag; chain one
                // follow-up regen instead of dropping them.
                if self.theme_regen_dirty {
                    return self.arm_regen_theme();
                }
                Command::none()
            }
            Plant::TopPlot(TopEvent::Pressed(id, button)) => Top::handle_press(self, id, button),
            Plant::TopPlot(TopEvent::Released(id, button)) => Top::handle_release(self, id, button),
            Plant::TopPlot(TopEvent::Remove(id)) => Top::handle_remove(self, id),
            Plant::TopPlot(TopEvent::SetLength(id, value)) => {
                Top::handle_set_length(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetThickness(id, value)) => {
                Top::handle_set_thickness(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetSlots(id, value)) => Top::handle_set_slots(self, id, value),
            Plant::TopPlot(TopEvent::SetSlotAlign(id, pos, align)) => {
                Top::handle_set_slot_align(self, id, pos, align)
            }
            Plant::TopPlot(TopEvent::SetSlotWidget(id, pos, widget, enabled)) => {
                Top::handle_set_slot_widget(self, id, pos, widget, enabled)
            }
            Plant::TopPlot(TopEvent::SetSlotPadding(id, value)) => {
                Top::handle_set_slot_padding(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetSlotSpacing(id, value)) => {
                Top::handle_set_slot_spacing(self, id, value)
            }
            Plant::TopPlot(TopEvent::WidgetTick) => Top::handle_widget_tick(self),
            Plant::TopPlot(TopEvent::WidgetsChanged) => Popup::refresh_bodies(self),
            Plant::TopPlot(TopEvent::PopupSelect(id, action)) => {
                Popup::handle_select(self, id, action)
            }
            Plant::TopPlot(TopEvent::CellAction(widget, action)) => {
                Top::handle_cell_action(self, widget, action)
            }
            Plant::TopPlot(TopEvent::SetOpacity(id, value)) => {
                Top::handle_set_opacity(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetFloating(id, value)) => {
                Top::handle_set_floating(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetMarginTop(id, value)) => {
                Top::handle_set_margin_top(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetMarginRight(id, value)) => {
                Top::handle_set_margin_right(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetMarginBottom(id, value)) => {
                Top::handle_set_margin_bottom(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetMarginLeft(id, value)) => {
                Top::handle_set_margin_left(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetRadiusTl(id, value)) => {
                Top::handle_set_radius_tl(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetRadiusTr(id, value)) => {
                Top::handle_set_radius_tr(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetRadiusBl(id, value)) => {
                Top::handle_set_radius_bl(self, id, value)
            }
            Plant::TopPlot(TopEvent::SetRadiusBr(id, value)) => {
                Top::handle_set_radius_br(self, id, value)
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
            Plant::SettingPlot(SettingEvent::MapViewChanged { id, view }) => {
                // Mouse drop ends the drag: the incoming view has drag None,
                // so an image drag in the stored view means wallpapers moved.
                // (Per-move patches never dirty the theme — only the drop
                // arms the 2s regen countdown.)
                let dropped_image = self
                    .settings
                    .get(&id)
                    .is_some_and(|s| s.map_view().drag.is_some_and(|d| d.image.is_some()));
                if let Some(setting) = self.settings.get_mut(&id) {
                    setting.set_map_view(view);
                }
                if dropped_image {
                    return self.arm_regen_theme();
                }
                Command::none()
            }
            Plant::TopPlot(TopEvent::Sow) => {
                // delegate to Top layer (closest-edge detection and spawn)
                let menu_pos = self.context_menu.as_ref().map(|cm| Point::new(cm.x, cm.y));
                let menu_output = self.context_menu.as_ref().and_then(|cm| cm.output);
                if let Some(cm) = &mut self.context_menu {
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
        | Plant::BackgroundPlot(BackgroundEvent::Released(..))
        // Accept re-emits as a Patch (already Scope::All); cancel is silent.
        | Plant::BackgroundPlot(BackgroundEvent::WallpaperPicked(Some(_))) => Scope::All,
        Plant::BackgroundPlot(BackgroundEvent::PickWallpaper)
        | Plant::BackgroundPlot(BackgroundEvent::WallpaperPicked(None)) => Scope::None,
        // Lua-widget timer only runs due scripts (repaint goes through
        // WidgetsChanged when an output actually moved).
        Plant::TopPlot(TopEvent::WidgetTick) => Scope::None,
        | Plant::TopPlot(TopEvent::Pressed(..))
        | Plant::TopPlot(TopEvent::Released(..))
        | Plant::TopPlot(TopEvent::Remove(..))
        | Plant::TopPlot(TopEvent::SetLength(..))
        | Plant::TopPlot(TopEvent::SetThickness(..))
        | Plant::TopPlot(TopEvent::SetSlots(..))
        | Plant::TopPlot(TopEvent::SetSlotAlign(..))
        | Plant::TopPlot(TopEvent::SetSlotWidget(..))
        | Plant::TopPlot(TopEvent::SetSlotPadding(..))
        | Plant::TopPlot(TopEvent::SetSlotSpacing(..))
        | Plant::TopPlot(TopEvent::WidgetsChanged)
        | Plant::TopPlot(TopEvent::PopupSelect(..))
        | Plant::TopPlot(TopEvent::CellAction(..))
        | Plant::TopPlot(TopEvent::SetOpacity(..))
        | Plant::TopPlot(TopEvent::SetFloating(..))
        | Plant::TopPlot(TopEvent::SetMarginTop(..))
        | Plant::TopPlot(TopEvent::SetMarginRight(..))
        | Plant::TopPlot(TopEvent::SetMarginBottom(..))
        | Plant::TopPlot(TopEvent::SetMarginLeft(..))
        | Plant::TopPlot(TopEvent::SetRadiusTl(..))
        | Plant::TopPlot(TopEvent::SetRadiusTr(..))
        | Plant::TopPlot(TopEvent::SetRadiusBl(..))
        | Plant::TopPlot(TopEvent::SetRadiusBr(..))
        | Plant::Graft(_, Event::Mouse(iced::mouse::Event::ButtonReleased(_))) => Scope::All,
        // CursorMoved is handled via throttled background tick; no direct redraw to avoid flood
        Plant::Graft(_, Event::Mouse(iced::mouse::Event::CursorMoved { .. })) => Scope::None,
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
            | SettingEvent::MapViewChanged { id, .. },
        ) => Scope::Window(*id),
        Plant::Graft(_, Event::Mouse(_)) => Scope::None,
        Plant::Graft(_, _) => Scope::None,
        Plant::Wayland(_) => Scope::All,
        _ => Scope::All,
    }
}
