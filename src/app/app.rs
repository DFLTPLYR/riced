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

use super::layers::{Background, ContextMenu, SelectionRect, Setting, Top};
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
    pub(crate) settings: HashMap<iced::window::Id, Setting>,
    pub(crate) backgrounds: HashMap<OutputId, Background>,
    pub(crate) background_ids: HashMap<OutputId, iced::window::Id>,
    pub(crate) shell_events: ShellReceiver,
    // per-window cursor
    pub(crate) last_cursor: HashMap<iced::window::Id, Point>,
    // global selection rect (single, like Background.selectionRect)
    pub(crate) selection_rect: SelectionRect,
    // fade animation after select end (QML Behavior on opacity 150ms InOutQuad)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlotInfo {
    Setting,
    Background(OutputId),
    Top(OutputId),
}

impl Plots {
    pub fn new(shell_events: ShellReceiver) -> Self {
        let (config, config_mtime) = Config::load();
        // Seed ~/.config/riced/theme/ with the vendored reshell themes and
        // ~/.config/riced/templates/ with the default template set on
        // first run, then sync the global theme before first paint.
        crate::theme::ensure_user_themes();
        crate::colorgen::ensure_user_templates();
        let theme_mtime = crate::theme::poll(&config.theme, &None).unwrap_or(None);
        crate::theme::sync(&config.theme);
        let mut wallpapers = HashMap::new();
        Self::sync_wallpapers(&config, &mut wallpapers);
        Self {
            ids: HashMap::new(),
            tops: HashMap::new(),
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
            theme_mtime,
            wallpapers,
            repaint_seq: 0,
        }
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
        // QML Behavior 150ms InOutQuad on opacity needs 60fps ticks only while fading
        if self.fade_start.is_some() {
            subs.push(
                iced::time::every(Duration::from_millis(16))
                    .map(|_| Plant::BackgroundPlot(BackgroundEvent::SelectionTick)),
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
                .map(|t| t.view(id))
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
                if let Some(info) = self.ids.get(&id).copied() {
                    match info {
                        PlotInfo::Top(_) => {
                            self.ids.remove(&id);
                            self.tops.remove(&id);
                        }
                        PlotInfo::Background(output) => {
                            self.ids.remove(&id);
                            self.backgrounds.remove(&output);
                            self.background_ids.remove(&output);
                        }
                        PlotInfo::Setting => {
                            Setting::remove(&mut self.settings, &mut self.ids, id);
                        }
                    }
                } else {
                    // Unknown id (e.g. duplicate close event): still drop tracking.
                    Setting::remove(&mut self.settings, &mut self.ids, id);
                }
                // Idempotent close: covers both the in-window close button
                // and the compositor's X button (via close_events -> Uproot).
                iced_runtime::task::effect(Action::Window(WindowAction::Close(id)))
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
                // No auto Top bar on start - user creates via Add Top context menu
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
                    Self::repaint_burst()
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
                // drive fade animation (150ms InOutQuad) — clear when done
                if let Some(start) = self.fade_start
                    && start.elapsed() >= Duration::from_millis(150)
                {
                    self.fade_rect = None;
                    self.fade_start = None;
                }
                Command::none()
            }
            Plant::BackgroundPlot(BackgroundEvent::Repaint) => Background::repaint(self),
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
                Command::none()
            }
            Plant::Config(ConfigEvent::ConfigReloaded(cfg)) => {
                self.config = cfg;
                Self::sync_wallpapers(&self.config, &mut self.wallpapers);
                Command::none()
            }
            Plant::Config(ConfigEvent::Patch(patch)) => {
                // Single source of truth: mutate live config, persist (updating
                // mtime so the poll tick doesn't echo it back), and redraw All
                // so every subscribed view picks the new value up next frame.
                // Image add/remove re-syncs the pre-decoded wallpaper cache;
                // move/scale/z reuse the same pixels.
                let sync = matches!(
                    patch,
                    ConfigPatch::AddImage(_) | ConfigPatch::RemoveImage { .. }
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
                self.config.apply(patch);
                if sync {
                    Self::sync_wallpapers(&self.config, &mut self.wallpapers);
                }
                self.config_mtime = self.config.save().or(self.config_mtime);
                if let Some(dir) = retemplate_dir {
                    return Self::retemplate_command(dir, &self.config);
                }
                Command::none()
            }
            Plant::Config(ConfigEvent::TemplatesDone(errors)) => {
                for err in &errors {
                    eprintln!("riced: template: {err}");
                }
                Command::none()
            }
            Plant::TopPlot(TopEvent::Pressed(id, button)) => Top::handle_press(self, id, button),
            Plant::TopPlot(TopEvent::Released(id, button)) => Top::handle_release(self, id, button),
            Plant::SettingPlot(SettingEvent::Select(id, page)) => {
                Setting::handle_select(self, id, page)
            }
            Plant::SettingPlot(SettingEvent::MapViewChanged { id, view }) => {
                if let Some(setting) = self.settings.get_mut(&id) {
                    setting.set_map_view(view);
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
                if let Some(cmd) = Top::handle_add(
                    &mut self.tops,
                    &mut self.ids,
                    &self.output_infos,
                    menu_pos,
                    menu_output,
                ) {
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
        | Plant::TopPlot(TopEvent::Pressed(..))
        | Plant::TopPlot(TopEvent::Released(..))
        | Plant::Graft(_, Event::Mouse(iced::mouse::Event::ButtonReleased(_))) => Scope::All,
        // CursorMoved is handled via throttled background tick; no direct redraw to avoid flood
        Plant::Graft(_, Event::Mouse(iced::mouse::Event::CursorMoved { .. })) => Scope::None,
        // ConfigTick is a cheap mtime check — redraw only on actual reload.
        // IpcPoll just stats an (usually absent) file — same, no redraw.
        // TemplatesDone only logs hook/template errors; the theme itself
        // already repainted via the Patch that triggered it.
        Plant::Config(ConfigEvent::ConfigTick)
        | Plant::Config(ConfigEvent::TemplatesDone(_))
        | Plant::IpcPoll => Scope::None,
        Plant::Config(ConfigEvent::ConfigReloaded(_)) | Plant::Config(ConfigEvent::Patch(_)) => {
            Scope::All
        }
        // Settings page select / map pan-zoom only affects its own window.
        Plant::SettingPlot(
            SettingEvent::Select(id, _) | SettingEvent::MapViewChanged { id, .. },
        ) => Scope::Window(*id),
        Plant::Graft(_, Event::Mouse(_)) => Scope::None,
        Plant::Graft(_, _) => Scope::None,
        Plant::Wayland(_) => Scope::All,
        _ => Scope::All,
    }
}
