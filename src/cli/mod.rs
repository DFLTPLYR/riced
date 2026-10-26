use clap::{Parser, Subcommand};
pub mod build;
pub mod commands;
pub mod extension;
use std::io;
use std::path::{Path, PathBuf};

/// `riced` command line. No subcommand = run the Wayland shell daemon.
/// Subcommands are single-shot clients that queue a request for the
/// running daemon through a command file (see [`ipc_path`]).
#[derive(Debug, Parser)]
#[command(name = "riced", about = "Riced Wayland shell", version)]
pub struct Cli {
    /// Project directory containing config.lua.
    #[arg(short = 'p', long, global = true, conflicts_with = "config")]
    pub project: Option<PathBuf>,
    /// Named project: ./config/riced/NAME, then $XDG_CONFIG_HOME/riced/NAME.
    #[arg(short = 'c', long, global = true)]
    pub config: Option<String>,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Commands {
    /// Run the M0 declarative Lua app in a plain iced window.
    LuaDemo {
        /// Override the embedded main.lua (file edits reload live).
        #[arg(long)]
        script: Option<PathBuf>,
    },
    /// Ask the running daemon to open the Settings panel
    #[command(alias = "settings")]
    OpenSettings,
    /// Generate a Material You `dynamic` theme from the configured
    /// wallpapers (ported from `sys/src/colorgen.rs`) and save it to
    /// `~/.config/riced/theme/dynamic.json`
    GenerateTheme {
        /// Scheme variant: content, tonalspot, monochrome, neutral,
        /// vibrant, expressive, fidelity, rainbow, fruitsalad.
        /// Defaults to `[theme] variant` from config.lua.
        #[arg(long)]
        variant: Option<String>,
        /// Also select the new theme (`[theme] name = "dynamic"`) so the
        /// running daemon hot-reloads to it.
        #[arg(long)]
        set: bool,
        /// Render a `[templates]` dir (TOML with input/output paths
        /// and hooks, same format as sys) against the generated scheme.
        /// Defaults to the seeded `~/.config/riced/templates/`.
        #[arg(long)]
        templates: Option<PathBuf>,
        /// Skip template rendering entirely.
        #[arg(long)]
        no_templates: bool,
    },
    /// Generate and build a project shell from the extensions enabled in
    /// `config.lua`.
    Build {
        /// Build with optimizations.
        #[arg(long)]
        release: bool,
        /// Require the generated Cargo.lock to be current instead of updating it.
        #[arg(long)]
        locked: bool,
        /// Riced framework source directory. Defaults to `RICED_FRAMEWORK`,
        /// then the source tree this binary was built from.
        #[arg(long)]
        framework: Option<PathBuf>,
        /// Cargo target directory. Defaults to `<project>/target/riced-shell`.
        #[arg(long)]
        target_dir: Option<PathBuf>,
    },
    /// Compile the extension crates a project enables.
    #[command(subcommand)]
    Extension(ExtensionCommand),
    /// Re-render a `[templates]` dir with a stored theme object (sys
    /// `change_theme` equivalent): find-and-replace `{{vars}}` without
    /// regenerating. Defaults to the configured theme + `[theme]
    /// templates_dir`.
    ApplyTemplates {
        /// Templates dir to render. Defaults to `[theme] templates-dir`.
        #[arg(long)]
        templates: Option<PathBuf>,
        /// Theme object to pass. Defaults to `[theme] name`.
        #[arg(long)]
        name: Option<String>,
    },
}

/// `riced extension` subcommands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ExtensionCommand {
    /// Compile the extension crates enabled by the project.
    Build {
        /// Extension name; defaults to every extension in the project.
        name: Option<String>,
        /// Build with optimizations.
        #[arg(long)]
        release: bool,
        /// Require the recorded lockfile instead of updating it.
        #[arg(long)]
        locked: bool,
    },
    /// Compile the extension crates, then rebuild the generated shell so
    /// compiled-in changes are linked.
    Update {
        /// Extension name; defaults to every extension in the project.
        name: Option<String>,
        /// Build with optimizations.
        #[arg(long)]
        release: bool,
        /// Require the recorded lockfiles instead of updating them.
        #[arg(long)]
        locked: bool,
        /// Riced framework source directory. Defaults to `RICED_FRAMEWORK`,
        /// then the source tree this binary was built from.
        #[arg(long)]
        framework: Option<PathBuf>,
        /// Cargo target directory. Defaults to `<project>/target/riced-shell`.
        #[arg(long)]
        target_dir: Option<PathBuf>,
    },
}

