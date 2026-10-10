//! Vendored template set and `{{var}}` rendering with shell hooks.
use super::scheme::expand_tilde;
use crate::config::ThemeConfig;
use include_dir::{Dir, include_dir};
use regex::Regex;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Default templates set (vendored reshell core/theme)
// ---------------------------------------------------------------------------

/// Default `[templates]` set, copied from reshell `core/theme/` (inputs +
/// `config.toml`), embedded at compile time.
static DEFAULT_TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// `~/.config/riced/templates` (`$XDG_CONFIG_HOME` aware): the editable
/// copy of [`DEFAULT_TEMPLATES`] that rendering actually reads.
pub fn user_templates_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("riced").join("templates"))
        .unwrap_or_else(|| PathBuf::from("templates"))
}

/// Seed the user templates dir with the embedded defaults: every file the
/// user doesn't already have (never overwrites edits). `into` selects the
/// root (used by tests to avoid touching `$HOME`).
pub fn ensure_user_templates_into(root: &Path) -> io::Result<()> {
    std::fs::create_dir_all(root)?;
    seed_recursive(&DEFAULT_TEMPLATES, root)
}

fn seed_recursive(dir: &Dir, dest_root: &Path) -> io::Result<()> {
    // File::path is root-relative, so every level joins against dest_root.
    for f in dir.files() {
        let dest = dest_root.join(f.path());
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, f.contents())?;
    }
    for d in dir.dirs() {
        seed_recursive(d, dest_root)?;
    }
    Ok(())
}

/// All embedded template files, recursively.
#[cfg(test)]
fn default_template_files(dir: &Dir, out: &mut Vec<std::path::PathBuf>) {
    out.extend(dir.files().map(|f| f.path().to_path_buf()));
    for d in dir.dirs() {
        default_template_files(d, out);
    }
}

/// Seed [`user_templates_dir`] (best-effort: failures log and rendering
/// reports the missing dir per template).
pub fn ensure_user_templates() {
    let dir = user_templates_dir();
    if let Err(e) = ensure_user_templates_into(&dir) {
        eprintln!("templates: cannot seed {}: {e}", dir.display());
    }
}

/// Which templates dir a theme switch / CLI run renders, if any:
/// `"off"` disables; an explicit `[theme] templates_dir` wins; otherwise
/// the seeded user dir when its `config.toml` exists.
pub fn effective_templates_dir(theme: &ThemeConfig) -> Option<PathBuf> {
    if theme.templates_dir == "off" {
        return None;
    }
    if !theme.templates_dir.is_empty() {
        return Some(expand_tilde(&theme.templates_dir));
    }
    let dir = user_templates_dir();
    dir.join("config.toml").is_file().then_some(dir)
}

// ---------------------------------------------------------------------------
// Templates: {{var}} substitution + pre/post hooks (same as sys)
// ---------------------------------------------------------------------------

static TEMPLATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*(.+?)\s*\}\}").unwrap());

/// Substitute `{{ key }}` variables; unknown keys pass through untouched.
pub fn render_template(content: &str, variables: &HashMap<String, String>) -> String {
    TEMPLATE_RE
        .replace_all(content, |caps: &regex::Captures| {
            let expr = caps[1].trim();
            variables
                .get(expr)
                .cloned()
                .unwrap_or_else(|| caps[0].to_string())
        })
        .to_string()
}

fn run_hook(hook: &str, variables: &HashMap<String, String>) -> bool {
    let rendered = render_template(hook, variables);
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(serde::Deserialize)]
struct TemplatesConfig {
    templates: Option<HashMap<String, TemplateEntry>>,
}

#[derive(serde::Deserialize)]
struct TemplateEntry {
    input_path: String,
    output_path: String,
    pre_hook: Option<String>,
    post_hook: Option<String>,
}

