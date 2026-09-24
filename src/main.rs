use iced_exwlshell::daemon;
use wayland_client::Connection;

mod app;
mod composables;
mod components;
mod config;

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
            // daemon's own surface is a tiny 1px placeholder (not used for selection)
            // per-output fullscreen Backgrounds for selection are spawned via Plots on OutputInsert
            anchor: Anchor::Top,
            size: LayerSize::fill_width(1),
            exclusive_zone: 0,
            start_mode: StartMode::Active,
            layer: Layer::Background,
            blur_option: BlurOption::None,
            events_transparent: true,
            ..Default::default()
        },
        with_connection: Some(connection2.into()),
        shell_broadcast,
        ..Default::default()
    })
    .redraw_scope(redraw_scope)
    .run()
}