/// Parse `std::env::args`. Returns the requested client command, if any.
pub fn parse() -> Cli {
    Cli::parse()
}

impl Cli {
    pub fn project_root(&self) -> Result<Option<PathBuf>, String> {
        let candidates = if let Some(path) = &self.project {
            vec![path.clone()]
        } else if let Some(name) = &self.config {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            {
                return Err("config name must contain only letters, digits, '_' or '-'".into());
            }
            let mut paths = vec![PathBuf::from("config/riced").join(name)];
            if let Some(config) = dirs::config_dir() {
                paths.push(config.join("riced").join(name));
            }
            paths
        } else {
            return Ok(None);
        };
        for path in &candidates {
            if path.join("config.lua").is_file() {
                return path
                    .canonicalize()
                    .map(Some)
                    .map_err(|e| format!("{}: {e}", path.display()));
            }
        }
        Err(format!(
            "no config.lua found in {}",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" or ")
        ))
    }
}

/// File the CLI writes and the daemon drains (250ms poll in
/// `Plots::subscription`). `$XDG_RUNTIME_DIR/riced.cmd`, falling back
/// to the temp dir when unset (e.g. nested sessions, tests).
pub fn ipc_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("riced.cmd")
}

/// Commands the daemon understands, one per line in [`ipc_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedCommand {
    OpenSettings,
}

impl QueuedCommand {
    fn serialize(self) -> &'static str {
        match self {
            QueuedCommand::OpenSettings => "open-settings",
        }
    }

    fn parse(line: &str) -> Option<Self> {
        match line.trim() {
            "open-settings" => Some(QueuedCommand::OpenSettings),
            _ => None,
        }
    }
}

/// Client side: queue `open-settings` for the daemon (atomic temp+rename
/// so the daemon never reads a half-written file).
pub fn queue_open_settings() -> io::Result<()> {
    queue_at(&ipc_path(), QueuedCommand::OpenSettings)
}

