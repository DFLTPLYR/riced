//! Transactional wallpaper jobs: drafts are editor-only; tiles are desktop-only.
use super::{ConfigEvent, Plant, Plots, SettingEvent};
use crate::wallpaper::{self, Batch, Request, draft::Draft, raster::Sources};
use iced::{Task, window};

impl Plots {
    /// Defer cleanup after a close or worker result so message, view, and
    /// renderer owners can drop before cache eviction and allocator trimming.
    /// `redraw` repaints all surfaces after image-owning closes. Renderer cache
    /// eviction is backend-dependent; a redraw alone does not guarantee it.
    pub(crate) fn reclaim_image_memory(&self, redraw: bool) -> Task<Plant> {
        Task::perform(
            async { tokio::time::sleep(std::time::Duration::from_millis(100)).await },
            move |_| Plant::Config(ConfigEvent::ReclaimImageMemory { redraw }),
        )
    }

    pub(crate) fn load_wallpaper_preview(&mut self, id: window::Id) -> Task<Plant> {
        let outputs: Vec<_> = wallpaper::outputs(&self.windows.output_infos)
            .into_iter()
            .map(|(_, spec)| spec)
            .collect();
        let Some(draft) = self
            .windows
            .settings
            .get_mut(&id)
            .and_then(|setting| setting.wallpaper_draft.as_mut())
        else {
            return Task::none();
        };
        if draft.preview_outputs != outputs {
            draft.preview_outputs = outputs.clone();
            draft.assets_revision += 1;
            draft.loading = true;
        }
        if !draft.loading || draft.preview_running.is_some() {
            return Task::none();
        }
        let images = wallpaper::source_images(&draft.recipe, &outputs);
        let sources = draft.sources.clone();
        let revision = draft.assets_revision;
        draft.preview_running = Some(revision);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let result = wallpaper::raster::load_preview_sources(&images, sources);
                    // Decoder temporaries belong to this worker's allocator
                    // arena; trim after they drop, keeping returned pixels live.
                    crate::theme_gen::trim_memory();
                    result
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| {
                Plant::SettingPlot(SettingEvent::WallpaperPreviewReady {
                    id,
                    revision,
                    result,
                })
            },
        )
    }

    pub(crate) fn discard_wallpaper(&mut self, id: window::Id) -> Task<Plant> {
        if self
            .desktop
            .wallpaper
            .pending
            .as_ref()
            .is_some_and(|request| request.editor.is_some_and(|(editor, _)| editor == id))
        {
            self.desktop.wallpaper.generation += 1;
            self.desktop.wallpaper.pending = None;
        }
        if let Some(setting) = self.windows.settings.get_mut(&id) {
            let running = setting
                .wallpaper_draft
                .as_ref()
                .and_then(|draft| draft.preview_running);
            let revision = setting
                .wallpaper_draft
                .as_ref()
                .map_or(0, |draft| draft.assets_revision + 1);
            let mut draft = Draft::new(&self.config.background);
            draft.assets_revision = revision;
            draft.preview_running = running;
            setting.wallpaper_draft = Some(draft);
        }
        self.load_wallpaper_preview(id)
    }

    pub(crate) fn apply_wallpaper(&mut self, id: window::Id) -> Task<Plant> {
        let Some(draft) = self
            .windows
            .settings
            .get_mut(&id)
            .and_then(|setting| setting.wallpaper_draft.as_mut())
        else {
            return Task::none();
        };
        if draft.loading || draft.applying {
            return Task::none();
        }
        if draft.base != self.config.background {
            draft.error = Some(
                "Wallpaper configuration changed. Discard to reload the current layout.".into(),
            );
            return Task::none();
        }
        let outputs = wallpaper::outputs(&self.windows.output_infos);
        if outputs.is_empty() {
            draft.error = Some("No outputs are available to apply wallpaper".into());
            return Task::none();
        }
        let baseline = match crate::config::Config::apply_baseline(
            &crate::config::config_path(),
            &self.config_mtime,
        ) {
            Ok(baseline) => baseline,
            Err(error) => {
                draft.error = Some(error);
                return Task::none();
            }
        };
        draft.applying = true;
        draft.error = None;
        self.desktop.wallpaper.generation += 1;
        self.desktop.wallpaper.pending = Some(Request {
            config_baseline: Some(baseline),
            generation: self.desktop.wallpaper.generation,
            editor: Some((id, draft.revision)),
            images: draft.recipe.clone(),
            base: self.config.background.clone(),
            outputs,
            sources: draft.sources.clone(),
        });
        self.launch_wallpaper_job()
    }

    pub(crate) fn refresh_wallpapers(&mut self) -> Task<Plant> {
        let outputs = wallpaper::outputs(&self.windows.output_infos);
        let editors: Vec<_> = self.windows.settings.keys().copied().collect();
        let mut previews: Vec<_> = editors
            .into_iter()
            .map(|id| self.load_wallpaper_preview(id))
            .collect();
        if outputs.is_empty() {
            return Task::batch(previews);
        }
        if let Some(request) = &self.desktop.wallpaper.pending {
            if request.outputs == outputs && request.base == self.config.background {
                return Task::batch(previews);
            }
        } else if self.desktop.wallpaper.targets == outputs
            && self.desktop.wallpaper.layout == self.config.background
        {
            return Task::batch(previews);
        }
        let previous = self.desktop.wallpaper.pending.take();
        let (editor, images, sources, config_baseline) = match previous {
            Some(request) if request.editor.is_some() && request.base == self.config.background => {
                (
                    request.editor,
                    request.images,
                    request.sources,
                    request.config_baseline,
                )
            }
            other => {
                if let Some((id, _)) = other.and_then(|request| request.editor)
                    && let Some(draft) = self
                        .windows
                        .settings
                        .get_mut(&id)
                        .and_then(|setting| setting.wallpaper_draft.as_mut())
                {
                    draft.applying = false;
                    draft.error = Some("Wallpaper configuration changed during Apply.".into());
                }
                (None, self.config.background.clone(), Sources::new(), None)
            }
        };
        self.desktop.wallpaper.generation += 1;
        self.desktop.wallpaper.pending = Some(Request {
            config_baseline,
            generation: self.desktop.wallpaper.generation,
            editor,
            images,
            base: self.config.background.clone(),
            outputs,
            sources,
        });
        previews.push(self.launch_wallpaper_job());
        Task::batch(previews)
    }

    fn launch_wallpaper_job(&mut self) -> Task<Plant> {
        if self.desktop.wallpaper.running.is_some() {
            return Task::none();
        }
        let Some(request) = self.desktop.wallpaper.pending.clone() else {
            return Task::none();
        };
        let generation = request.generation;
        self.desktop.wallpaper.running = Some(generation);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let result = wallpaper::bake(&request, &wallpaper::cache_dir());
                    drop(request);
                    crate::theme_gen::trim_memory();
                    result
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Plant::Config(ConfigEvent::WallpaperBaked { generation, result }),
        )
    }

    pub(crate) fn wallpaper_baked(
        &mut self,
        generation: u64,
        result: Result<Batch, String>,
    ) -> Task<Plant> {
        if self.desktop.wallpaper.running != Some(generation) {
            return Task::none();
        }
        self.desktop.wallpaper.running = None;
        if self
            .desktop
            .wallpaper
            .pending
            .as_ref()
            .is_none_or(|request| request.generation != generation)
        {
            return Task::batch([
                self.launch_wallpaper_job(),
                self.reclaim_image_memory(false),
            ]);
        }
        let request = self
            .desktop
            .wallpaper
            .pending
            .take()
            .expect("matching wallpaper request");
        if request.base != self.config.background
            || request.outputs != wallpaper::outputs(&self.windows.output_infos)
        {
            if let Some((id, _)) = request.editor
                && let Some(draft) = self
                    .windows
                    .settings
                    .get_mut(&id)
                    .and_then(|setting| setting.wallpaper_draft.as_mut())
            {
                draft.applying = false;
                draft.error =
                    Some("Outputs or wallpaper configuration changed during Apply.".into());
            }
            return self.refresh_wallpapers();
        }
        let result = result.and_then(|batch| {
            if request.editor.is_some() {
                let baseline = request
                    .config_baseline
                    .as_deref()
                    .ok_or("Missing configuration baseline; reload before Apply.")?;
                let (next, mtime) = wallpaper::commit(
                    &self.config,
                    &request.images,
                    &batch,
                    &wallpaper::cache_dir(),
                    &crate::config::config_path(),
                    baseline,
                )?;
                self.config_mtime = mtime;
                self.config = next;
                self.config_jobs.invalidate();
            } else if let Err(error) =
                wallpaper::save_manifest(&wallpaper::cache_dir(), &request.images, &batch)
            {
                eprintln!("riced: cannot cache wallpaper manifest: {error}");
            }
            Ok(batch)
        });
        match result {
            Ok(batch) => {
                let publication = self.desktop.wallpaper.publish_request(&request, batch);
                wallpaper::prune(
                    &wallpaper::cache_dir(),
                    &request
                        .images
                        .image
                        .iter()
                        .cloned()
                        .chain(request.images.monitor.iter().map(|m| {
                            crate::config::BackgroundImage {
                                path: m.path.clone(),
                                ..Default::default()
                            }
                        }))
                        .collect::<Vec<_>>(),
                    &self.desktop.wallpaper.applied,
                );
                if let Some((id, _)) = request.editor {
                    for (editor, setting) in &mut self.windows.settings {
                        if let Some(draft) = &mut setting.wallpaper_draft {
                            if *editor == id {
                                draft.base = request.images.clone();
                                draft.applying = false;
                                draft.error = None;
                            } else if draft.dirty() {
                                draft.error = Some(
                                    "Wallpaper was applied in another window. Discard to reload."
                                        .into(),
                                );
                            } else {
                                draft.base = request.images.clone();
                                draft.recipe = request.images.clone();
                                draft.assets_revision += 1;
                                draft.loading = true;
                            }
                        }
                    }
                }
                let mut commands = vec![self.reclaim_image_memory(false)];
                if publication.explicit_apply {
                    commands.push(super::apps::daemon_event(
                        self,
                        &serde_json::json!({"type":"wallpaper_applied","theme":self.config.theme}),
                    ));
                }
                if publication.pixels_changed {
                    commands.push(Task::done(Plant::BackgroundPlot(
                        super::BackgroundEvent::Repaint,
                    )));
                }
                let editors: Vec<_> = self.windows.settings.keys().copied().collect();
                for id in editors {
                    commands.push(self.load_wallpaper_preview(id));
                }
                Task::batch(commands)
            }
            Err(error) => {
                eprintln!("riced: wallpaper apply failed: {error}");
                if let Some((id, _)) = request.editor
                    && let Some(draft) = self
                        .windows
                        .settings
                        .get_mut(&id)
                        .and_then(|setting| setting.wallpaper_draft.as_mut())
                {
                    draft.applying = false;
                    draft.error = Some(error);
                }
                self.reclaim_image_memory(false)
            }
        }
    }
}
