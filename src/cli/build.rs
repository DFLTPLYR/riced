//! Generate and build a project shell with native and executable services.
//!
//! The authored project only lists extensions in `config.lua`. This module
//! writes a small Cargo package under `.riced/` whose `main` registers those
//! crates and calls `riced::run_project`. The generated `Cargo.lock` is kept
//! beside it so `--locked` builds are reproducible.
use crate::config;
use std::{
    collections::HashSet,
    fmt::Write as _,
    path::{Component, Path, PathBuf},
    process::Command,
};

pub(crate) struct Options {
    pub release: bool,
    pub locked: bool,
    pub framework: Option<PathBuf>,
    pub target_dir: Option<PathBuf>,
}

#[derive(Debug)]
struct Extension {
    name: String,
    package: String,
    ident: String,
    path: PathBuf,
}

#[derive(Debug)]
struct Plan {
    project: PathBuf,
    generated: PathBuf,
    package: String,
    framework: PathBuf,
    extensions: Vec<Extension>,
    processes: Vec<crate::framework::ExtensionConfig>,
}

pub(crate) fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let project = resolve_project()?;
    let config_path = config::config_path_in(&project);
    if !config_path.is_file() {
        return Err(format!("no config.lua in {}", project.display()).into());
    }
    std::fs::read_to_string(&config_path)
        .map_err(|error| format!("{}: {error}", config_path.display()))?;
    let (config, _) = config::Config::load();
    if let Some((source, error)) = config::take_parse_error() {
        return Err(format!("{source}: {error}").into());
    }
    let plan = Plan::new(project, &config, options.framework.clone())?;
    plan.write()?;
    plan.build(&options)?;
    Ok(())
}

/// Resolve the project selected by `-p`/`-c`, the current directory, or the
/// installed config, and install it as the runtime project root.
pub(crate) fn resolve_project() -> Result<PathBuf, String> {
    let project = if let Some(root) = config::project_root() {
        root
    } else {
        let cwd = std::env::current_dir()
            .map_err(|error| format!("cannot read current directory: {error}"))?;
        if config::config_path_in(&cwd).is_file() {
            cwd
        } else {
            let installed = config::project_dir();
            if !config::config_path_in(&installed).is_file() {
                return Err(
                    "no config.lua in the current directory or the installed config directory; pass -p/--project"
                        .into(),
                );
            }
            installed
        }
    };
    let project = project
        .canonicalize()
        .map_err(|error| format!("{}: {error}", project.display()))?;
    if config::project_root().is_none() {
        config::set_project_root(project.clone())?;
    }
    Ok(project)
}

impl Plan {
    fn new(
        project: PathBuf,
        config: &config::Config,
        framework: Option<PathBuf>,
    ) -> Result<Self, String> {
        let framework = framework
            .or_else(|| std::env::var_os("RICED_FRAMEWORK").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        if !framework.exists() {
            return Err(format!(
                "framework source {} does not exist; pass --framework or set RICED_FRAMEWORK",
                framework.display()
            ));
        }
        let framework = framework
            .canonicalize()
            .map_err(|error| format!("{}: {error}", framework.display()))?;
        validate_framework(&framework)?;

        let mut names = HashSet::new();
        let mut extensions = Vec::new();
        let mut processes = Vec::new();
        for extension in &config.extensions {
            if !names.insert(extension.name.clone()) {
                return Err(format!("duplicate extension {:?}", extension.name));
            }
            if extension.process.is_some() {
                processes.push(extension.clone());
                continue;
            }
            let path = extension_path(&project, extension);
            let path = path
                .canonicalize()
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let manifest_path = path.join("Cargo.toml");
            let source = std::fs::read_to_string(&manifest_path)
                .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
            let manifest: toml::Value = toml::from_str(&source)
                .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
            let package = manifest
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "extension {:?}: {} has no package.name",
                        extension.name,
                        manifest_path.display()
                    )
                })?;
            let lib_name = manifest
                .get("lib")
                .and_then(|lib| lib.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or(package);
            let ident = crate_ident(lib_name).ok_or_else(|| {
                format!(
                    "extension {:?}: crate name {lib_name:?} is not a valid Rust identifier",
                    extension.name
                )
            })?;
            if !depends_on_sdk(&manifest) {
                return Err(format!(
                    "extension {:?} must depend on riced-extension-sdk",
                    extension.name
                ));
            }
            if !path.join("src/lib.rs").is_file() {
                return Err(format!(
                    "extension {:?}: {} has no src/lib.rs",
                    extension.name,
                    path.display()
                ));
            }
            extensions.push(Extension {
                name: extension.name.clone(),
                package: package.to_owned(),
                ident,
                path,
            });
        }
        extensions.sort_by(|a, b| a.name.cmp(&b.name));

