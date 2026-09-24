use iced::Event;
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
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum TopEvent {
    Sow,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum BackgroundEvent {
    Sow,
    SelectionTick,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Plant {
    // Create,Update, Delete
    Tend,
    Uproot(Id),
    Graft(Id, Event),
    Wayland(LandEvent),
    TopPlot(TopEvent),
    BackgroundPlot(BackgroundEvent),
    Config(ConfigEvent),
}
