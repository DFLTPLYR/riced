use clap::{Parser, Subcommand};
use std::io;
use std::path::{Path, PathBuf};

/// `riced` command line. No subcommand = run the Wayland shell daemon.
/// Subcommands are single-shot clients that queue a request for the
/// running daemon through a command file (see [`ipc_path`]).
#[derive(Debug, Parser)]
#[command(name = "riced", about = "Riced Wayland shell", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Commands {
    /// Ask the running daemon to open the Settings panel
    #[command(alias = "settings")]
    OpenSettings,
}

/// Parse `std::env::args`. Returns the requested client command, if any.
pub fn parse() -> Option<Commands> {
    Cli::parse().command
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
