use iced_exwlshell::daemon;
use wayland_client::Connection;

mod app;

use app::{Plots, redraw_scope};
use iced_exwlshell::reexport::{Anchor, BlurOption, Layer, LayerSize};
use iced_exwlshell::settings::{LayerShellSettings, Settings, StartMode};

pub fn main() -> Result<(), iced_exwlshell::Error> {
    tracing_subscriber::fmt().init();
    let connection = Connection::connect_to_env().unwrap();
    let connection2 = connection.clone();

    let (shell_broadcast, shell_events) = iced_wayland_subscriber::shell::channel();

    daemon(
        move || Plots::new(shell_events.clone()),
        Plots::namespace,
        Plots::update,
        Plots::view,
    )
    .subscription(Plots::subscription)
    .settings(Settings {
        layer_settings: LayerShellSettings {
            anchor: Anchor::all(), // Top|Bottom|Left|Right required for FILL src/size.rs:94
            size: LayerSize::FILL, // not fill_width(1)
            exclusive_zone: 0,     // i32, not LayerSize (0 = no reserve, -1 = ignore)
            start_mode: StartMode::AllScreens, // one fullscreen surface per output
            layer: Layer::Background,
            blur_option: BlurOption::FullRegion,
            events_transparent: false, // false = receive click/drag, true => pass through
            ..Default::default()
        },
        with_connection: Some(connection2.into()),
        shell_broadcast,
        ..Default::default()
    })
    .redraw_scope(redraw_scope)
    .run()
}