        let generated = project.join(".riced");
        Ok(Self {
            package: shell_package(&project),
            project,
            generated,
            framework,
            extensions,
            processes,
        })
    }

    fn manifest(&self) -> String {
        let mut out = String::new();
        out.push_str("# Generated by `riced build` from config.lua; do not edit.\n\n");
        writeln!(out, "[package]").unwrap();
        writeln!(out, "name = {:?}", self.package).unwrap();
        writeln!(out, "version = \"0.1.0\"").unwrap();
        writeln!(out, "edition = \"2024\"").unwrap();
        writeln!(out, "publish = false").unwrap();
        out.push_str("\n[workspace]\n\n[dependencies]\n");
        let framework = relative(&self.generated, &self.framework);
        writeln!(out, "riced = {{ path = {:?} }}", path_text(&framework)).unwrap();
        for extension in &self.extensions {
            let path = relative(&self.generated, &extension.path);
            writeln!(
                out,
                "{} = {{ path = {:?} }}",
                extension.package,
                path_text(&path)
            )
            .unwrap();
        }
        out
    }

    fn main_source(&self) -> String {
        let mut out = String::new();
        out.push_str("// Generated by `riced build` from config.lua; do not edit.\n\n");
        out.push_str("fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
        out.push_str("    let mut registry = riced::Registry::default();\n");
        out.push_str("    let project = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\")).parent().expect(\"generated project\");\n");
        for extension in &self.processes {
            if let Some(process) = &extension.process {
                writeln!(out, "    riced::ProcessExtension {{ name: {:?}, descriptor: riced::Descriptor {{ name: {:?}, commands: &{:?} }}, argv: vec!{:?}.into_iter().map(String::from).collect(), directory: project.to_path_buf(), timeout: std::time::Duration::from_millis({}) }}.register(&mut registry, riced::builtin_service_topics())?;",
                    extension.name, process.service, process.commands, process.argv, process.timeout_ms).unwrap();
            }
        }
        for extension in &self.extensions {
            writeln!(
                out,
                "    {}::register(&mut registry, riced::builtin_service_topics())?;",
                extension.ident
            )
            .unwrap();
        }
        out.push_str("    riced::run_project(registry, project)\n");
        out.push_str("}\n");
        out
    }

    fn write(&self) -> Result<(), String> {
        let source_dir = self.generated.join("src");
        std::fs::create_dir_all(&source_dir)
            .map_err(|error| format!("{}: {error}", source_dir.display()))?;
        write_if_changed(&self.generated.join("Cargo.toml"), &self.manifest())?;
        write_if_changed(&source_dir.join("main.rs"), &self.main_source())
    }

    fn build(&self, options: &Options) -> Result<(), String> {
        let manifest = self.generated.join("Cargo.toml");
        let lockfile = self.generated.join("Cargo.lock");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        if options.locked {
            if !lockfile.is_file() {
                return Err(format!(
                    "{} is missing; run `riced build` without --locked first",
                    lockfile.display()
                ));
            }
        } else {
            let status = Command::new(&cargo)
                .arg("generate-lockfile")
                .arg("--manifest-path")
                .arg(&manifest)
                .status()
                .map_err(|error| format!("cannot run cargo: {error}"))?;
            if !status.success() {
                return Err(format!("cargo generate-lockfile failed: {status}"));
            }
        }

        let target = options
            .target_dir
            .clone()
            .unwrap_or_else(|| self.project.join("target").join("riced-shell"));
        let mut command = Command::new(&cargo);
        command
            .arg("build")
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .arg("--locked");
        if options.release {
            command.arg("--release");
        }
        let status = command
            .status()
            .map_err(|error| format!("cannot run cargo: {error}"))?;
        if !status.success() {
            return Err(format!("cargo build failed: {status}"));
        }

        let profile = if options.release { "release" } else { "debug" };
        let binary = target.join(profile).join(&self.package);
        println!("riced: built {}", binary.display());
        println!(
            "riced: run it from the project with `{} -p {}`",
            binary.display(),
            self.project.display()
        );
        Ok(())
    }
}

