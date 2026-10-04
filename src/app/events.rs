use iced::Event;
use iced::mouse::Button;
use iced::window::Id;
use iced_exwlshell::to_layer_message;
use iced_wayland_subscriber::OutputInfo;
use iced_wayland_subscriber::shell::ShellInfo;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
// Payloads are forwarded from the shell subscriber; several variants are
// currently acknowledged without inspecting their contents.
#[allow(dead_code)]
pub enum LandEvent {
    NewShell(ShellInfo),
    Closed(Id),
    WindowOutputChanged {
        window: Id,
        output: Option<OutputInfo>,
    },
    OutputAdded(OutputInfo),
    OutputUpdated(OutputInfo),
    OutputRemoved(OutputInfo),
    Locked,
    LockDenied,
    LockedFinished,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum ConfigEvent {
    ConfigTick,
    ConfigReloaded(crate::config::Config),
    /// Runtime edit from a Settings-panel control. Applied to the single
    /// live config, persisted, and broadcast via full redraw.
    Patch(crate::config::ConfigPatch),
    /// Coalesced disk write for staged `Patch` edits (carries the arm
    /// generation; stale timers are ignored). Slider drags fire dozens of
    /// patches a second — memory updates every tick, the file once idle.
    SaveTimer(u64),
    /// Slider released: persist staged edits to the config file right now
    /// instead of waiting out the coalescing timer. No-op when clean.
    SaveNow,
    /// Template re-render after a theme switch finished (sys `change_theme`
    /// equivalent). Carries per-template errors, empty when all applied.
    TemplatesDone(Vec<String>),
    /// Pending-regen countdown elapsed (carries the arm generation;
    /// stale timers are ignored).
    RegenTimer(u64),
    /// Dynamic-theme regeneration finished: fresh `dynamic.json` on disk
    /// (empty errors) or the failure reasons. Always repaints so new
    /// colors apply on the next frame.
    ThemeRegenerated(Vec<String>),
    /// `widgets.toml` changed on disk: fresh declarative widget defs.
    /// Stored live and repainted (bars re-resolve slot names).
    WidgetsReloaded(Vec<crate::config::WidgetDef>),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum TopEvent {
    Sow,
    Pressed(Id, Button),
    Released(Id, Button),
    /// Left press on one widget (renderer hit-tested per-widget mouse
    /// area): records the press target so the matching release — and
    /// only it — dispatches the click. Bubbled outer releases ignore
    /// widget targets (their own handler owns the click).
    WidgetPressed(Id, usize, String),
    /// Left release on one widget: clicks only when the press target
    /// matches (same bar/slot/widget, under the hold threshold).
    WidgetReleased(Id, usize, String),
    /// Remove the bar: close its window, drop tracking, and delete its
    /// `[[bar]]` entry so it stays gone after restart.
    Remove(Id),
    /// Bar length as % of the output long axis (1–100): applied live.
    SetLength(Id, f32),
    /// Bar thickness in px (1–thin output axis): applied live.
    SetThickness(Id, f32),
    /// Grid cells along the long axis (columns when horizontal, rows when
    /// vertical): applied live, persisted.
    SetSlots(Id, u32),
    /// Child alignment of one slot (position, not label): applied live,
    /// persisted to the bar's `[[bar]] aligns` entry.
    SetSlotAlign(Id, usize, crate::app::layers::top::SlotAlign),
    /// Widget of one slot by `widgets.toml` name (position, not
    /// label): checked appends the name, unchecked removes it. Applied
    /// live, persisted to the bar's `[[bar]] widgets` entry.
    SetSlotWidget(Id, usize, String, bool),
    /// Inset inside every slot cell (px): applied live, persisted to
    /// the bar's `[[bar]] slot_padding` entry.
    SetSlotPadding(Id, f32),
    /// Gap between slot cells and icon/text segments (px): applied
    /// live, persisted to the bar's `[[bar]] slot_spacing` entry.
    SetSlotSpacing(Id, f32),
    /// 250ms Lua-widget timer: re-runs every `render()` whose interval
    /// elapsed. Never repaints by itself — emits `WidgetsChanged` when
    /// an output moved.
    WidgetTick,
    /// 16ms animation frame: advances the aura-anim runtime for list
    /// enter/exit transitions and sweeps settled ghosts. Only
    /// subscribed while a motion is active (repaint via Scope::All).
    WidgetAnim,
    /// A Lua widget output moved: repaint so bars pick the new text up.
    WidgetsChanged,
    /// Click a popup item: run the widget's `on_action()` with the item
    /// key, then re-render menu and cell.
    PopupSelect(Id, String),
    /// Click a cell button (`ui.button` in a `render()` tree): run the
    /// owning widget's `on_action()` with the button key, then
    /// re-render that widget (a toggle flips its next output).
    CellAction(String, String),
    /// Backdrop opacity preset (0.0/0.25/0.5/0.75/1.0): single commit.
    SetOpacity(Id, f32),
    /// Floating look: inset the backdrop with margins (view-live padding,
    /// exclusive zone kept): applied live.
    SetFloating(Id, bool),
    /// Content inset in px, floating look only (view-live padding).
    SetMarginTop(Id, i32),
    SetMarginRight(Id, i32),
    SetMarginBottom(Id, i32),
    SetMarginLeft(Id, i32),
    /// Per-corner rounding in px (view-live).
    SetRadiusTl(Id, f32),
    SetRadiusTr(Id, f32),
    SetRadiusBl(Id, f32),
    SetRadiusBr(Id, f32),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum BackgroundEvent {
    SelectionTick,
    Repaint,
    Pressed(Id, Button),
    Released(Id, Button),
    /// Open a native file dialog for a new wallpaper. The async result
    /// comes back as `WallpaperPicked`: `Some(path)` on accept, `None`
    /// when the dialog is cancelled — which is a no-op by design.
    PickWallpaper,
    WallpaperPicked(Option<String>),
}

#[derive(Debug, Clone, Copy)]
pub enum SettingEvent {
    Select(Id, crate::app::layers::SettingPage),
    /// Pick the bar edited by the Panel page (`id` = settings window).
    SelectBar(Id, Id),
    /// Pick the wallpaper image edited below the map (`id` = settings
    /// window, `usize` = index into `[[background.image]]`).
    SelectImage(Id, usize),
    /// Pick the slot the Panel page aligns (`id` = settings window,
    /// `usize` = position into the edited bar's slots).
    SelectSlot(Id, usize),
    MapViewChanged {
        id: Id,
        view: crate::components::display_map::MapView,
    },
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Plant {
    // Create,Update, Delete
    Tend,
    Sprout,
    Uproot(Id),
    Graft(Id, Event),
    // File IPC poll tick (see `cli`): drains `riced.cmd` queued by CLI clients.
    IpcPoll,
    // Wayland
    Wayland(LandEvent),
    // Layers/Plots
    TopPlot(TopEvent),
    BackgroundPlot(BackgroundEvent),
    SettingPlot(SettingEvent),
    // Config
    Config(ConfigEvent),
}
