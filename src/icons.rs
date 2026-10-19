//! Shared freedesktop icon lookup for application icons, tray items, and notifications.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

const EXTENSIONS: [&str; 3] = ["png", "svg", "xpm"];
const CACHE_CAP: usize = 1024;

fn base_dirs() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".icons"));
    }
    if let Some(data) = dirs::data_dir() {
        bases.push(data.join("icons"));
    }
    let data =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in data.split(':').filter(|dir| !dir.is_empty()) {
        bases.push(Path::new(dir).join("icons"));
        bases.push(Path::new(dir).join("pixmaps"));
    }
    bases.push(PathBuf::from("/usr/share/pixmaps"));
    bases
}

fn simple_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// Icon theme directory names, suitable for persisting in
/// `[theme].icon-theme`. Cursor-only themes (which declare an `Inherits` but
/// no icon `Directories`) are excluded; they contribute no icons of their own.
pub(crate) fn available_themes() -> Vec<String> {
    let scan = || {
        let mut names = BTreeSet::from(["hicolor".to_owned()]);
        for base in base_dirs() {
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            for entry in entries.flatten() {
                let manifest = entry.path().join("index.theme");
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !manifest.is_file() || !simple_name(&name) {
                    continue;
                }
                if index(&manifest).is_some_and(|sections| declares_icons(&sections)) {
                    names.insert(name);
                }
            }
        }
        names.into_iter().collect::<Vec<_>>()
    };
    let Ok(mut cache) = THEMES.lock() else {
        return scan();
    };
    if let Some((at, names)) = cache.as_ref()
        && at.elapsed() < THEME_TTL
    {
        return names.clone();
    }
    let names = scan();
    *cache = Some((std::time::Instant::now(), names.clone()));
    names
}

const THEME_TTL: std::time::Duration = std::time::Duration::from_secs(30);
type ThemeCache = Mutex<Option<(std::time::Instant, Vec<String>)>>;
static THEMES: LazyLock<ThemeCache> = LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    theme: String,
    name: String,
    size: u32,
    extra: Vec<PathBuf>,
}

/// Filenames present in a directory, read once and reused so a miss does not
/// stat every extension across every search root on every lookup.
type Listings = HashMap<PathBuf, Arc<HashSet<OsString>>>;

/// Parsed `index.theme` plus the search roots that actually contain the theme
/// directory. Memoized so a miss neither re-reads the manifest nor probes
/// every root for every icon name.
struct ThemeMeta {
    sections: Sections,
    roots: Vec<usize>,
    directories: Vec<String>,
}

pub(crate) struct Resolver {
    theme: String,
    bases: Vec<PathBuf>,
    cache: HashMap<Key, Option<PathBuf>>,
    listings: Listings,
    themes: HashMap<String, Option<Arc<ThemeMeta>>>,
    /// Complete set of icon stems per theme, built lazily only after a miss so
    /// themes whose icons hit early never pay for a full directory walk.
    indexes: HashMap<String, Arc<HashSet<String>>>,
}

impl Resolver {
    /// Isolated lookup caches keep background previews off the live resolver lock.
    pub(crate) fn for_previews() -> Self {
        Self::new("hicolor", base_dirs())
    }

    fn new(theme: &str, bases: Vec<PathBuf>) -> Self {
        Self {
            theme: theme.into(),
            bases,
            cache: HashMap::new(),
            listings: HashMap::new(),
            themes: HashMap::new(),
            indexes: HashMap::new(),
        }
    }

    fn set_theme(&mut self, theme: &str) -> bool {
        if self.theme == theme {
            return false;
        }
        self.theme = theme.to_owned();
        self.cache.clear();
        self.listings.clear();
        self.themes.clear();
        self.indexes.clear();
        true
    }

    fn resolve(&mut self, name: &str, size: u32, extra: &[PathBuf]) -> Option<PathBuf> {
        let theme = self.theme.clone();
        self.resolve_in_theme(&theme, name, size, extra)
    }

    pub(crate) fn resolve_in_theme(
        &mut self,
        theme: &str,
        name: &str,
        size: u32,
        extra: &[PathBuf],
    ) -> Option<PathBuf> {
        if Path::new(name).is_absolute() {
            return Path::new(name).is_file().then(|| PathBuf::from(name));
        }
        if !simple_name(name) {
            return None;
        }
        let key = Key {
            theme: theme.into(),
            name: name.into(),
            size: size.max(1),
            extra: extra.to_vec(),
        };
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        // Private application roots stay on the stat path so freshly dropped
        // icons are seen without a restart; they also never share name-only
        // cache entries with themed results.
        let found = extra
            .iter()
            .filter(|root| root.is_absolute())
            .find_map(|root| {
                icon_file(root, name).or_else(|| {
                    find_theme_stat(
                        name,
                        key.size,
                        theme,
                        std::slice::from_ref(root),
                        &mut HashSet::new(),
                    )
                })
            })
            .or_else(|| self.find_theme(name, key.size, theme, &mut HashSet::new()))
            .or_else(|| {
                (theme != "hicolor")
                    .then(|| self.find_theme(name, key.size, "hicolor", &mut HashSet::new()))
                    .flatten()
            })
            .or_else(|| self.bases.iter().find_map(|root| icon_file(root, name)));
        if self.cache.len() >= CACHE_CAP {
            self.cache.clear();
        }
        self.cache.insert(key, found.clone());
        found
    }