/// Render every `[templates.*]` entry of `<dir>/config.toml`: read
/// `<dir>/<input_path>`, substitute variables, write to `output_path`
/// (`~` expands to `$HOME`, parents created), running optional
/// `pre_hook`/`post_hook` shell snippets. Returns per-template errors
/// (empty = all applied). Same semantics as sys, sequential.
pub fn process_templates(dir: &Path, variables: &HashMap<String, String>) -> Vec<String> {
    let config_file = dir.join("config.toml");
    let content = match std::fs::read_to_string(&config_file) {
        Ok(c) => c,
        Err(e) => {
            return vec![format!("failed to read {}: {e}", config_file.display())];
        }
    };

    let config: TemplatesConfig = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            return vec![format!("failed to parse config.toml: {e}")];
        }
    };

    let templates = match config.templates {
        Some(t) => t,
        None => {
            return vec!["no [templates] sections in config.toml".to_string()];
        }
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let mut errors = Vec::new();

    for (name, entry) in &templates {
        let input = dir.join(&entry.input_path);
        let content = match std::fs::read_to_string(&input) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{name}: failed to read {}: {e}", input.display()));
                continue;
            }
        };

        if let Some(ref hook) = entry.pre_hook
            && !run_hook(hook, variables)
        {
            errors.push(format!("{name}: pre_hook failed: {hook}"));
        }

        let rendered = render_template(&content, variables);

        let output_raw = entry.output_path.replace("~", &home);
        let output = Path::new(&output_raw);
        if let Some(parent) = output.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(output, &rendered) {
            errors.push(format!(
                "{name}: failed to write {}: {e}",
                entry.output_path
            ));
        }

        if let Some(ref hook) = entry.post_hook
            && !run_hook(hook, variables)
        {
            errors.push(format!("{name}: post_hook failed: {hook}"));
        }
    }

    errors
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn render_template_substitutes_and_passes_unknown_through() {
        let mut vars = HashMap::new();
        vars.insert("colors.primary".to_string(), "#aabbcc".to_string());
        assert_eq!(
            render_template("fg {{colors.primary}}; bg {{colors.missing}}", &vars),
            "fg #aabbcc; bg {{colors.missing}}"
        );
        // whitespace-tolerant, like reshell's kitty template
        assert_eq!(
            render_template("cursor            {{ colors.primary }}", &vars),
            "cursor            #aabbcc"
        );
    }

    #[test]
    fn process_templates_renders_files_and_reports_errors() {
        let dir = std::env::temp_dir().join(format!("riced-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[templates.good]\ninput_path = \"in.txt\"\noutput_path = \"OUT_PLACEHOLDER/out.txt\"\n\
             [templates.broken]\ninput_path = \"missing.txt\"\noutput_path = \"out2.txt\"\n",
        )
        .unwrap();
        // output under the temp dir (avoids touching $HOME in tests)
        let out = dir.join("out").display().to_string();
        let cfg = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            cfg.replace("OUT_PLACEHOLDER", &out),
        )
        .unwrap();
        std::fs::write(dir.join("in.txt"), "primary={{colors.primary}}\n").unwrap();

        let mut vars = HashMap::new();
        vars.insert("colors.primary".to_string(), "#112233".to_string());
        let errors = process_templates(&dir, &vars);
        assert_eq!(
            std::fs::read_to_string(dir.join("out").join("out.txt")).unwrap(),
            "primary=#112233\n"
        );
        // only the missing-input template errors
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("broken:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_templates_embed_config_and_inputs() {
        let mut files = Vec::new();
        default_template_files(&DEFAULT_TEMPLATES, &mut files);
        // mirrors reshell core/theme (inputs + config.toml + colors.json)
        assert!(files.len() >= 20, "embedded {} files", files.len());
        assert!(
            files.iter().any(|p| p == Path::new("config.toml")),
            "config.toml embedded"
        );
        assert!(
            files
                .iter()
                .any(|p| p == Path::new("kitty/kitty-colors.conf")),
            "kitty input embedded"
        );
    }

    #[test]
    fn ensure_user_templates_seeds_without_overwriting() {
        let root = std::env::temp_dir().join(format!("riced-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // pre-existing user edit survives seeding
        std::fs::create_dir_all(root.join("kitty")).unwrap();
        std::fs::write(root.join("kitty/kitty-colors.conf"), "user edit\n").unwrap();

        ensure_user_templates_into(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("kitty/kitty-colors.conf")).unwrap(),
            "user edit\n"
        );
        // missing files appear, including config.toml
        assert!(root.join("config.toml").is_file());
        assert!(root.join("hypr/colors.lua").is_file());
        // second run is a no-op
        ensure_user_templates_into(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn effective_templates_dir_honors_off_and_explicit() {
        let mut cfg = ThemeConfig::default();
        // explicit dir wins as-is (existence is the renderer's problem)
        cfg.templates_dir = "/tmp/riced-explicit-tpl".to_string();
        assert_eq!(
            effective_templates_dir(&cfg),
            Some(PathBuf::from("/tmp/riced-explicit-tpl"))
        );
        cfg.templates_dir = "~/tpl".to_string();
        assert_eq!(
            effective_templates_dir(&cfg),
            Some(PathBuf::from(std::env::var("HOME").unwrap()).join("tpl"))
        );
        // "off" disables even with a real dir behind it
        cfg.templates_dir = "off".to_string();
        assert_eq!(effective_templates_dir(&cfg), None);
    }
}
