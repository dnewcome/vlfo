//! Headless rendering to PNG frames. Logical time is frame / fps, so output
//! is deterministic; `realtime` additionally paces frames to wall-clock so
//! live OSC input lands where it would on screen.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::gpu::{Gpu, IsfPipeline};
use crate::params::Params;

pub struct Offscreen {
    pub width: u32,
    pub height: u32,
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_row: u32,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Offscreen {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let padded_row = (width * 4).div_ceil(256) * 256;
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Offscreen { width, height, texture, view, readback, padded_row }
    }

    pub fn format() -> wgpu::TextureFormat {
        FORMAT
    }

    /// Render one frame and return tightly packed RGBA8 rows, top row first.
    pub fn render(&self, gpu: &Gpu, pipe: &IsfPipeline) -> Result<Vec<u8>> {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        pipe.draw(&mut enc, &self.view);
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([enc.finish()]);
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device.poll(wgpu::PollType::Wait).context("device poll")?;
        rx.recv().context("map channel")?.context("buffer map failed")?;
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        {
            let data = slice.get_mapped_range();
            for row in 0..self.height as usize {
                let start = row * self.padded_row as usize;
                out.extend_from_slice(&data[start..start + (self.width * 4) as usize]);
            }
        }
        self.readback.unmap();
        Ok(out)
    }
}

pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    w.write_image_data(rgba)?;
    Ok(())
}

pub struct RenderJob<'a> {
    pub out_dir: &'a Path,
    pub frames: u32,
    pub fps: f32,
    pub realtime: bool,
}

pub fn render_sequence(
    gpu: &Gpu,
    pipe: &IsfPipeline,
    target: &Offscreen,
    params: &Arc<Mutex<Params>>,
    job: RenderJob<'_>,
) -> Result<()> {
    std::fs::create_dir_all(job.out_dir)?;
    let start = Instant::now();
    let dt = 1.0 / job.fps;
    for f in 0..job.frames {
        if job.realtime {
            let due = start + Duration::from_secs_f32(f as f32 * dt);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        {
            let mut p = params.lock().unwrap();
            p.set_frame(target.width, target.height, f as f32 * dt, dt, f as i32, 0);
            gpu.queue.write_buffer(&pipe.uniforms, 0, &p.data);
            p.end_frame();
        }
        let rgba = target.render(gpu, pipe)?;
        let path = job.out_dir.join(format!("frame{f:05}.png"));
        write_png(&path, target.width, target.height, &rgba)?;
    }
    log::info!("wrote {} frames to {}", job.frames, job.out_dir.display());
    Ok(())
}