    /// Directory contents for `dir`, read once and shared with the theme index.
    fn listing(&mut self, dir: &Path) -> Arc<HashSet<OsString>> {
        self.listings
            .entry(dir.to_owned())
            .or_insert_with(|| {
                let files: HashSet<OsString> = std::fs::read_dir(dir)
                    .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
                    .unwrap_or_default();
                Arc::new(files)
            })
            .clone()
    }

    fn theme_meta(&mut self, theme: &str) -> Option<Arc<ThemeMeta>> {
        if !simple_name(theme) {
            return None;
        }
        if let Some(hit) = self.themes.get(theme) {
            return hit.clone();
        }
        let Some(sections) = self
            .bases
            .iter()
            .find_map(|root| index(&root.join(theme).join("index.theme")))
        else {
            self.themes.insert(theme.to_owned(), None);
            return None;
        };
        let roots: Vec<usize> = self
            .bases
            .iter()
            .enumerate()
            .filter(|(_, root)| root.join(theme).is_dir())
            .map(|(index, _)| index)
            .collect();
        let directories = theme_directories(&sections);
        let meta = Arc::new(ThemeMeta {
            sections,
            roots,
            directories,
        });
        self.themes.insert(theme.to_owned(), Some(meta.clone()));
        Some(meta)
    }

    /// Walk a theme's size directories in best-fit order. `None` proves the
    /// theme holds no such icon, which is where the name index is earned.
    fn scan_theme(
        &mut self,
        meta: &ThemeMeta,
        theme: &str,
        size: u32,
        name: &str,
    ) -> Option<PathBuf> {
        if meta.directories.is_empty() {
            return None;
        }
        let mut directories: Vec<(u32, &str)> = meta
            .directories
            .iter()
            .map(|dir| {
                let distance = meta
                    .sections
                    .get(dir)
                    .map_or(u32::MAX, |values| distance(values, size));
                (distance, dir.as_str())
            })
            .collect();
        directories.sort_by_key(|(distance, _)| *distance);
        for (_, dir) in directories {
            for &root in &meta.roots {
                let path = self.bases[root].join(theme).join(dir);
                let files = self.listing(&path);
                if let Some(found) = match_icon(&files, &path, name) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Collect every icon stem a theme provides, reusing cached listings.
    fn build_index(&mut self, meta: &ThemeMeta, theme: &str) -> Arc<HashSet<String>> {
        let mut names = HashSet::new();
        for &root in &meta.roots {
            for dir in &meta.directories {
                let path = self.bases[root].join(theme).join(dir);
                for file in self.listing(&path).iter() {
                    if let Some(stem) = icon_stem(file) {
                        names.insert(stem);
                    }
                }
            }
        }
        Arc::new(names)
    }

    fn find_theme(
        &mut self,
        name: &str,
        size: u32,
        theme: &str,
        visited: &mut HashSet<String>,
    ) -> Option<PathBuf> {
        if !visited.insert(theme.to_owned()) || visited.len() > 64 {
            return None;
        }
        let meta = self.theme_meta(theme)?;
        let stem = icon_stem(OsStr::new(name)).unwrap_or_else(|| name.to_owned());
        match self.indexes.get(theme).cloned() {
            // A complete index proves absence without a directory walk.
            Some(names) => {
                if names.contains(&stem)
                    && let Some(found) = self.scan_theme(&meta, theme, size, name)
                {
                    return Some(found);
                }
            }
            None => {
                if let Some(found) = self.scan_theme(&meta, theme, size, name) {
                    return Some(found);
                }
                let names = self.build_index(&meta, theme);
                self.indexes.insert(theme.to_owned(), names);
            }
        }
        for parent in meta
            .sections
            .get("Icon Theme")
            .into_iter()
            .flat_map(|header| header.get("Inherits"))
            .flat_map(|parents| parents.split(','))
            .map(str::trim)
        {
            if let Some(found) = self.find_theme(name, size, parent, visited) {
                return Some(found);
            }
        }
        None
    }
}

static RESOLVER: LazyLock<Mutex<Resolver>> =
    LazyLock::new(|| Mutex::new(Resolver::new("hicolor", base_dirs())));

/// Set the shared theme; invalidate all positive and negative lookup entries.
pub(crate) fn set_theme(theme: &str) -> bool {
    RESOLVER
        .lock()
        .is_ok_and(|mut resolver| resolver.set_theme(theme))
}

/// Resolve a DesktopEntry/SNI icon name or an absolute filename. `size` is the
/// desired pixel size. Extra roots (e.g. SNI `IconThemePath`) precede the theme.
pub(crate) fn resolve(name: &str, size: u32, extra: &[PathBuf]) -> Option<PathBuf> {
    RESOLVER.lock().ok()?.resolve(name, size, extra)
}

pub(crate) fn image(
    name: &str,
    size: u32,
    extra: &[PathBuf],
) -> Option<iced::widget::image::Handle> {
    crate::notify::images::image_file(&resolve(name, size, extra)?.to_string_lossy())
}

fn icon_file(dir: &Path, name: &str) -> Option<PathBuf> {
    if Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext))
    {
        let path = dir.join(name);
        return path.is_file().then_some(path);
    }
    EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|path| path.is_file())
}

