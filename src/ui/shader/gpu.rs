//! One shared pipeline cache; uniforms belong to individual draws, never files.
use iced::{
    Rectangle,
    advanced::image::{Handle, Id},
    wgpu,
    widget::shader::{self, Viewport},
};
use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Uniforms {
    pub time: f32,
    pub opacity: f32,
    pub resolution: [f32; 2],
    pub mouse: [f32; 2],
    pub padding: [f32; 2],
    pub params: [[f32; 4]; 4],
    pub uv_rect: [f32; 4],
}

#[derive(Debug)]
pub(super) struct Fragment {
    code: Arc<str>,
    uniforms: Uniforms,
    slot: AtomicUsize,
    image: Option<Handle>,
}
impl Fragment {
    pub(super) fn new(code: Arc<str>, uniforms: Uniforms, image: Option<Handle>) -> Self {
        Self {
            code,
            uniforms,
            slot: AtomicUsize::new(usize::MAX),
            image,
        }
    }
}

struct Binding {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    image: Option<Id>,
}
struct Texture {
    view: wgpu::TextureView,
    used: bool,
}
struct Entry {
    pipeline: Option<wgpu::RenderPipeline>,
    bindings: Vec<Binding>,
    used: usize,
}
pub(super) struct Pipeline {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    entries: HashMap<Arc<str>, Entry>,
    textures: HashMap<Option<Id>, Texture>,
    sampler: wgpu::Sampler,
}
impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("riced.shader.uniforms"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        Self {
            format,
            layout,
            entries: HashMap::new(),
            textures: HashMap::new(),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("riced.shader.image-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }
    fn trim(&mut self) {
        // Drop superseded source revisions and inactive pipelines. Reuse each
        // live shader's buffer pool instead of allocating buffers every frame.
        self.entries.retain(|_, entry| {
            let used = entry.used;
            entry.bindings.truncate(used);
            entry.used = 0;
            used > 0
        });
        self.textures.retain(|_, texture| {
            let used = texture.used;
            texture.used = false;
            used
        });
    }
}

impl Pipeline {
    fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: Option<&Handle>,
    ) -> Option<Id> {
        let id = image.map(Handle::id);
        if let Some(texture) = self.textures.get_mut(&id) {
            texture.used = true;
            return id;
        }
        let (width, height, pixels) = match image {
            Some(Handle::Rgba {
                width,
                height,
                pixels,
                ..
            }) => (*width, *height, pixels.as_ref()),
            _ => (1, 1, &[255_u8; 4][..]),
        };
        // Wallpaper tiles are owned decoded RGBA. Downsample only if an output
        // exceeds this adapter's texture limit; never decode files in a frame.
        let maximum = device.limits().max_texture_dimension_2d;
        let resized = if width > maximum || height > maximum {
            let scale = maximum as f64 / f64::from(width.max(height));
            image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, pixels).map(|image| {
                image::imageops::resize(
                    &image,
                    (f64::from(width) * scale).max(1.0) as u32,
                    (f64::from(height) * scale).max(1.0) as u32,
                    image::imageops::FilterType::Triangle,
                )
            })
        } else {
            None
        };
        let (width, height, pixels) = resized.as_ref().map_or((width, height, pixels), |image| {
            (image.width(), image.height(), image.as_raw().as_slice())
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("riced.shader.image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if iced_renderer::graphics::color::GAMMA_CORRECTION {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.textures.insert(
            id,
            Texture {
                view: texture.create_view(&Default::default()),
                used: true,
            },
        );
        id
    }

    fn compile(&self, device: &wgpu::Device, code: &str) -> Option<wgpu::RenderPipeline> {
        // Naga catches source errors before rendering. Device validation still
        // needs an error scope (adapter limits/format support can differ).
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("riced.shader"),
            source: wgpu::ShaderSource::Wgsl(code.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("riced.shader"),
            bind_group_layouts: &[&self.layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("riced.shader"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        if let Some(error) = iced::futures::executor::block_on(device.pop_error_scope()) {
            tracing::error!("shader pipeline: {error}");
            None
        } else {
            Some(pipeline)
        }
    }
}

impl shader::Primitive for Fragment {
    type Pipeline = Pipeline;
    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &Rectangle,
        _: &Viewport,
    ) {
        if !pipeline.entries.contains_key(&self.code) {
            let compiled = pipeline.compile(device, &self.code).or_else(|| {
                self.image.as_ref()?;
                pipeline.compile(device, super::passthrough_code().as_ref())
            });
            pipeline.entries.insert(
                self.code.clone(),
                Entry {
                    pipeline: compiled,
                    bindings: Vec::new(),
                    used: 0,
                },
            );
        }
        let image = pipeline.texture(device, queue, self.image.as_ref());
        let Some(entry) = pipeline.entries.get_mut(&self.code) else {
            return;
        };
        let slot = entry.used;
        entry.used += 1;
        if entry.pipeline.is_none() {
            return;
        }
        if slot == entry.bindings.len() || entry.bindings[slot].image != image {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("riced.shader.instance"),
                size: size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("riced.shader.instance"),
                layout: &pipeline.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &pipeline.textures[&image].view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                    },
                ],
            });
            let binding = Binding {
                buffer,
                group,
                image,
            };
            if slot == entry.bindings.len() {
                entry.bindings.push(binding);
            } else {
                entry.bindings[slot] = binding;
            }
        }
        queue.write_buffer(
            &entry.bindings[slot].buffer,
            0,
            bytemuck::bytes_of(&self.uniforms),
        );
        self.slot.store(slot, Ordering::Relaxed);
    }
    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if let Some(entry) = pipeline.entries.get(&self.code)
            && let Some(pipeline) = &entry.pipeline
            && let Some(binding) = entry.bindings.get(self.slot.load(Ordering::Relaxed))
        {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &binding.group, &[]);
            pass.draw(0..3, 0..1);
        }
        true
    }
}