fn queue_at(path: &Path, cmd: QueuedCommand) -> io::Result<()> {
    let tmp = path.with_extension("cmd.tmp");
    std::fs::write(&tmp, format!("{}\n", cmd.serialize()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Daemon side: take one queued command, if any. Always clears the file
/// when it exists so a request is delivered at most once.
pub fn take_queued_command() -> Option<QueuedCommand> {
    take_at(&ipc_path())
}

fn take_at(path: &Path) -> Option<QueuedCommand> {
    let content = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    content.lines().next().and_then(QueuedCommand::parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_path_is_validated_and_made_absolute() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.lua"), "return {}").unwrap();
        let cli = Cli::try_parse_from(["riced", "-p", dir.path().to_str().unwrap()]).unwrap();
        assert_eq!(
            cli.project_root().unwrap(),
            Some(dir.path().canonicalize().unwrap())
        );
    }

    #[test]
    fn named_example_resolves_from_repository_root() {
        let cli = Cli::try_parse_from(["riced", "-c", "example"]).unwrap();
        assert_eq!(
            cli.project_root().unwrap(),
            Some(Path::new("config/riced/example").canonicalize().unwrap())
        );
    }

    #[test]
    fn rejects_missing_projects_unsafe_names_and_conflicting_selectors() {
        assert!(
            Cli::try_parse_from(["riced", "-p", "/nonexistent/riced-project"])
                .unwrap()
                .project_root()
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["riced", "-c", "../example"])
                .unwrap()
                .project_root()
                .is_err()
        );
        assert!(Cli::try_parse_from(["riced", "-p", ".", "-c", "example"]).is_err());
    }

    #[test]
    fn parses_lua_demo_override() {
        assert_eq!(
            Cli::try_parse_from(["riced", "lua-demo", "--script", "/tmp/main.lua"])
                .unwrap()
                .command,
            Some(Commands::LuaDemo {
                script: Some(PathBuf::from("/tmp/main.lua"))
            })
        );
    }

    #[test]
    fn parses_open_settings_and_alias() {
        assert_eq!(
            Cli::try_parse_from(["riced", "open-settings"])
                .unwrap()
                .command,
            Some(Commands::OpenSettings)
        );
        assert_eq!(
            Cli::try_parse_from(["riced", "settings"]).unwrap().command,
            Some(Commands::OpenSettings)
        );
        assert_eq!(Cli::try_parse_from(["riced"]).unwrap().command, None);
    }

    #[test]
    fn parses_extension_build_and_update() {
        assert_eq!(
            Cli::try_parse_from(["riced", "extension", "build", "hello", "--release"])
                .unwrap()
                .command,
            Some(Commands::Extension(ExtensionCommand::Build {
                name: Some("hello".into()),
                release: true,
                locked: false,
            }))
        );
        assert_eq!(
            Cli::try_parse_from(["riced", "extension", "update"])
                .unwrap()
                .command,
            Some(Commands::Extension(ExtensionCommand::Update {
                name: None,
                release: false,
                locked: false,
                framework: None,
                target_dir: None,
            }))
        );
    }

    #[test]
    fn parses_generate_theme_flags() {
        assert_eq!(
            Cli::try_parse_from(["riced", "generate-theme"])
                .unwrap()
                .command,
            Some(Commands::GenerateTheme {
                variant: None,
                set: false,
                templates: None,
                no_templates: false,
            })
        );
        assert_eq!(
            Cli::try_parse_from([
                "riced",
                "generate-theme",
                "--variant",
                "vibrant",
                "--set",
                "--templates",
                "/tmp/tpl"
            ])
            .unwrap()
            .command,
            Some(Commands::GenerateTheme {
                variant: Some("vibrant".to_string()),
                set: true,
                templates: Some(PathBuf::from("/tmp/tpl")),
                no_templates: false,
            })
        );
        assert_eq!(
            Cli::try_parse_from(["riced", "generate-theme", "--no-templates"])
                .unwrap()
                .command,
            Some(Commands::GenerateTheme {
                variant: None,
                set: false,
                templates: None,
                no_templates: true,
            })
        );
    }

    #[test]
    fn parses_apply_templates_flags() {
        assert_eq!(
            Cli::try_parse_from(["riced", "apply-templates"])
                .unwrap()
                .command,
            Some(Commands::ApplyTemplates {
                templates: None,
                name: None,
            })
        );
        assert_eq!(
            Cli::try_parse_from([
                "riced",
                "apply-templates",
                "--templates",
                "/tmp/tpl",
                "--name",
                "dracula"
            ])
            .unwrap()
            .command,
            Some(Commands::ApplyTemplates {
                templates: Some(PathBuf::from("/tmp/tpl")),
                name: Some("dracula".to_string()),
            })
        );
    }

    #[test]
    fn queue_take_roundtrip() {
        // Own subdir: tests run in parallel and each removes its dir at the end.
        let dir = std::env::temp_dir().join(format!("riced-test-{}-roundtrip", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("roundtrip.cmd");
        let _ = std::fs::remove_file(&path);
        assert_eq!(take_at(&path), None);
        queue_at(&path, QueuedCommand::OpenSettings).unwrap();
        assert_eq!(take_at(&path), Some(QueuedCommand::OpenSettings));
        // delivered at most once
        assert_eq!(take_at(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_command_is_dropped() {
        // Own subdir: tests run in parallel and each removes its dir at the end.
        let dir = std::env::temp_dir().join(format!("riced-test-{}-unknown", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("unknown.cmd");
        std::fs::write(&path, "bogus\n").unwrap();
        assert_eq!(take_at(&path), None);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
