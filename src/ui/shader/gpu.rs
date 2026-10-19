//! One shared pipeline cache; uniforms belong to individual draws, never files.
use iced::{
    Rectangle, wgpu,
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
}
impl Fragment {
    pub(super) fn new(code: Arc<str>, uniforms: Uniforms) -> Self {
        Self {
            code,
            uniforms,
            slot: AtomicUsize::new(usize::MAX),
        }
    }
}

struct Binding {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
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
}
impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("riced.shader.uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        Self {
            format,
            layout,
            entries: HashMap::new(),
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
    }
}

impl Pipeline {
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
            let compiled = pipeline.compile(device, &self.code);
            pipeline.entries.insert(
                self.code.clone(),
                Entry {
                    pipeline: compiled,
                    bindings: Vec::new(),
                    used: 0,
                },
            );
        }
        let Some(entry) = pipeline.entries.get_mut(&self.code) else {
            return;
        };
        let slot = entry.used;
        entry.used += 1;
        if entry.pipeline.is_none() {
            return;
        }
        if slot == entry.bindings.len() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("riced.shader.instance"),
                size: size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("riced.shader.instance"),
                layout: &pipeline.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            entry.bindings.push(Binding { buffer, group });
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
