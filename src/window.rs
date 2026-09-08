//! Live window: winit event loop driving an Output every frame and blitting
//! it to the surface as a preview.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::gpu::{self, Blit, Gpu, IsfPipeline};
use crate::hot::{self, Watch};
use crate::isf::{Isf, Layout};
use crate::offscreen::Offscreen;
use crate::output::Output;
use crate::params::Params;

pub struct Settings {
    /// Shader file, watched for changes (hot reload).
    pub path: std::path::PathBuf,
    pub window: (u32, u32),
    /// Output (render) resolution; None = follow the window's pixel size.
    pub output: Option<(u32, u32)>,
    pub ndi: Option<String>,
    pub fps: f32,
    pub title: String,
}

struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    output: Output,
    blit: Blit,
    start: Instant,
    last: Instant,
    watch: Watch,
}

pub struct App {
    isf: Isf,
    layout: Layout,
    params: Arc<Mutex<Params>>,
    settings: Settings,
    live: Option<Live>,
    error: Option<anyhow::Error>,
}

impl App {
    pub fn new(isf: Isf, layout: Layout, params: Arc<Mutex<Params>>, settings: Settings) -> Self {
        App { isf, layout, params, settings, live: None, error: None }
    }

    pub fn run(mut self) -> Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        match self.error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn init(&mut self, el: &ActiveEventLoop) -> Result<Live> {
        let attrs = Window::default_attributes()
            .with_title(&self.settings.title)
            .with_inner_size(LogicalSize::new(self.settings.window.0, self.settings.window.1));
        let window = Arc::new(el.create_window(attrs)?);
        let instance = Gpu::new_instance();
        let surface = instance.create_surface(window.clone())?;
        let gpu = Gpu::from_instance(instance, Some(&surface))?;
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = gpu::pick_format(&caps.formats);
        let phys = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: phys.width.max(1),
            height: phys.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&gpu.device, &config);
        let (ow, oh) = self.settings.output.unwrap_or((config.width, config.height));
        let pipe = IsfPipeline::new(&gpu, &self.isf, &self.layout, Offscreen::format())?;
        let output = Output::new(&gpu, pipe, ow, oh, self.settings.ndi.as_deref(), self.settings.fps)?;
        let mut blit = Blit::new(&gpu, format);
        blit.set_source(&gpu, &output.target.view);
        log::info!("window {}x{} {:?}, output {ow}x{oh}", config.width, config.height, format);
        let now = Instant::now();
        let watch = Watch::new(&self.settings.path);
        Ok(Live { window, surface, config, gpu, output, blit, start: now, last: now, watch })
    }

    fn redraw(&mut self) -> Result<()> {
        let Some(live) = self.live.as_mut() else { return Ok(()) };
        if live.watch.changed() {
            match hot::rebuild(&live.gpu, live.watch.path(), Offscreen::format(), &self.params) {
                Ok((isf, layout, pipe)) => {
                    live.output.set_pipeline(pipe);
                    self.isf = isf;
                    self.layout = layout;
                    log::info!("reloaded {}", live.watch.path().display());
                    crate::print_inputs(&self.isf);
                }
                Err(e) => log::error!("reload failed, keeping the old shader:\n{e:#}"),
            }
        }
        let now = Instant::now();
        let t = (now - live.start).as_secs_f32();
        let dt = (now - live.last).as_secs_f32();
        live.last = now;
        live.output.frame(&live.gpu, &self.params, t, dt)?;
        let frame = match live.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                live.surface.configure(&live.gpu.device, &live.config);
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = live.gpu.device.create_command_encoder(&Default::default());
        live.blit.draw(&mut enc, &view);
        live.gpu.queue.submit([enc.finish()]);
        live.window.pre_present_notify();
        frame.present();
        live.window.request_redraw();
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_none() {
            match self.init(el) {
                Ok(l) => self.live = Some(l),
                Err(e) => {
                    self.error = Some(e);
                    el.exit();
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, .. },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => el.exit(),
                Key::Character(c) if c == "q" => el.exit(),
                _ => {}
            },
            WindowEvent::Resized(size) => {
                if let Some(live) = self.live.as_mut() {
                    live.config.width = size.width.max(1);
                    live.config.height = size.height.max(1);
                    live.surface.configure(&live.gpu.device, &live.config);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.error = Some(e);
                    el.exit();
                }
            }
            _ => {}
        }
    }
}
