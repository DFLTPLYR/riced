use iced::Event;
use iced::window::Id;
use iced_exwlshell::to_layer_message;
use iced_wayland_subscriber::OutputInfo;
use iced_wayland_subscriber::shell::ShellInfo;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
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

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Plant {
    // Create,Update, Delete
    Grow,
    Uproot(Id),
    Tend,
    Wayland(LandEvent),
    Graft(Id, Event),
    SelectionTick,
    AddTop,
}
