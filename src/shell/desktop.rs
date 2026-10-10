//! Desktop chrome state shared across output surfaces.
use super::screens::{ContextMenu, SelectionRect};
use crate::lua::composable::ComposableRuntime;
use iced::Point;
use std::{cell::RefCell, collections::HashMap, path::PathBuf, time::Instant};

#[derive(Debug, Default)]
pub(crate) struct DesktopState {
    pub selection_rect: SelectionRect,
    pub composable_runtime: RefCell<ComposableRuntime>,
    pub fade_rect: Option<SelectionRect>,
    pub fade_start: Option<Instant>,
    pub context_menu: Option<ContextMenu>,
    pub repaint_seq: u64,
    /// Pre-decoded pixels guarantee a populated first frame on each surface.
    pub wallpapers: HashMap<PathBuf, iced::widget::image::Handle>,
}

impl DesktopState {
    pub(crate) fn sync_wallpapers(&mut self, config: &crate::config::Config) {
        let live: Vec<PathBuf> = config
            .background
            .image
            .iter()
            .map(|image| image.local_path())
            .collect();
        self.wallpapers.retain(|path, _| live.contains(path));
        let mut missing = Vec::new();
        for path in live {
            if !path.as_os_str().is_empty()
                && !self.wallpapers.contains_key(&path)
                && !missing.contains(&path)
            {
                missing.push(path);
            }
        }
        std::thread::scope(|scope| {
            let jobs: Vec<_> = missing
                .into_iter()
                .map(|path| {
                    scope.spawn(move || {
                        let decoded = crate::config::decode_handle(&path);
                        (path, decoded)
                    })
                })
                .collect();
            for job in jobs {
                match job.join() {
                    Ok((path, Some((_, _, handle)))) => {
                        self.wallpapers.insert(path, handle);
                    }
                    Ok((_, None)) => {}
                    Err(_) => eprintln!("riced: wallpaper decode thread failed, skipping"),
                }
            }
        });
    }

    pub(crate) fn begin_selection(&mut self, point: Point) {
        if let Some(menu) = &mut self.context_menu {
            menu.open = false;
        }
        self.fade_rect = None;
        self.fade_start = None;
        self.selection_rect.selecting = true;
        self.selection_rect.start_point = Some(point);
        self.selection_rect.x = point.x;
        self.selection_rect.y = point.y;
        self.selection_rect.width = 0.0;
        self.selection_rect.height = 0.0;
    }

    pub(crate) fn end_selection(&mut self, now: Instant) {
        if self.selection_rect.selecting {
            self.fade_rect = Some(self.selection_rect.clone());
            self.fade_start = Some(now);
        }
        self.selection_rect.reset();
    }
}
