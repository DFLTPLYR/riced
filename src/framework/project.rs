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
    pub fn app_root(&self, name: &str) -> PathBuf {
        self.root.join("apps").join(name)
    }
    pub fn source(&self, name: &str) -> PathBuf {
        self.app_root(name).join("app.lua")
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
        let components = self.app_root(name).join("components");
        match std::fs::metadata(&components) {
            Ok(_) => visit(&components, &mut sources)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", components.display())),
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
}
