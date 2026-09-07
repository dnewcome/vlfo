//! An output: the ISF pipeline rendered into an offscreen texture at a fixed
//! resolution, optionally published over NDI. The window (if any) previews
//! this texture; headless `serve` just runs it on a timer.

use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::gpu::{Gpu, IsfPipeline};
use crate::ndi;
use crate::offscreen::Offscreen;
use crate::params::Params;

pub struct Output {
    pub target: Offscreen,
    pub pipe: IsfPipeline,
    ndi: Option<NdiWorker>,
    frame: i32,
}

struct NdiWorker {
    tx: SyncSender<Vec<u8>>,
    dropped: u64,
}

impl Output {
    pub fn new(gpu: &Gpu, pipe: IsfPipeline, width: u32, height: u32, ndi_name: Option<&str>, fps: f32) -> Result<Self> {
        let target = Offscreen::new(gpu, width, height);
        let ndi = match ndi_name {
            Some(name) => Some(NdiWorker::spawn(name, width, height, fps)?),
            None => None,
        };
        Ok(Output { target, pipe, ndi, frame: 0 })
    }

    pub fn wants_readback(&self) -> bool {
        self.ndi.is_some()
    }

    /// Render one frame at logical time `t`. Reads the frame back and ships it
    /// to NDI when publishing.
    pub fn frame(&mut self, gpu: &Gpu, params: &Arc<Mutex<Params>>, t: f32, dt: f32) -> Result<()> {
        {
            let mut p = params.lock().unwrap();
            p.set_frame(self.target.width, self.target.height, t, dt, self.frame, 0);
            gpu.queue.write_buffer(&self.pipe.uniforms, 0, &p.data);
            p.end_frame();
        }
        self.frame += 1;
        if self.wants_readback() {
            let rgba = self.target.render(gpu, &self.pipe)?;
            if let Some(n) = self.ndi.as_mut() {
                n.push(rgba);
            }
        } else {
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            self.pipe.draw(&mut enc, &self.target.view);
            gpu.queue.submit([enc.finish()]);
        }
        Ok(())
    }
}

impl NdiWorker {
    fn spawn(name: &str, width: u32, height: u32, fps: f32) -> Result<Self> {
        let sender = ndi::Sender::new(name, fps)?;
        let (tx, rx) = sync_channel::<Vec<u8>>(2);
        std::thread::Builder::new().name(format!("ndi:{name}")).spawn(move || {
            for rgba in rx {
                if let Err(e) = sender.send_rgba(width, height, &rgba) {
                    log::warn!("NDI send: {e:#}");
                }
            }
        })?;
        Ok(NdiWorker { tx, dropped: 0 })
    }

    fn push(&mut self, rgba: Vec<u8>) {
        match self.tx.try_send(rgba) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped += 1;
                if self.dropped % 60 == 1 {
                    log::warn!("NDI sender falling behind, dropped {} frames", self.dropped);
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}
