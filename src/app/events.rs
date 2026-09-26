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
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum TopEvent {
    Sow,
    Pressed(Id, Button),
    Released(Id, Button),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum BackgroundEvent {
    Sow,
    SelectionTick,
    Pressed(Id, Button),
    Released(Id, Button),
}

#[derive(Debug, Clone, Copy)]
pub enum SettingEvent {
    Select(Id, crate::app::layers::SettingPage),
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
