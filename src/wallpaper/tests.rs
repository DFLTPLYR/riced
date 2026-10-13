use super::*;
use image::{Rgba, RgbaImage};

#[test]
fn per_monitor_fits_use_native_pixels_with_opaque_black_margins() {
    use crate::config::{MonitorWallpaper, WallpaperFit};
    let dir = directory("monitor-fits");
    let original = RgbaImage::from_fn(4, 2, |x, y| {
        Rgba([(x * 40 + 30) as u8, (y * 60 + 40) as u8, 90, 255])
    });
    let (image, sources) = source(&dir, "pattern.png", original.clone());
    let spec = OutputSpec {
        key: "DP-1".into(),
        rect: (-10.0, 20.0, 4.0, 4.0),
        pixels: (8, 8),
    };
    for (fit, expected_rect) in [
        (WallpaperFit::Fit, (-10.0, 21.0, 4.0, 2.0)),
        (WallpaperFit::Expand, (-12.0, 20.0, 8.0, 4.0)),
        (WallpaperFit::Stretch, (-10.0, 20.0, 4.0, 4.0)),
        (WallpaperFit::Center, (-9.0, 21.5, 2.0, 1.0)),
    ] {
        let assignment = MonitorWallpaper {
            output: spec.key.clone(),
            path: image.path.clone(),
            fit,
        };
        let placement = raster::monitor_image(&spec, &assignment, &sources[&image.local_path()]);
        assert_eq!(
            (placement.x, placement.y, placement.width, placement.height),
            expected_rect
        );
        let recipe = BackgroundConfig {
            mode: WallpaperMode::PerMonitor,
            monitor: vec![assignment],
            ..Default::default()
        };
        let pixels = raster::compose_output(&spec, &recipe, &sources).unwrap();
        assert!(pixels.pixels().all(|p| p[3] == 255));
        match fit {
            WallpaperFit::Fit => {
                assert_eq!(pixels.get_pixel(0, 0).0, [0, 0, 0, 255]);
                assert_ne!(pixels.get_pixel(0, 2).0, [0, 0, 0, 255]);
                assert_eq!(pixels.get_pixel(0, 6).0, [0, 0, 0, 255]);
            }
            WallpaperFit::Center => {
                for y in 0..2 {
                    for x in 0..4 {
                        assert_eq!(pixels.get_pixel(x + 2, y + 3), original.get_pixel(x, y));
                    }
                }
                assert_eq!(pixels.get_pixel(1, 3).0, [0, 0, 0, 255]);
                assert_eq!(pixels.get_pixel(6, 3).0, [0, 0, 0, 255]);
            }
            _ => assert!(pixels.pixels().all(|p| p[2] == 90)),
        }
    }
    let (alpha, sources) = source(
        &dir,
        "alpha.png",
        RgbaImage::from_pixel(4, 2, Rgba([255, 0, 0, 128])),
    );
    let recipe = BackgroundConfig {
        mode: WallpaperMode::PerMonitor,
        monitor: vec![MonitorWallpaper {
            output: spec.key.clone(),
            path: alpha.path,
            fit: WallpaperFit::Stretch,
        }],
        ..Default::default()
    };
    let pixels = raster::compose_output(&spec, &recipe, &sources).unwrap();
    assert_eq!(pixels.get_pixel(0, 0).0, [188, 0, 0, 255]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn monitor_assignments_are_isolated_persisted_and_restored_by_output_name() {
    use crate::config::{MonitorWallpaper, WallpaperFit};
    let dir = directory("monitor-independent");
    let (green, _) = source(
        &dir,
        "green.png",
        RgbaImage::from_pixel(4, 2, Rgba([0, 255, 0, 255])),
    );
    let (blue, _) = source(
        &dir,
        "blue.png",
        RgbaImage::from_pixel(2, 4, Rgba([0, 0, 255, 255])),
    );
    let mut request = request(vec![]);
    request.images = BackgroundConfig {
        mode: WallpaperMode::PerMonitor,
        image: vec![BackgroundImage {
            path: "/missing/inactive-freeform.png".into(),
            ..Default::default()
        }],
        monitor: vec![
            MonitorWallpaper {
                output: "DP-1".into(),
                path: green.path.clone(),
                fit: WallpaperFit::Expand,
            },
            MonitorWallpaper {
                output: "DP-2".into(),
                path: blue.path.clone(),
                fit: WallpaperFit::Stretch,
            },
            MonitorWallpaper {
                output: "disconnected".into(),
                path: "/missing/disconnected.png".into(),
                fit: WallpaperFit::Fit,
            },
        ],
    };
    request.outputs.push((
        OutputId(2),
        OutputSpec {
            key: "DP-2".into(),
            rect: (0.0, 0.0, 2.0, 4.0),
            pixels: (2, 4),
        },
    ));
    request.outputs.push((
        OutputId(3),
        OutputSpec {
            key: "unassigned".into(),
            rect: (0.0, 0.0, 2.0, 2.0),
            pixels: (2, 2),
        },
    ));
    let cache = dir.join("cache");
    let batch = bake(&request, &cache).unwrap();
    for (id, color) in [
        (OutputId(1), [0, 255, 0, 255]),
        (OutputId(2), [0, 0, 255, 255]),
        (OutputId(3), [0, 0, 0, 255]),
    ] {
        assert!(
            image::open(&batch.tiles[&id].path)
                .unwrap()
                .to_rgba8()
                .pixels()
                .all(|p| p.0 == color)
        );
    }
    let specs: Vec<_> = request.outputs.iter().map(|(_, s)| s.clone()).collect();
    let views = crate::theme_gen::views::render_outputs(&specs, &request.images).unwrap();
    for ((id, _), view) in request.outputs.iter().zip(views) {
        assert_eq!(
            view,
            raster::over_black(&image::open(&batch.tiles[id].path).unwrap().to_rgba8())
        );
    }
    let config_path = dir.join("config.toml");
    commit(
        &crate::config::Config::default(),
        &request.images,
        &batch,
        &cache,
        &config_path,
    )
    .unwrap();
    assert_eq!(
        crate::config::Config::load_from(&config_path).0.background,
        request.images
    );
    std::fs::remove_file(green.local_path()).unwrap();
    std::fs::remove_file(blue.local_path()).unwrap();
    request.editor = None;
    // Wayland object IDs can change on reconnect; assignments use stable names.
    request.outputs[0].0 = OutputId(99);
    let restored = bake(&request, &cache).unwrap();
    assert!(restored.tiles.contains_key(&OutputId(99)));
    assert_eq!(
        restored.tiles[&OutputId(99)].path,
        batch.tiles[&OutputId(1)].path
    );
    // An actual newly connected assignment must validate its source.
    request.outputs.push((
        OutputId(4),
        OutputSpec {
            key: "disconnected".into(),
            rect: (4.0, 0.0, 2.0, 2.0),
            pixels: (2, 2),
        },
    ));
    assert!(bake(&request, &cache).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_freeform_cache_survives_upgrade_and_cached_monitor_order_is_stable() {
    let dir = directory("legacy-cache");
    let (image, _) = source(
        &dir,
        "source.png",
        RgbaImage::from_pixel(4, 2, Rgba([20, 30, 40, 255])),
    );
    let mut request = request(vec![image.clone()]);
    request.outputs.push((
        OutputId(2),
        OutputSpec {
            key: "DP-2".into(),
            rect: (0.0, 0.0, 4.0, 2.0),
            pixels: (4, 2),
        },
    ));
    let cache = dir.join("cache");
    let batch = bake(&request, &cache).unwrap();
    let mut saved: Vec<_> = batch
        .tiles
        .values()
        .map(|tile| serde_json::json!({"spec":tile.spec, "path":tile.path}))
        .collect();
    saved.sort_by(|a, b| b["spec"]["key"].as_str().cmp(&a["spec"]["key"].as_str()));
    std::fs::remove_file(image.local_path()).unwrap();
    request.editor = None;
    for version in [1, 2] {
        let images = if version == 1 {
            serde_json::to_value(&request.images.image).unwrap()
        } else {
            serde_json::to_value(&request.images).unwrap()
        };
        std::fs::write(
            cache.join("applied.json"),
            serde_json::to_vec(
                &serde_json::json!({"version":version,"images":images,"tiles":saved}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(bake(&request, &cache).unwrap().tiles.len(), 2);
        let outputs = cached_outputs_from(&cache);
        assert_eq!(
            outputs.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
            ["DP-1", "DP-2"]
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn switching_wallpaper_modes_retains_both_recipes_without_applying_the_draft() {
    use crate::config::{ConfigPatch, MonitorWallpaper, WallpaperFit};
    let base = BackgroundConfig {
        image: vec![BackgroundImage {
            path: "freeform.png".into(),
            x: 17.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut draft = draft::Draft::new(&base);
    draft.patch(ConfigPatch::WallpaperMode(WallpaperMode::PerMonitor));
    draft.patch(ConfigPatch::MonitorWallpaper(MonitorWallpaper {
        output: "DP-1".into(),
        path: "one.png".into(),
        fit: WallpaperFit::Fit,
    }));
    draft.patch(ConfigPatch::MonitorWallpaper(MonitorWallpaper {
        output: "DP-2".into(),
        path: "two.png".into(),
        fit: WallpaperFit::Center,
    }));
    draft.patch(ConfigPatch::MonitorWallpaper(MonitorWallpaper {
        output: "DP-1".into(),
        path: "changed.png".into(),
        fit: WallpaperFit::Stretch,
    }));
    assert_eq!(draft.recipe.monitor.len(), 2);
    assert_eq!(draft.recipe.monitor[1].path, "two.png");
    draft.patch(ConfigPatch::WallpaperMode(WallpaperMode::FreeForm));
    assert_eq!(draft.recipe.image, base.image);
    assert_eq!(draft.recipe.monitor[0].path, "changed.png");
    assert_eq!(draft.base, base);
    assert!(draft.dirty());
    let discarded = draft::Draft::new(&draft.base);
    assert!(!discarded.dirty());
    assert!(discarded.recipe.monitor.is_empty());
}

fn directory(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("riced-wallpaper-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn repeated_apply_reuses_handles_without_retaining_replaced_pixels() {
    let output = OutputId(1);
    let spec = OutputSpec {
        key: "DP-1".into(),
        rect: (0.0, 0.0, 2.0, 1.0),
        pixels: (2, 1),
    };
    let tile = |color: u8, name: &str| Tile {
        spec: spec.clone(),
        path: PathBuf::from(name),
        handle: iced::widget::image::Handle::from_rgba(2, 1, vec![color; 8]),
    };
    let mut state = State::default();
    assert!(state.publish(Batch {
        tiles: HashMap::from([(output, tile(20, "first.png"))])
    }));
    let original_id = state.applied[&output].handle.id();
    for _ in 0..4 {
        assert!(!state.publish(Batch {
            tiles: HashMap::from([(output, tile(20, "new-cache-path.png"))])
        }));
        assert_eq!(state.applied[&output].handle.id(), original_id);
        assert_eq!(
            state.applied[&output].path,
            PathBuf::from("new-cache-path.png")
        );
    }
    assert!(state.publish(Batch {
        tiles: HashMap::from([(output, tile(30, "changed.png"))])
    }));
    assert_ne!(state.applied[&output].handle.id(), original_id);
    assert!(state.publish(Batch {
        tiles: HashMap::new()
    }));
    assert!(state.applied.is_empty());
}

#[test]
fn cached_tiles_restore_in_parallel_across_many_outputs() {
    let dir = directory("parallel-restore");
    let (image, _) = source(
        &dir,
        "shared.png",
        RgbaImage::from_pixel(4, 2, Rgba([7, 9, 11, 255])),
    );
    let outputs: Vec<_> = (1..=4)
        .map(|n| {
            (
                OutputId(n),
                OutputSpec {
                    key: format!("DP-{n}"),
                    rect: (0.0, 0.0, 4.0, 2.0),
                    pixels: (4, 2),
                },
            )
        })
        .collect();
    let mut request = request(vec![image.clone()]);
    request.outputs = outputs;
    let cache = dir.join("cache");
    let batch = bake(&request, &cache).unwrap();
    assert_eq!(batch.tiles.len(), 4);
    save_manifest(&cache, &request.images, &batch).unwrap();
    std::fs::remove_file(image.local_path()).unwrap();
    request.editor = None;
    request.sources.clear();
    let restored = bake(&request, &cache).unwrap();
    assert_eq!(restored.tiles.len(), 4);
    for (id, tile) in &restored.tiles {
        assert_eq!(tile.path, batch.tiles[id].path);
        assert_eq!(
            image::open(&tile.path).unwrap().to_rgba8(),
            RgbaImage::from_pixel(4, 2, Rgba([7, 9, 11, 255]))
        );
    }
    std::fs::remove_file(&restored.tiles[&OutputId(2)].path).unwrap();
    assert!(
        bake(&request, &cache).is_err(),
        "a broken cache must not publish a partial set when originals are unavailable"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn apply_snapshots_reuse_loaded_pixels_even_if_the_source_file_changes() {
    let dir = directory("incremental");
    let (first, sources) = source(
        &dir,
        "first.png",
        RgbaImage::from_pixel(8, 4, Rgba([10, 20, 30, 255])),
    );
    let (second, _) = source(
        &dir,
        "second.png",
        RgbaImage::from_pixel(4, 4, Rgba([40, 50, 60, 255])),
    );
    // Replacing an already-loaded file on disk must not re-decode it: the
    // editor keeps the pixels it previewed until an explicit reload.
    RgbaImage::from_pixel(8, 4, Rgba([200, 0, 0, 255]))
        .save(first.local_path())
        .unwrap();
    let images = vec![first.clone(), second.clone()];
    let sources = raster::load_sources(&images, sources).unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(
        sources[&first.local_path()].pixels,
        bytes::Bytes::from([10u8, 20, 30, 255].repeat(8 * 4)),
        "unchanged source must keep its loaded pixels"
    );
    assert_eq!(sources[&second.local_path()].width, 4);
    assert_eq!(sources[&second.local_path()].height, 4);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn preview_sources_reuse_unchanged_files_reload_replacements_and_release_removals() {
    let dir = directory("preview-revisions");
    let (first, sources) = source(
        &dir,
        "first.png",
        RgbaImage::from_pixel(8, 4, Rgba([10, 20, 30, 255])),
    );
    let first_source = sources[&first.local_path()].clone();
    let (second, _) = source(
        &dir,
        "second.png",
        RgbaImage::from_pixel(4, 4, Rgba([40, 50, 60, 255])),
    );
    let images = vec![first.clone(), second.clone()];
    let loaded = raster::load_preview_sources(&images, sources).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &first_source,
        &loaded[&first.local_path()]
    ));
    let removed = std::sync::Arc::downgrade(&loaded[&second.local_path()]);
    let replacement = dir.join("replacement.png");
    RgbaImage::from_pixel(8, 4, Rgba([200, 0, 0, 255]))
        .save(&replacement)
        .unwrap();
    std::fs::rename(replacement, first.local_path()).unwrap();
    let loaded = raster::load_preview_sources(&images, loaded).unwrap();
    assert!(!std::sync::Arc::ptr_eq(
        &first_source,
        &loaded[&first.local_path()]
    ));
    assert_eq!(&loaded[&first.local_path()].pixels[..4], &[200, 0, 0, 255]);
    let loaded = raster::load_preview_sources(std::slice::from_ref(&first), loaded).unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(removed.upgrade().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn restoration_never_requests_theme_generation_but_explicit_apply_does_even_if_unchanged() {
    let output = OutputId(1);
    let spec = OutputSpec {
        key: "DP-1".into(),
        rect: (0.0, 0.0, 1.0, 1.0),
        pixels: (1, 1),
    };
    let batch = || Batch {
        tiles: HashMap::from([(
            output,
            Tile {
                spec: spec.clone(),
                path: PathBuf::from("applied.png"),
                handle: Handle::from_rgba(1, 1, vec![20; 4]),
            },
        )]),
    };
    let mut restore = request(vec![]);
    restore.editor = None;
    restore.outputs = vec![(output, spec.clone())];
    let mut state = State::default();
    let result = state.publish_request(&restore, batch());
    assert!(result.pixels_changed);
    assert!(!result.regenerate_theme);
    let result = state.publish_request(&restore, batch());
    assert!(!result.pixels_changed);
    assert!(!result.regenerate_theme);
    let mut apply = restore.clone();
    apply.editor = Some((window::Id::unique(), 0));
    let result = state.publish_request(&apply, batch());
    assert!(!result.pixels_changed);
    assert!(result.regenerate_theme);
}

fn source(dir: &Path, name: &str, pixels: RgbaImage) -> (BackgroundImage, raster::Sources) {
    let path = dir.join(name);
    pixels.save(&path).unwrap();
    let image = BackgroundImage {
        path: path.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let sources = raster::load_sources(std::slice::from_ref(&image), Sources::new()).unwrap();
    (image, sources)
}

fn request(images: Vec<BackgroundImage>) -> Request {
    Request {
        generation: 1,
        editor: Some((window::Id::unique(), 0)),
        base: BackgroundConfig::default(),
        images: BackgroundConfig {
            image: images,
            ..Default::default()
        },
        outputs: vec![(
            OutputId(1),
            OutputSpec {
                key: "DP-1".into(),
                rect: (0.0, 0.0, 4.0, 2.0),
                pixels: (4, 2),
            },
        )],
        sources: Sources::new(),
    }
}

#[test]
fn native_pixels_and_lossless_tiles_preserve_sources_and_survive_restart_without_sources() {
    let dir = directory("native");
    let original = RgbaImage::from_fn(4, 2, |x, y| {
        Rgba([(x * 50 + 7) as u8, (y * 90 + 11) as u8, 83, 126])
    });
    let (image, sources) = source(&dir, "original.png", original.clone());
    let raw = std::fs::read(image.local_path()).unwrap();
    assert_eq!(
        raster::compose(
            (0.0, 0.0, 4.0, 2.0),
            (4, 2),
            std::slice::from_ref(&image),
            &sources
        )
        .unwrap(),
        original
    );
    let mut request = request(vec![image.clone()]);
    request.sources = sources;
    let cache = dir.join("cache");
    let batch = bake(&request, &cache).unwrap();
    assert_eq!(
        image::open(&batch.tiles[&OutputId(1)].path)
            .unwrap()
            .to_rgba8(),
        original
    );
    assert_eq!(std::fs::read(image.local_path()).unwrap(), raw);
    save_manifest(&cache, &request.images, &batch).unwrap();
    std::fs::remove_file(image.local_path()).unwrap();
    request.editor = None;
    request.sources.clear();
    let restored = bake(&request, &cache).unwrap();
    assert_eq!(
        image::open(&restored.tiles[&OutputId(1)].path)
            .unwrap()
            .to_rgba8(),
        original
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn crops_use_actual_source_dimensions_and_output_pixel_density() {
    let dir = directory("scale");
    let (mut image, sources) = source(
        &dir,
        "stripes.png",
        RgbaImage::from_fn(4, 1, |x, _| {
            if x < 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        }),
    );
    image.x = -2.0;
    image.width = 8.0;
    // A 2-logical-pixel output at 2x density maps directly to four source
    // samples over the right half, rather than treating the width override
    // as the file's native pixel count.
    let crop = raster::compose((2.0, 0.0, 2.0, 1.0), (4, 2), &[image], &sources).unwrap();
    assert!(crop.get_pixel(0, 0)[2] > crop.get_pixel(0, 0)[0]);
    assert_eq!(crop.get_pixel(3, 0).0, [0, 0, 255, 255]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rotated_output_pixel_dimensions_keep_native_detail() {
    use wayland_client::protocol::wl_output::Transform;
    assert_eq!(
        crate::shared::geometry::oriented_pixels((3840, 2160), Transform::Normal),
        (3840, 2160)
    );
    assert_eq!(
        crate::shared::geometry::oriented_pixels((3840, 2160), Transform::_90),
        (2160, 3840)
    );
    assert_eq!(
        crate::shared::geometry::oriented_pixels((3840, 2160), Transform::Flipped270),
        (2160, 3840)
    );
}

#[test]
fn alpha_and_z_order_are_composited_without_transparent_color_halos() {
    let dir = directory("alpha");
    let (mut image, sources) = source(
        &dir,
        "edges.png",
        RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 255, 0, 0])
            }
        }),
    );
    image.width = 2.0;
    let filtered = raster::compose((0.0, 0.0, 2.0, 1.0), (1, 1), &[image], &sources).unwrap();
    assert_eq!(filtered.get_pixel(0, 0).0, [255, 0, 0, 128]);
    let (bottom, mut sources) = source(
        &dir,
        "red.png",
        RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])),
    );
    let (mut top, extra) = source(
        &dir,
        "blue.png",
        RgbaImage::from_pixel(1, 1, Rgba([0, 0, 255, 128])),
    );
    top.z = 1;
    sources.extend(extra);
    let composed = raster::compose((0.0, 0.0, 1.0, 1.0), (1, 1), &[top, bottom], &sources).unwrap();
    let pixel = composed.get_pixel(0, 0);
    assert!((pixel[0] as i32 - 187).abs() <= 1 && (pixel[2] as i32 - 188).abs() <= 1);
    assert_eq!(pixel[3], 255);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_persistence_restores_manifest_and_does_not_replace_configuration() {
    let dir = directory("commit");
    let cache = dir.join("cache");
    let (image, _) = source(
        &dir,
        "image.png",
        RgbaImage::from_pixel(4, 2, Rgba([20, 30, 40, 255])),
    );
    let request = request(vec![image]);
    let batch = bake(&request, &cache).unwrap();
    save_manifest(
        &cache,
        &BackgroundConfig::default(),
        &Batch {
            tiles: HashMap::new(),
        },
    )
    .unwrap();
    let previous = std::fs::read(cache.join("applied.json")).unwrap();
    let blocking_parent = dir.join("not-a-directory");
    std::fs::write(&blocking_parent, "keep").unwrap();
    let config = crate::config::Config::default();
    assert!(
        commit(
            &config,
            &request.images,
            &batch,
            &cache,
            &blocking_parent.join("config.toml")
        )
        .is_err()
    );
    assert_eq!(std::fs::read(cache.join("applied.json")).unwrap(), previous);
    let path = dir.join("config.toml");
    let (saved, _) = commit(&config, &request.images, &batch, &cache, &path).unwrap();
    assert_eq!(saved.background, request.images);
    assert_eq!(
        crate::config::Config::load_from(&path).0.background,
        request.images
    );
    assert_eq!(saved.theme.name, config.theme.name);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn draft_editing_retains_applied_recipe_until_apply_and_prunes_only_unreferenced_cache() {
    let dir = directory("draft");
    let (image, _) = source(
        &dir,
        "image.png",
        RgbaImage::from_pixel(4, 2, Rgba([1, 2, 3, 255])),
    );
    let mut draft = draft::Draft::new(&BackgroundConfig {
        image: vec![image.clone()],
        ..Default::default()
    });
    draft.patch(crate::config::ConfigPatch::MoveImage {
        index: 0,
        x: 99.0,
        y: -3.0,
    });
    assert!(draft.dirty());
    assert_eq!(draft.base.image[0], image);
    assert_eq!(draft.recipe.image[0].x, 99.0);
    let cache = dir.join("cache");
    let batch = bake(&request(vec![image.clone()]), &cache).unwrap();
    let stale = cache.join("0123456789abcdef");
    std::fs::create_dir_all(&stale).unwrap();
    let original = cache.join("abcdef0123456789").join("originals");
    std::fs::create_dir_all(&original).unwrap();
    let protected = BackgroundImage {
        path: original.join("keep.png").to_string_lossy().into_owned(),
        ..Default::default()
    };
    prune(&cache, &[image.clone(), protected], &batch.tiles);
    assert!(!stale.exists());
    assert!(original.exists());
    assert!(image.local_path().exists());
    std::fs::remove_dir_all(dir).unwrap();
}
