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
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum TopEvent {
    Sow,
    Pressed(Id, Button),
    Released(Id, Button),
    /// Bar length as % of the output long axis (1–100): applied live.
    SetLength(Id, f32),
    /// Bar thickness in px (1–thin output axis): applied live.
    SetThickness(Id, f32),
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
    Sow,
    SelectionTick,
    Repaint,
    Pressed(Id, Button),
    Released(Id, Button),
}

#[derive(Debug, Clone, Copy)]
pub enum SettingEvent {
    Select(Id, crate::app::layers::SettingPage),
    /// Pick the bar edited by the Panel page (`id` = settings window).
    SelectBar(Id, Id),
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