fn validate_framework(framework: &Path) -> Result<(), String> {
    let manifest_path = framework.join("Cargo.toml");
    let source = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: toml::Value =
        toml::from_str(&source).map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    if manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        != Some("riced")
    {
        return Err(format!(
            "{} is not the riced framework (package.name must be \"riced\")",
            manifest_path.display()
        ));
    }
    if !framework.join("src/lib.rs").is_file() {
        return Err(format!(
            "{} is not the riced framework (src/lib.rs is missing)",
            framework.display()
        ));
    }
    Ok(())
}

/// Locate an extension's crate directory: `path` when set, otherwise the
/// documented `extensions/<name>` default beside the project.
pub(crate) fn extension_path(
    project: &Path,
    extension: &crate::framework::ExtensionConfig,
) -> PathBuf {
    if extension.path.is_empty() {
        project.join("extensions").join(&extension.name)
    } else {
        let path = PathBuf::from(&extension.path);
        if path.is_absolute() {
            path
        } else {
            project.join(path)
        }
    }
}

fn depends_on_sdk(manifest: &toml::Value) -> bool {
    manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .is_some_and(|dependencies| {
            dependencies.contains_key("riced-extension-sdk")
                || dependencies.values().any(|dependency| {
                    dependency.get("package").and_then(toml::Value::as_str)
                        == Some("riced-extension-sdk")
                })
        })
}

fn crate_ident(package: &str) -> Option<String> {
    let ident = package.replace('-', "_");
    let mut chars = ident.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || is_keyword(&ident)
    {
        return None;
    }
    Some(ident)
}

fn is_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "try"
    )
}

fn shell_package(project: &Path) -> String {
    let name = project
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let mut sanitized = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            sanitized.push(character.to_ascii_lowercase());
        } else {
            sanitized.push('-');
        }
    }
    let sanitized = sanitized.trim_matches('-');
    let sanitized = if sanitized.is_empty() {
        "project"
    } else {
        sanitized
    };
    format!("riced-shell-{sanitized}")
}

