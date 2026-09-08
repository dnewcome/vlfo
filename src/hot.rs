//! Shader hot reload: poll the ISF file's mtime and rebuild the pipeline on
//! change, keeping the values of inputs that still exist.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result};

use crate::gpu::{Gpu, IsfPipeline};
use crate::isf::{self, Isf, Layout};
use crate::params::Params;

pub struct Watch {
    path: PathBuf,
    mtime: Option<SystemTime>,
    last_check: Instant,
}

impl Watch {
    pub fn new(path: &Path) -> Self {
        Watch { path: path.to_path_buf(), mtime: mtime(path), last_check: Instant::now() }
    }

    /// True (at most twice a second) when the file changed since last time.
    pub fn changed(&mut self) -> bool {
        if self.last_check.elapsed().as_millis() < 500 {
            return false;
        }
        self.last_check = Instant::now();
        let now = mtime(&self.path);
        if now != self.mtime && now.is_some() {
            self.mtime = now;
            true
        } else {
            false
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Re-read, translate and build the shader; migrate parameter values.
/// On failure nothing is replaced and the error is returned for logging.
pub fn rebuild(
    gpu: &Gpu,
    path: &Path,
    format: wgpu::TextureFormat,
    params: &Arc<Mutex<Params>>,
) -> Result<(Isf, Layout, IsfPipeline)> {
    let src = crate::graph::load_isf_source(path)?;
    let isf = isf::parse(&src).with_context(|| format!("parse {}", path.display()))?;
    let layout = isf::layout(&isf);
    let pipe = IsfPipeline::new(gpu, &isf, &layout, format)?;
    {
        let mut p = params.lock().unwrap();
        let migrated = p.migrate(&isf, layout.clone());
        *p = migrated;
    }
    Ok((isf, layout, pipe))
}
