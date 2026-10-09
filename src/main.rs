use iced_exwlshell::daemon;
use wayland_client::Connection;

mod app;
mod cli;
mod colorgen;
mod components;
mod composables;
mod config;
mod lua;
mod notify;
mod services;
mod shared;
mod shell;
mod theme;
mod theme_gen;
mod ui;

use cli::commands::{run_apply_templates, run_generate_theme};
use iced_exwlshell::settings::{LayerShellSettings, Settings, StartMode};
use shell::{Plots, redraw_scope};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    let _ = tracing_log::LogTracer::init();
    let command = cli::parse();
    if let Some(cli::Commands::LuaDemo { script }) = command {
        lua::demo::run(script)?;
        return Ok(());
    }

    let result = match command {
        Some(cli::Commands::LuaDemo { .. }) => unreachable!("Lua demo handled above"),
        Some(cli::Commands::OpenSettings) => {
            if let Err(e) = cli::queue_open_settings() {
                eprintln!("riced: cannot queue open-settings: {e}");
                std::process::exit(1);
            }
            println!("riced: settings requested");
            Ok(())
        }
        Some(cli::Commands::GenerateTheme {
            variant,
            set,
            templates,
            no_templates,
        }) => run_generate_theme(variant, set, templates, no_templates),
        Some(cli::Commands::ApplyTemplates { templates, name }) => {
            run_apply_templates(templates, name)
        }
        // No subcommand: run the shell daemon.
        None => run_daemon(),
    };
    result.map_err(Into::into)
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
    .theme(|plots: &Plots, _| theme::theme_for(&plots.config.theme))
    .style(theme::app_style)
    .settings(Settings {
        layer_settings: LayerShellSettings {
            start_mode: StartMode::Background,
            ..Default::default()
        },
        with_connection: Some(connection2.into()),
        shell_broadcast,
        ..Default::default()
    })
    .redraw_scope(redraw_scope)
    .run()
}
