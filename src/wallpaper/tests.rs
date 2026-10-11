use super::*;
use image::{Rgba, RgbaImage};

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
        base: Vec::new(),
        images,
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
        &[],
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
    assert_eq!(saved.background.image, request.images);
    assert_eq!(
        crate::config::Config::load_from(&path).0.background.image,
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
    let mut draft = draft::Draft::new(std::slice::from_ref(&image));
    draft.patch(crate::config::ConfigPatch::MoveImage {
        index: 0,
        x: 99.0,
        y: -3.0,
    });
    assert!(draft.dirty());
    assert_eq!(draft.base[0], image);
    assert_eq!(draft.images[0].x, 99.0);
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
