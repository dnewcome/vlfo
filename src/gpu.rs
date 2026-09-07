//! wgpu setup and the single-pass ISF pipeline: a full-screen triangle whose
//! fragment shader is the translated ISF program.

use std::borrow::Cow;

use anyhow::{anyhow, bail, Context, Result};

use crate::isf::{self, Isf, Layout};

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    pub fn new_instance() -> wgpu::Instance {
        wgpu::Instance::new(&wgpu::InstanceDescriptor::default())
    }

    /// Headless device (no surface).
    pub fn new() -> Result<Self> {
        Self::from_instance(Self::new_instance(), None)
    }

    /// Device from an existing instance, optionally compatible with a surface
    /// created from that same instance.
    pub fn from_instance(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Self> {
        pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: surface,
                    force_fallback_adapter: false,
                })
                .await
                .context("no suitable GPU adapter")?;
            let info = adapter.get_info();
            log::info!("adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("vlfo"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    ..Default::default()
                })
                .await
                .context("request_device failed")?;
            Ok(Gpu { instance, adapter, device, queue })
        })
    }
}

/// Translate ISF-derived GLSL to a naga module, validated.
pub fn translate(glsl: &str) -> Result<(naga::Module, naga::valid::ModuleInfo)> {
    let opts = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = naga::front::glsl::Frontend::default()
        .parse(&opts, glsl)
        .map_err(|errs| anyhow!("{}", errs.emit_to_string(glsl)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| anyhow!("{}", e.emit_to_string(glsl)))?;
    Ok((module, info))
}

const VS_WGSL: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}
"#;

pub struct IsfPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniforms: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl IsfPipeline {
    pub fn new(gpu: &Gpu, isf: &Isf, layout: &Layout, format: wgpu::TextureFormat) -> Result<Self> {
        if isf.image_inputs().next().is_some() {
            bail!(
                "shader has image inputs ({}); not supported yet",
                isf.image_inputs().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ")
            );
        }
        let glsl = isf::generate_glsl(isf, layout);
        let (module, _info) = translate(&glsl)?;
        let fs = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("isf-fragment"),
            source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
        });
        let vs = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen-vertex"),
            source: wgpu::ShaderSource::Wgsl(VS_WGSL.into()),
        });
        let uniforms = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("isf-uniforms"),
            size: layout.size as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("isf-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("isf-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let pl = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("isf-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("isf-pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some("main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Ok(IsfPipeline { pipeline, uniforms, bind_group })
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("isf-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Prefer a non-sRGB surface format: ISF shaders write display-ready values,
/// like a GL default framebuffer.
pub fn pick_format(formats: &[wgpu::TextureFormat]) -> wgpu::TextureFormat {
    formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(formats[0])
}

const BLIT_WGSL: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    var o: VsOut;
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.uv = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return o;
}
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@fragment
fn fs(i: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, smp, i.uv);
}
"#;

/// Draws a texture onto a surface (window preview of an output).
pub struct Blit {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind_group: Option<wgpu::BindGroup>,
}

impl Blit {
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        let module = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let bgl = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pl = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit-pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Blit { pipeline, bgl, sampler, bind_group: None }
    }

    pub fn set_source(&mut self, gpu: &Gpu, view: &wgpu::TextureView) {
        self.bind_group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let Some(bg) = &self.bind_group else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }
}
