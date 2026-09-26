use iced_exwlshell::daemon;
use wayland_client::Connection;

mod app;
mod cli;
mod components;
mod composables;
mod config;

use app::{Plots, redraw_scope};
use iced_exwlshell::reexport::{Anchor, BlurOption, Layer, LayerSize};
use iced_exwlshell::settings::{LayerShellSettings, Settings, StartMode};

pub fn main() -> Result<(), iced_exwlshell::Error> {
    tracing_subscriber::fmt().init();
    // iced (wgpu, etc.) logs through the `log` facade, which is dropped
    // entirely unless a backend is installed. Forward it into tracing so
    // internals (e.g. the swapchain alpha-mode selection) are visible.
    let _ = tracing_log::LogTracer::init();
    // Single-shot client: `riced open-settings` queues a request for the
    // running daemon through the command file, no surfaces involved.
    match cli::parse() {
        Some(cli::Commands::OpenSettings) => {
            if let Err(e) = cli::queue_open_settings() {
                eprintln!("riced: cannot queue open-settings: {e}");
                std::process::exit(1);
            }
            println!("riced: settings requested");
            Ok(())
        }
        // No subcommand: run the shell daemon.
        None => run_daemon(),
    }
}

fn run_daemon() -> Result<(), iced_exwlshell::Error> {
    let connection = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!(
            "riced: cannot connect to Wayland ({e:?}); \
             is WAYLAND_DISPLAY set and are you inside a Wayland session?"
        );
        std::process::exit(1);
    });
    let connection2 = connection.clone();

    let (shell_broadcast, shell_events) = iced_wayland_subscriber::shell::channel();

    daemon(
        move || Plots::new(shell_events.clone()),
        Plots::namespace,
        Plots::update,
        Plots::view,
    )
    .title(Plots::title)
    .subscription(Plots::subscription)
    // Transparent clear color: every frame is cleared with this before widgets
    // blend onto it, so alpha < 1 here is what lets the compositor (Hyprland
    // blur/opacity rules) see through the surface. The default theme base is
    // opaque, which bakes alpha = 1.0 into every pixel no matter what translucent
    // container colors the layers use. This is daemon-global, so Background/Top
    // keep their opaque look via explicit root backgrounds in their own views.
    .style(|_, _| iced::theme::Style {
        background_color: iced::Color::TRANSPARENT,
        text_color: iced::Color::WHITE,
    })
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
