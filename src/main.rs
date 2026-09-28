use iced_exwlshell::daemon;
use wayland_client::Connection;

mod app;
mod cli;
mod colorgen;
mod components;
mod composables;
mod config;
mod theme;

use app::{Plots, redraw_scope};
use iced_exwlshell::reexport::{BlurOption, Layer, LayerSize};
use iced_exwlshell::settings::{LayerShellSettings, Settings, StartMode};

pub fn main() -> Result<(), iced_exwlshell::Error> {
    tracing_subscriber::fmt().init();
    // iced (wgpu, etc.) logs through the `log` facade, which is dropped
    // entirely unless a backend is installed. Forward it into tracing so
    // internals (e.g. the swapchain alpha-mode selection) are visible.
    let _ = tracing_log::LogTracer::init();
    // Single-shot clients queue requests for the running daemon through
    // the command file, or run fully standalone — no surfaces involved.
    match cli::parse() {
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
        }) => run_generate_theme(variant, set, templates),
        // No subcommand: run the shell daemon.
        None => run_daemon(),
    }
}

/// `riced generate-theme`: Material You dynamic theme from the configured
/// wallpapers (ported from `sys/src/colorgen.rs`). Standalone — needs no
/// Wayland connection; the daemon picks the result up via hot-reload.
fn run_generate_theme(
    variant: Option<String>,
    set: bool,
    templates: Option<std::path::PathBuf>,
) -> Result<(), iced_exwlshell::Error> {
    let (mut config, _) = config::Config::load();
    // Seeded here too so `dynamic.json` lands next to the other themes
    // even when the daemon hasn't run yet.
    theme::ensure_user_themes();

    let variant = variant.unwrap_or_else(|| config.theme.variant.clone());
    if !colorgen::VARIANT_NAMES.contains(&variant.as_str()) {
        eprintln!(
            "riced: unknown variant {variant:?}, falling back to tonalspot ({})",
            colorgen::VARIANT_NAMES.join(" ")
        );
    }
    let paths = colorgen::wallpaper_paths(&config);
    if paths.is_empty() {
        eprintln!("riced: no wallpaper images configured (see [background.image])");
        std::process::exit(1);
    }
    // Headless CLI: no output geometry is known, so each placed wallpaper is
    // its own view (its resolved global rect). The daemon path passes live
    // output rects instead — either way the seed comes from the same
    // wallpaper_views math, combined across all views.
    let rects: Vec<(f32, f32, f32, f32)> = config
        .background
        .image
        .iter()
        .filter_map(crate::components::display_map::MapLayer::resolved)
        .collect();
    if rects.is_empty() {
        eprintln!("riced: no wallpaper views to combine");
        std::process::exit(1);
    }
    println!(
        "riced: generating {variant} theme from {} wallpaper(s) ({} view(s))",
        paths.len(),
        rects.len()
    );
    let views = colorgen::render_views(&rects, &config.background.image);

    let generated = match colorgen::generate_from_views(&views, &variant, config.theme.darkmode) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("riced: cannot generate theme: {e}");
            std::process::exit(1);
        }
    };
    match colorgen::write_dynamic_theme(&generated.payload) {
        Ok(path) => println!("riced: wrote {}", path.display()),
        Err(e) => {
            eprintln!("riced: cannot write dynamic.json: {e}");
            std::process::exit(1);
        }
    }

    if let Some(dir) = templates {
        let errors = colorgen::process_templates(&dir, &generated.variables);
        if errors.is_empty() {
            println!("riced: templates applied from {}", dir.display());
        } else {
            for err in &errors {
                eprintln!("riced: template: {err}");
            }
            std::process::exit(1);
        }
    }

    if set {
        config.theme.name = "dynamic".to_string();
        config.theme.variant = variant;
        config.save();
        println!("riced: theme set to dynamic (daemon hot-reloads)");
    }
    Ok(())
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