fn relative(from_dir: &Path, to: &Path) -> PathBuf {
    let from = from_dir.components().collect::<Vec<_>>();
    let target = to.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut path = PathBuf::new();
    for _ in common..from.len() {
        path.push(Component::ParentDir.as_os_str());
    }
    for component in &target[common..] {
        path.push(component.as_os_str());
    }
    if path.as_os_str().is_empty() {
        path.push(".");
    }
    path
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    std::fs::write(path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn framework(root: &Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"riced\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
    }

    fn extension(root: &Path, name: &str) {
        let dir = root.join("extensions").join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"riced-extension-{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nriced-extension-sdk = {{ path = \"../../../crates/riced-extension-sdk\" }}\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
    }

    #[test]
    fn generates_registration_and_relative_paths() {
        let project = temp_dir();
        let framework_dir = temp_dir();
        framework(framework_dir.path());
        extension(project.path(), "demo");
        let config =
            crate::config::test_parse_config("return {extensions={{name='demo'}},apps={}}")
                .unwrap();
        let plan = Plan::new(
            project.path().canonicalize().unwrap(),
            &config,
            Some(framework_dir.path().to_path_buf()),
        )
        .unwrap();
        let manifest = plan.manifest();
        assert!(manifest.contains("name = \"riced-shell-"));
        assert!(manifest.contains("riced = { path = \""));
        assert!(manifest.contains("riced-extension-demo = { path = \""));
        assert!(!manifest.contains(project.path().to_str().unwrap()));
        let main = plan.main_source();
        assert!(main.contains("riced_extension_demo::register("));
        assert!(main.contains("riced::run_project(registry, project)"));
        assert!(main.contains("CARGO_MANIFEST_DIR"));
        assert!(!main.contains(project.path().to_str().unwrap()));
    }

    #[test]
    fn rejects_unknown_packages_and_missing_sdk_dependencies() {
        let project = temp_dir();
        let framework_dir = temp_dir();
        framework(framework_dir.path());
        extension(project.path(), "demo");
        let config = crate::config::test_parse_config(
            "return {extensions={{name='demo',path='../outside'}}}",
        )
        .unwrap();
        let error = Plan::new(
            project.path().canonicalize().unwrap(),
            &config,
            Some(framework_dir.path().to_path_buf()),
        )
        .unwrap_err();
        assert!(error.contains("No such file") || error.contains("outside"));

        let dir = project.path().join("extensions/demo");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"riced-extension-demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        let config =
            crate::config::test_parse_config("return {extensions={{name='demo'}}}").unwrap();
        let error = Plan::new(
            project.path().canonicalize().unwrap(),
            &config,
            Some(framework_dir.path().to_path_buf()),
        )
        .unwrap_err();
        assert!(error.contains("riced-extension-sdk"));
    }

    #[test]
    fn validates_config_names_and_duplicates() {
        assert!(crate::config::test_parse_config("return {extensions={{name='Bad'}}}").is_err());
        assert!(
            crate::config::test_parse_config("return {extensions={{name='demo'},{name='demo'}}}")
                .is_err()
        );
    }

    #[test]
    fn process_registration_is_generated_without_linking_the_executable() {
        let project = temp_dir();
        let framework_dir = temp_dir();
        framework(framework_dir.path());
        let config = crate::config::test_parse_config("return {extensions={{name='worker',process={argv={'./bin/worker','--stdio'},service='colors',commands={'generate'},timeout_ms=5000}}}}").unwrap();
        let saved = crate::config::test_format_config(&config).unwrap();
        assert_eq!(crate::config::test_parse_config(&saved).unwrap(), config);
        let plan = Plan::new(
            project.path().canonicalize().unwrap(),
            &config,
            Some(framework_dir.path().to_path_buf()),
        )
        .unwrap();
        assert!(plan.extensions.is_empty());
        assert!(!plan.manifest().contains("worker"));
        let main = plan.main_source();
        assert!(main.contains("riced::ProcessExtension"));
        assert!(main.contains("commands: &[\"generate\"]"));
        assert!(main.contains("argv: vec![\"./bin/worker\", \"--stdio\"]"));
        assert!(main.contains("from_millis(5000)"));
    }

    #[test]
    fn framework_example_registers_theme_gen_and_the_palette_worker_without_collision() {
        let config =
            crate::config::test_parse_config(include_str!("../../examples/config.lua")).unwrap();
        assert_eq!(config.extensions.len(), 5);
        let theme_gen = config
            .extensions
            .iter()
            .find(|extension| extension.name == "theme_gen")
            .expect("theme_gen extension");
        assert_eq!(theme_gen.path, "../extensions/theme-gen");
        assert!(theme_gen.process.is_none());
        let toplevels = config
            .extensions
            .iter()
            .find(|extension| extension.name == "wayland_toplevels")
            .expect("wayland toplevels extension");
        assert_eq!(toplevels.path, "../extensions/wayland-toplevels");
        assert!(toplevels.process.is_none());
        let workspaces = config
            .extensions
            .iter()
            .find(|extension| extension.name == "wayland_workspaces")
            .expect("wayland workspaces extension");
        assert_eq!(workspaces.path, "../extensions/wayland-workspaces");
        assert!(workspaces.process.is_none());
        let palette = config
            .extensions
            .iter()
            .find(|extension| extension.name == "palette_worker")
            .expect("palette worker extension");
        let process = palette.process.as_ref().unwrap();
        assert_eq!(process.service, "palette");
        assert_eq!(process.commands, ["generate"]);
        assert_eq!(
            process.argv,
            ["../target/debug/riced-extension-palette-worker"]
        );
        // A process extension still names its crate so `riced extension build`
        // can compile it.
        assert_eq!(palette.path, "../extensions/palette-worker");
        let hello = config
            .extensions
            .iter()
            .find(|extension| extension.name == "hello")
            .expect("hello extension");
        assert_eq!(hello.path, "extensions/hello");
        let hello_process = hello.process.as_ref().unwrap();
        assert_eq!(hello_process.service, "hello");
        assert_eq!(hello_process.commands, ["greet"]);
        assert_eq!(
            hello_process.argv,
            ["../target/debug/riced-extension-hello"]
        );
        assert!(config.apps.iter().any(|app| app.name == "bar"));
        assert!(config.apps.iter().any(|app| app.name == "palette"));
        assert!(config.apps.iter().any(|app| app.name == "theme_gen"));
        assert!(config.apps.iter().any(|app| app.name == "toplevels"));
        assert!(config.apps.iter().any(|app| app.name == "workspaces"));
        assert!(config.apps.iter().any(|app| app.name == "editors"));
        assert!(config.apps.iter().any(|app| app.name == "hello"));
        assert!(config.apps.iter().any(|app| app.name == "popup"));
        // The shell-level surface a real configuration sets, kept in the
        // example so the reference shell is not a reduced subset.
        assert_eq!(config.theme.name, "gruvbox");
        assert!(config.theme.darkmode);
        assert_eq!(
            config.animation.speed,
            crate::config::AnimationSpeed::Medium
        );
        assert_eq!(config.interaction.hold.milliseconds(), 250);
        assert_eq!(config.notifications.output, "mouse");
        assert_eq!(config.notifications.source, "notifications.lua");
        assert_eq!(config.notifications.timeout_ms, 5000);
    }

    #[test]
    fn process_config_rejects_unsafe_or_ambiguous_options() {
        for options in [
            "argv={},service='colors',commands={'generate'}",
            "argv={'worker'},service='Bad',commands={'generate'}",
            "argv={'worker'},service='colors',commands={'generate','generate'}",
            "argv={'worker'},service='colors',commands={'generate'},timeout_ms=0",
            "argv={'worker'},service='colors',commands={'generate'},unknown=true",
        ] {
            let source =
                format!("return {{extensions={{{{name='worker',process={{{options}}}}}}}}}");
            assert!(
                crate::config::test_parse_config(&source).is_err(),
                "{source}"
            );
        }
        // A process extension may name its crate for `riced extension build`;
        // that does not link it into the shell.
        let config = crate::config::test_parse_config(
            "return {extensions={{name='worker',path='../extensions/worker',process={argv={'worker'},service='colors',commands={'generate'}}}}}",
        )
        .unwrap();
        assert_eq!(config.extensions[0].path, "../extensions/worker");
        assert!(config.extensions[0].process.is_some());
    }

    #[test]
    fn relative_paths_are_stable() {
        assert_eq!(
            relative(Path::new("/a/b/.riced"), Path::new("/a/b/extensions/demo")),
            Path::new("../extensions/demo")
        );
        assert_eq!(
            relative(Path::new("/a/b/.riced"), Path::new("/a/b")),
            Path::new("..")
        );
        assert_eq!(
            relative(Path::new("/a/b"), Path::new("/a/b")),
            Path::new(".")
        );
    }

    #[test]
    fn crate_idents_and_package_names_are_sanitized() {
        assert_eq!(
            crate_ident("riced-extension-demo").as_deref(),
            Some("riced_extension_demo")
        );
        assert_eq!(crate_ident("9bad"), None);
        assert_eq!(crate_ident("type"), None);
        assert_eq!(
            shell_package(Path::new("/tmp/My Project!")),
            "riced-shell-my-project"
        );
    }
}
