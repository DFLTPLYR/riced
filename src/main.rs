use iced_exwlshell::daemon;
use wayland_client::Connection;

mod app;

use app::{Monitor, redraw_scope};
use iced_exwlshell::reexport::{BlurOption, Layer, LayerSize};
use iced_exwlshell::settings::{LayerShellSettings, Settings, StartMode};

pub fn main() -> Result<(), iced_exwlshell::Error> {
    tracing_subscriber::fmt().init();
    let connection = Connection::connect_to_env().unwrap();
    let connection2 = connection.clone();

    let (shell_broadcast, shell_events) = iced_wayland_subscriber::shell::channel();

    daemon(
        move || Monitor::new(shell_events.clone()),
        Monitor::namespace,
        Monitor::update,
        Monitor::view,
    )
    .style(Monitor::style)
    .subscription(Monitor::subscription)
    .settings(Settings {
        layer_settings: LayerShellSettings {
            size: LayerSize::fill_width(1),
            exclusive_zone: 0,
            start_mode: StartMode::AllScreens,
            layer: Layer::Background,
            blur_option: BlurOption::FullRegion,
            events_transparent: true,
            ..Default::default()
        },
        with_connection: Some(connection2.into()),
        shell_broadcast,
        keep_compositor_alive: false,
        ..Default::default()
    })
    .redraw_scope(redraw_scope)
    .run()
}
