use iced::Event;
use iced::window::Id;
use iced_exwlshell::to_layer_message;
use iced_wayland_subscriber::{OutputId, OutputInfo};

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum WayEvent {
    OutputInsert(OutputInfo),
    OutputRemoved(OutputId),
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Plant {
    // Create,Update, Delete
    Grow,
    Uproot(Id),
    Tend,
    Wayland(WayEvent),
    Graft(Id, Event),
    SelectionTick,
    AddTop,
}
