//! Headless command execution, separate from argument parsing and daemon boot.
use crate::{config, theme, theme_gen as colorgen};

pub(crate) fn run_apply_templates(
    templates: Option<std::path::PathBuf>,
    name: Option<String>,
) -> Result<(), iced_exwlshell::Error> {
    let (config, _) = config::Config::load();
    theme::ensure_user_themes();
    colorgen::ensure_user_templates();
    let dir = templates.or_else(|| colorgen::effective_templates_dir(&config.theme));
    let Some(dir) = dir else {
        eprintln!(
            "riced: no templates dir (pass --templates, set [theme] templates_dir, or seed ~/.config/riced/templates/)"
        );
        std::process::exit(1);
    };
    let name = name.unwrap_or_else(|| config.theme.name.clone());
    let text = match colorgen::stored_theme_text(&name) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("riced: {e}");
            std::process::exit(1);
        }
    };
    let errors = colorgen::render_theme_templates(&dir, &text, config.theme.darkmode);
    if errors.is_empty() {
        println!("riced: applied {name} to {}", dir.display());
    } else {
        for err in &errors {
            eprintln!("riced: template: {err}");
        }
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn run_generate_theme(
    variant: Option<String>,
    set: bool,
    templates: Option<std::path::PathBuf>,
    no_templates: bool,
) -> Result<(), iced_exwlshell::Error> {
    let (mut config, _) = config::Config::load();
    theme::ensure_user_themes();
    colorgen::ensure_user_templates();
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
    let rects: Vec<(f32, f32, f32, f32)> = config
        .background
        .image
        .iter()
        .filter_map(crate::ui::widgets::display_map::MapLayer::resolved)
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
    if no_templates {
        println!("riced: templates skipped (--no-templates)");
    } else if let Some(dir) = templates.or_else(|| colorgen::effective_templates_dir(&config.theme))
    {
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
        config.theme.name = "dynamic".into();
        config.theme.variant = variant;
        config.save();
        println!("riced: theme set to dynamic (daemon hot-reloads)");
    }
    Ok(())
}
