use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "riced-icons-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn file(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::new(8, 8).save(&path).unwrap();
        path
    }
    fn theme(&self, name: &str, inherits: &str) {
        let root = self.0.join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.theme"), format!(
            "[Icon Theme]\nName={name}\nInherits={inherits}\nDirectories=24x24/apps,48x48/apps,scalable/apps\n\n[24x24/apps]\nSize=24\nType=Fixed\n\n[48x48/apps]\nSize=48\nType=Fixed\n\n[scalable/apps]\nSize=64\nType=Scalable\nMinSize=64\nMaxSize=256\n"
        )).unwrap();
    }
    fn resolver(&self, theme: &str) -> Resolver {
        Resolver::new(theme, vec![self.0.clone()])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn previews_resolve_the_requested_theme_without_changing_the_active_theme() {
    let fixture = Fixture::new();
    fixture.theme("Active", "");
    fixture.theme("Preview", "");
    let active = fixture.file("Active/48x48/apps/firefox.png");
    let preview = fixture.file("Preview/48x48/apps/firefox.png");
    let mut resolver = fixture.resolver("Active");
    assert_eq!(resolver.resolve("firefox", 48, &[]), Some(active.clone()));
    assert_eq!(
        resolver.resolve_in_theme("Preview", "firefox", 48, &[]),
        Some(preview)
    );
    assert_eq!(resolver.resolve("firefox", 48, &[]), Some(active));
}

#[test]
fn selected_theme_and_requested_size_determine_the_icon() {
    let fixture = Fixture::new();
    fixture.theme("Chosen", "");
    fixture.theme("AlphabeticallyFirst", "");
    fixture.file("AlphabeticallyFirst/24x24/apps/app.png");
    let small = fixture.file("Chosen/24x24/apps/app.png");
    let large = fixture.file("Chosen/48x48/apps/app.png");
    let scalable = fixture.file("Chosen/scalable/apps/app.png");
    let mut resolver = fixture.resolver("Chosen");
    assert_eq!(resolver.resolve("app", 24, &[]), Some(small));
    assert_eq!(resolver.resolve("app", 48, &[]), Some(large));
    assert_eq!(resolver.resolve("app", 128, &[]), Some(scalable));
}

#[test]
fn inheritance_cycles_terminate_and_hicolor_and_flat_files_are_fallbacks() {
    let fixture = Fixture::new();
    fixture.theme("Child", "Parent");
    fixture.theme("Parent", "Child");
    fixture.theme("hicolor", "");
    let inherited = fixture.file("Parent/24x24/apps/inherited.png");
    let fallback = fixture.file("hicolor/24x24/apps/fallback.png");
    let flat = fixture.file("legacy.png");
    let mut resolver = fixture.resolver("Child");
    assert_eq!(resolver.resolve("inherited", 24, &[]), Some(inherited));
    assert_eq!(resolver.resolve("fallback", 24, &[]), Some(fallback));
    assert_eq!(resolver.resolve("legacy.png", 24, &[]), Some(flat));
    assert_eq!(resolver.resolve("absent", 24, &[]), None);
}

#[test]
fn private_application_paths_do_not_share_cached_hits_or_misses() {
    let fixture = Fixture::new();
    let first = fixture.file("first/steam_tray_mono.png");
    let second = fixture.file("second/steam_tray_mono.png");
    let mut resolver = fixture.resolver("hicolor");
    assert_eq!(resolver.resolve("steam_tray_mono", 24, &[]), None);
    assert_eq!(
        resolver.resolve("steam_tray_mono", 24, &[fixture.0.join("first")]),
        Some(first.clone())
    );
    assert_eq!(
        resolver.resolve("steam_tray_mono", 24, &[fixture.0.join("second")]),
        Some(second)
    );
    assert_eq!(
        resolver.resolve(first.to_str().unwrap(), 48, &[]),
        Some(first)
    );
}

#[test]
fn theme_changes_invalidate_both_positive_and_negative_cache_entries() {
    let fixture = Fixture::new();
    fixture.theme("First", "");
    fixture.theme("Second", "");
    let first = fixture.file("First/24x24/apps/app.png");
    let second = fixture.file("Second/24x24/apps/app.png");
    let only_second = fixture.file("Second/24x24/apps/other.png");
    let mut resolver = fixture.resolver("First");
    assert_eq!(resolver.resolve("app", 24, &[]), Some(first));
    assert_eq!(resolver.resolve("other", 24, &[]), None);
    assert!(resolver.set_theme("Second"));
    assert_eq!(resolver.resolve("app", 24, &[]), Some(second));
    assert_eq!(resolver.resolve("other", 24, &[]), Some(only_second));
}

#[test]
fn indexed_themes_can_be_split_across_search_roots_and_use_scaled_sizes() {
    let fixture = Fixture::new();
    let user = fixture.0.join("user");
    let system = fixture.0.join("system");
    std::fs::create_dir_all(user.join("Split")).unwrap();
    std::fs::write(user.join("Split/index.theme"), "[Icon Theme]\nDirectories=normal\nScaledDirectories=hidpi\n[normal]\nSize=24\nType=Fixed\n[hidpi]\nSize=24\nScale=2\nType=Fixed\n").unwrap();
    let normal = fixture.file("system/Split/normal/app.png");
    let hidpi = fixture.file("system/Split/hidpi/app.png");
    let mut resolver = Resolver::new("Split", vec![user, system]);
    assert_eq!(resolver.resolve("app", 24, &[]), Some(normal));
    assert_eq!(resolver.resolve("app", 48, &[]), Some(hidpi));
}

#[test]
fn cursor_only_manifests_are_not_icon_themes() {
    let fixture = Fixture::new();
    let cursor = fixture.0.join("default");
    std::fs::create_dir_all(&cursor).unwrap();
    std::fs::write(
        cursor.join("index.theme"),
        "[Icon Theme]\nName=default\nInherits=hicolor\n",
    )
    .unwrap();
    let icons = fixture.0.join("Papirus");
    std::fs::create_dir_all(&icons).unwrap();
    std::fs::write(
        icons.join("index.theme"),
        "[Icon Theme]\nName=Papirus\nDirectories=24x24/apps\n\n[24x24/apps]\nSize=24\nType=Fixed\n",
    )
    .unwrap();
    assert!(!declares_icons(
        &index(&cursor.join("index.theme")).unwrap()
    ));
    assert!(declares_icons(&index(&icons.join("index.theme")).unwrap()));
}

#[test]
fn index_built_after_a_miss_still_resolves_own_and_inherited_icons() {
    let fixture = Fixture::new();
    fixture.theme("Child", "Parent");
    fixture.theme("Parent", "");
    let own = fixture.file("Child/24x24/apps/own.png");
    let inherited = fixture.file("Parent/24x24/apps/inherited.png");
    let mut resolver = fixture.resolver("Child");
    assert_eq!(resolver.resolve("absent", 24, &[]), None);
    assert_eq!(resolver.resolve("own", 24, &[]), Some(own));
    assert_eq!(resolver.resolve("inherited", 24, &[]), Some(inherited));
    assert_eq!(resolver.resolve("still-absent", 24, &[]), None);
}

#[test]
fn malformed_names_and_theme_directory_traversal_do_not_resolve() {
    let fixture = Fixture::new();
    fixture.theme("Chosen", "");
    fixture.file("Chosen/24x24/apps/app.png");
    let mut resolver = fixture.resolver("Chosen");
    for name in ["", ".", "..", "../app", "folder/app", "folder\\app"] {
        assert_eq!(resolver.resolve(name, 24, &[]), None, "{name}");
    }
    resolver.set_theme("../Chosen");
    assert_eq!(resolver.resolve("app", 24, &[]), None);
}
