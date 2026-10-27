//! Explicit app enablement; app names resolve to apps/<name>/app.lua.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AppConfig {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub props: std::collections::HashMap<String, crate::config::PropValue>,
}
impl AppConfig {
    pub fn identity(&self) -> &str {
        if self.id.is_empty() {
            &self.name
        } else {
            &self.id
        }
    }
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
pub(crate) fn deserialize_apps<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<AppConfig>, D::Error> {
    let apps = Vec::<AppConfig>::deserialize(deserializer)?;
    let mut ids = HashSet::new();
    for app in &apps {
        if !valid_name(&app.name) || (!app.id.is_empty() && !valid_name(&app.id)) {
            return Err(serde::de::Error::custom(
                "app name and id must contain only letters, digits, '_' or '-'",
            ));
        }
        if !ids.insert(app.identity()) {
            return Err(serde::de::Error::custom(format!(
                "duplicate app instance {:?}; assign distinct ids",
                app.identity()
            )));
        }
    }
    Ok(apps)
}

/// One compiled-in or executable extension enabled for a project.
///
/// `path` locates the extension crate relative to the project root and
/// defaults to `extensions/<name>` when omitted. A compiled-in crate must
/// expose `pub fn register(&mut Registry, &[&str]) -> Result<(), Error>` and is
/// linked into the shell. An executable extension sets `process` instead and is
/// not linked; its `path` is only used by `riced extension build` to compile the
/// crate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionConfig {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessConfig>,
}

/// Executable service capabilities captured by a generated shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessConfig {
    pub argv: Vec<String>,
    pub service: String,
    pub commands: Vec<String>,
    #[serde(default = "process_timeout")]
    pub timeout_ms: u64,
}
fn process_timeout() -> u64 {
    30_000
}

fn valid_extension_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

pub(crate) fn deserialize_extensions<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ExtensionConfig>, D::Error> {
    let extensions = Vec::<ExtensionConfig>::deserialize(deserializer)?;
    let mut names = HashSet::new();
    for extension in &extensions {
        if !valid_extension_name(&extension.name) {
            return Err(serde::de::Error::custom(format!(
                "extension name {:?} must start with a lowercase letter and contain only lowercase letters, digits, or '_'",
                extension.name
            )));
        }
        if extension.path.contains('\0') {
            return Err(serde::de::Error::custom(
                "extension path must not contain NUL bytes",
            ));
        }
        if let Some(process) = &extension.process {
            let mut commands = HashSet::new();
            if !valid_extension_name(&process.service)
                || process.commands.len() > 64
                || process
                    .commands
                    .iter()
                    .any(|name| !valid_extension_name(name) || !commands.insert(name))
                || process.argv.is_empty()
                || process.argv.len() > 64
                || process.argv[0].is_empty()
                || process
                    .argv
                    .iter()
                    .any(|s| s.contains('\0') || s.len() > 4096)
                || !(10..=300_000).contains(&process.timeout_ms)
            {
                return Err(serde::de::Error::custom(
                    "invalid process extension options",
                ));
            }
        }
        if !names.insert(extension.name.as_str()) {
            return Err(serde::de::Error::custom(format!(
                "duplicate extension {:?}",
                extension.name
            )));
        }
    }
    Ok(extensions)
}

#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub root: PathBuf,
}
impl Project {
    pub fn installed() -> Self {
        Self {
            root: crate::config::config_path()
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf(),
        }
    }
    /// Where an app keeps owned data (`state/`, generated documents).
    ///
    /// A folder app is `apps/<name>/`; a single-file app is `apps/<name>.lua`
    /// and gets this directory only for data, never for its entry point.
    pub fn app_root(&self, name: &str) -> PathBuf {
        if name == "root" && self.root.join("root.lua").is_file() {
            return self.root.clone();
        }
        self.root.join("apps").join(name)
    }
    /// `apps/<name>/app.lua`, or `apps/<name>.lua` when that file exists.
    pub fn source(&self, name: &str) -> PathBuf {
        if name == "root" && self.root.join("root.lua").is_file() {
            return self.root.join("root.lua");
        }
        let folder = self.app_root(name).join("app.lua");
        if folder.is_file() {
            return folder;
        }
        self.root.join("apps").join(format!("{name}.lua"))
    }
    /// Snapshot code, not generated data files, so saving preferences cannot reload the app.
    pub fn sources(&self, name: &str) -> Result<Vec<(PathBuf, String)>, String> {
        fn visit(path: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<(), String> {
            let mut entries = std::fs::read_dir(path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_dir() {
                    visit(&path, out)?;
                } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "lua") {
                    let source = std::fs::read_to_string(&path)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    out.push((path, source));
                }
            }
            Ok(())
        }
        let entry = self.source(name);
        let source =
            std::fs::read_to_string(&entry).map_err(|e| format!("{}: {e}", entry.display()))?;
        let mut sources = vec![(entry, source)];
        let state = self.app_root(name).join("state.lua");
        match std::fs::read_to_string(&state) {
            Ok(source) => sources.push((state, source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", state.display())),
        }
        // App-local components, then the project's shared `layers/`, which is
        // where a single-file app keeps its layer modules.
        for directory in [
            self.app_root(name).join("components"),
            self.root.join("layers"),
            self.app_root(name).join("shell"),
        ] {
            match std::fs::metadata(&directory) {
                Ok(_) => visit(&directory, &mut sources)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", directory.display())),
            }
        }
        Ok(sources)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_config_is_strict_and_roundtrips() {
        let config = crate::config::test_parse_config(
            "return {apps={{name='bar',id='primary',output='DP-1',props={height=32}}}}",
        )
        .unwrap();
        let text = crate::config::test_format_config(&config).unwrap();
        assert_eq!(crate::config::test_parse_config(&text).unwrap(), config);
        for source in [
            "return {apps={{name='../bar'}}}",
            "return {apps={{name='bar'},{name='bar'}}}",
            "return {apps={{name='bar',typo=true}}}",
            "return {apps={{}}}",
        ] {
            assert!(
                crate::config::test_parse_config(source).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn process_extensions_may_name_their_crate_for_building() {
        // `path` locates the crate `riced extension build` compiles; it does not
        // link the extension into the shell.
        let config = crate::config::test_parse_config(
            "return {extensions={{name='hello',path='extensions/hello',process={argv={'bin'},service='hello',commands={'greet'},timeout_ms=5000}}}}",
        )
        .unwrap();
        let extension = &config.extensions[0];
        assert_eq!(extension.path, "extensions/hello");
        assert_eq!(extension.process.as_ref().unwrap().service, "hello");
        let text = crate::config::test_format_config(&config).unwrap();
        assert_eq!(crate::config::test_parse_config(&text).unwrap(), config);
        for source in [
            "return {extensions={{name='hello',path='a\\0b',process={argv={'bin'},service='hello',commands={'greet'},timeout_ms=5000}}}}",
            "return {extensions={{name='hello',process={argv={},service='hello',commands={'greet'},timeout_ms=5000}}}}",
        ] {
            assert!(
                crate::config::test_parse_config(source).is_err(),
                "{source}"
            );
        }
    }
}