/// Match a theme icon name against a memoized directory listing instead of
/// stat-ing every extension.
fn match_icon(files: &HashSet<OsString>, dir: &Path, name: &str) -> Option<PathBuf> {
    if Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext))
    {
        return files.contains(OsStr::new(name)).then(|| dir.join(name));
    }
    EXTENSIONS
        .iter()
        .map(|ext| format!("{name}.{ext}"))
        .find(|file| files.contains(OsStr::new(file)))
        .map(|file| dir.join(file))
}

/// Icon name stem of a theme file, or `None` for non-icon files.
fn icon_stem(file: &OsStr) -> Option<String> {
    let path = Path::new(file);
    let ext = path.extension()?.to_str()?;
    if !EXTENSIONS.contains(&ext) {
        return None;
    }
    path.file_stem()?.to_str().map(str::to_owned)
}

/// Whether a manifest declares icon directories of its own. Cursor-only
/// themes declare `Inherits` but no `Directories`.
fn declares_icons(sections: &Sections) -> bool {
    sections
        .get("Icon Theme")
        .and_then(|header| header.get("Directories"))
        .is_some_and(|directories| !directories.trim().is_empty())
}

/// Validated `Directories` keys from a parsed `index.theme`.
fn theme_directories(sections: &Sections) -> Vec<String> {
    let Some(header) = sections.get("Icon Theme") else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    for key in ["Directories", "ScaledDirectories"] {
        for dir in header.get(key).into_iter().flat_map(|dirs| dirs.split(',')) {
            let dir = dir.trim();
            if dir.is_empty()
                || !Path::new(dir)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
            {
                continue;
            }
            if sections.get(dir).is_some() {
                directories.push(dir.to_owned());
            }
        }
    }
    directories
}

type Sections = HashMap<String, HashMap<String, String>>;

fn index(path: &Path) -> Option<Sections> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut sections = Sections::new();
    let mut section = String::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
    {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(section.clone())
                .or_default()
                .insert(key.trim().into(), value.trim().into());
        }
    }
    Some(sections)
}

/// Stat-based theme search for private `IconThemePath` roots, which are
/// mutable and therefore never memoized.
fn find_theme_stat(
    name: &str,
    size: u32,
    theme: &str,
    bases: &[PathBuf],
    visited: &mut HashSet<String>,
) -> Option<PathBuf> {
    if !simple_name(theme) || !visited.insert(theme.to_owned()) || visited.len() > 64 {
        return None;
    }
    let sections = bases
        .iter()
        .find_map(|root| index(&root.join(theme).join("index.theme")))?;
    let mut directories: Vec<(u32, String)> = theme_directories(&sections)
        .into_iter()
        .map(|dir| {
            let distance = sections
                .get(&dir)
                .map_or(u32::MAX, |values| distance(values, size));
            (distance, dir)
        })
        .collect();
    directories.sort_by_key(|(distance, _)| *distance);
    for (_, dir) in directories {
        for base in bases {
            if let Some(path) = icon_file(&base.join(theme).join(&dir), name) {
                return Some(path);
            }
        }
    }
    for parent in sections
        .get("Icon Theme")
        .into_iter()
        .flat_map(|header| header.get("Inherits"))
        .flat_map(|parents| parents.split(','))
        .map(str::trim)
    {
        if let Some(path) = find_theme_stat(name, size, parent, bases, visited) {
            return Some(path);
        }
    }
    None
}

fn distance(values: &HashMap<String, String>, requested: u32) -> u32 {
    let number = |key: &str, default| {
        values
            .get(key)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(default)
    };
    let size = number("Size", 48);
    let scale = number("Scale", 1).max(1);
    let (min, max) = match values
        .get("Type")
        .map(String::as_str)
        .unwrap_or("Threshold")
    {
        "Fixed" => (size, size),
        "Scalable" => (number("MinSize", size), number("MaxSize", size)),
        _ => (
            size.saturating_sub(number("Threshold", 2)),
            size.saturating_add(number("Threshold", 2)),
        ),
    };
    let min = min.saturating_mul(scale);
    let max = max.saturating_mul(scale).max(min);
    requested.abs_diff(requested.clamp(min, max))
}

#[cfg(test)]
mod tests;
