//! Compile the extension crates a project enables.
//!
//! `riced extension build` compiles the crates listed in `config.lua`;
//! `riced extension update` compiles them and then rebuilds the generated shell
//! so compiled-in changes are linked. Executable extensions only need a rebuild
//! of their own binary followed by a restart, but the shell rebuild is harmless
//! and keeps one command for both kinds.
use crate::{cli::build, config, framework::ExtensionConfig};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) struct Options {
    /// Extension name; `None` compiles every extension in the project.
    pub name: Option<String>,
    pub release: bool,
    pub locked: bool,
    /// Rebuild the generated shell after compiling.
    pub shell: bool,
    pub framework: Option<PathBuf>,
    pub target_dir: Option<PathBuf>,
}

/// One extension crate selected for compilation.
#[derive(Debug, PartialEq)]
struct Target {
    name: String,
    directory: PathBuf,
}

pub(crate) fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let project = build::resolve_project()?;
    let config_path = project.join("config.lua");
    if !config_path.is_file() {
        return Err(format!("no config.lua at {}", config_path.display()).into());
    }
    let (config, _) = config::Config::load();
    if let Some((source, error)) = config::take_parse_error() {
        return Err(format!("{source}: {error}").into());
    }

    let targets = select(&project, &config.extensions, options.name.as_deref())?;
    let manifests: Vec<PathBuf> = targets.iter().map(manifest).collect::<Result<_, _>>()?;
    for (target, manifest) in targets.iter().zip(&manifests) {
        compile(target, manifest, &options)?;
    }
    if links_into_shell(&config.extensions, &targets) {
        if options.shell {
            if let Err(error) = build::run(build::Options {
                release: options.release,
                locked: options.locked,
                framework: options.framework,
                target_dir: options.target_dir,
            }) {
                // Printed plainly: the returned error is Debug-formatted, which
                // would escape a multi-line hint.
                eprintln!(
                    "riced: rebuilding the shell needs the native build dependencies \
                     (pkg-config, wayland, xkbcommon, vulkan); `riced extension build` does not"
                );
                return Err(error);
            }
        } else {
            println!(
                "riced: compiled {} extension(s); run `riced extension update` to link compiled-in changes",
                targets.len()
            );
        }
    } else {
        println!(
            "riced: compiled {} executable extension(s); restart the shell to use the new binaries",
            targets.len()
        );
    }
    Ok(())
}

/// Only compiled-in extensions are linked into the shell, so a selection of
/// executable extensions never needs a shell rebuild (or its build deps).
fn links_into_shell(extensions: &[ExtensionConfig], targets: &[Target]) -> bool {
    targets
        .iter()
        .any(|target| !is_executable(extensions, &target.name))
}

/// Extensions selected by `name`, or every extension when it is `None`.
///
/// Pure resolution: crate existence is checked by [`manifest`] before cargo
/// runs, so a wrong `path` fails before any build starts.
fn select(
    project: &Path,
    extensions: &[ExtensionConfig],
    name: Option<&str>,
) -> Result<Vec<Target>, String> {
    if let Some(name) = name
        && !extensions.iter().any(|extension| extension.name == name)
    {
        return Err(format!("config.lua enables no extension named {name:?}"));
    }
    let mut targets = Vec::new();
    for extension in extensions {
        if name.is_some_and(|name| extension.name != name) {
            continue;
        }
        targets.push(Target {
            name: extension.name.clone(),
            directory: build::extension_path(project, extension),
        });
    }
    if targets.is_empty() {
        return Err("config.lua enables no extensions".into());
    }
    Ok(targets)
}

fn is_executable(extensions: &[ExtensionConfig], name: &str) -> bool {
    extensions
        .iter()
        .any(|extension| extension.name == name && extension.process.is_some())
}

/// The crate manifest for a selected extension, which must already exist.
fn manifest(target: &Target) -> Result<PathBuf, String> {
    let manifest = target.directory.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(format!(
            "extension {:?}: no Cargo.toml at {}; set `path` to its crate directory",
            target.name,
            manifest.display()
        ));
    }
    Ok(manifest)
}

/// Compile one extension crate with the workspace's cargo.
///
/// The manifest path is used directly, so a crate inside the framework
/// workspace and a standalone project crate both work.
fn compile(target: &Target, manifest: &Path, options: &Options) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(&cargo);
    command.arg("build").arg("--manifest-path").arg(manifest);
    if options.release {
        command.arg("--release");
    }
    if options.locked {
        command.arg("--locked");
    }
    let status = command
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !status.success() {
        return Err(format!(
            "cargo build failed for extension {:?}: {status}",
            target.name
        ));
    }
    println!(
        "riced: compiled extension {:?} ({})",
        target.name,
        target.directory.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a config so the tests exercise the same validation the CLI sees.
    fn extensions(source: &str) -> Vec<ExtensionConfig> {
        crate::config::test_parse_config(&format!("return {{extensions={{{source}}}}}"))
            .unwrap()
            .extensions
    }

    #[test]
    fn selection_resolves_paths_defaults_and_names() {
        let project = Path::new("/project");
        let extensions = extensions(
            "{name='theme_gen',path='../extensions/theme-gen'},\
             {name='hello',process={argv={'bin'},service='hello',commands={'greet'},timeout_ms=5000}}",
        );
        let targets = select(project, &extensions, None).unwrap();
        assert_eq!(
            targets,
            vec![
                Target {
                    name: "theme_gen".into(),
                    directory: PathBuf::from("/project/../extensions/theme-gen"),
                },
                Target {
                    name: "hello".into(),
                    directory: PathBuf::from("/project/extensions/hello"),
                },
            ]
        );
        assert_eq!(
            select(project, &extensions, Some("hello")).unwrap().len(),
            1
        );
        assert!(select(project, &extensions, Some("missing")).is_err());
        assert!(select(project, &[], None).is_err());
        assert!(!is_executable(&extensions, "theme_gen"));
        assert!(is_executable(&extensions, "hello"));
        // A compiled-in selection needs the shell relinked; a process-only one
        // only needs its binaries rebuilt.
        assert!(links_into_shell(&extensions, &targets));
        assert!(!links_into_shell(
            &extensions,
            &select(project, &extensions, Some("hello")).unwrap()
        ));
    }

    #[test]
    fn selection_reports_a_missing_crate_directory() {
        let extensions = extensions(
            "{name='ghost',process={argv={'bin'},service='ghost',commands={'run'},timeout_ms=5000}}",
        );
        let targets = select(Path::new("/project"), &extensions, None).unwrap();
        let error = manifest(&targets[0]).unwrap_err();
        assert!(error.contains("no Cargo.toml"), "{error}");
        assert!(error.contains("extensions/ghost"), "{error}");
    }
}
