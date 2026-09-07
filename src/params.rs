//! CPU-side parameter store backing the ISF uniform block. Shared between the
//! OSC receiver thread and the render loop.

use std::collections::HashMap;

use crate::isf::{InputKind, Isf, Layout, Slot, SlotKind};

pub struct Params {
    pub layout: Layout,
    pub data: Vec<u8>,
    by_name: HashMap<String, usize>,
    /// Event inputs set this frame; cleared after the frame is rendered.
    pending_events: Vec<usize>,
    event_slots: Vec<usize>,
}

impl Params {
    pub fn new(isf: &Isf, layout: Layout) -> Self {
        let by_name = layout
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.clone(), i))
            .collect();
        let mut p = Params {
            data: vec![0; layout.size],
            layout,
            by_name,
            pending_events: Vec::new(),
            event_slots: Vec::new(),
        };
        for inp in &isf.inputs {
            match &inp.kind {
                InputKind::Float { default, .. } => {
                    p.set_floats(&inp.name, &[*default]);
                }
                InputKind::Bool { default } => {
                    p.set_floats(&inp.name, &[*default as i32 as f32]);
                }
                InputKind::Long { default, .. } => {
                    p.set_floats(&inp.name, &[*default as f32]);
                }
                InputKind::Color { default } => {
                    p.set_floats(&inp.name, default);
                }
                InputKind::Point2D { default } => {
                    p.set_floats(&inp.name, default);
                }
                InputKind::Event => {
                    if let Some(i) = p.by_name.get(&inp.name) {
                        p.event_slots.push(*i);
                    }
                }
                InputKind::Image => {}
            }
        }
        p
    }

    pub fn slot(&self, name: &str) -> Option<&Slot> {
        self.by_name.get(name).map(|i| &self.layout.slots[*i])
    }

    fn write_f32(&mut self, off: usize, v: f32) {
        self.data[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn write_i32(&mut self, off: usize, v: i32) {
        self.data[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Set a parameter from a list of floats. Returns false if unknown.
    pub fn set_floats(&mut self, name: &str, vals: &[f32]) -> bool {
        let Some(&i) = self.by_name.get(name) else { return false };
        let slot = self.layout.slots[i].clone();
        match slot.kind {
            SlotKind::F32 => {
                if let Some(v) = vals.first() {
                    self.write_f32(slot.offset, *v);
                }
            }
            SlotKind::I32 => {
                if let Some(v) = vals.first() {
                    self.write_i32(slot.offset, v.round() as i32);
                    if self.event_slots.contains(&i) && *v != 0.0 {
                        self.pending_events.push(i);
                    }
                }
            }
            SlotKind::Vec2 => {
                for (k, v) in vals.iter().take(2).enumerate() {
                    self.write_f32(slot.offset + 4 * k, *v);
                }
            }
            SlotKind::Vec4 => {
                for (k, v) in vals.iter().take(4).enumerate() {
                    self.write_f32(slot.offset + 4 * k, *v);
                }
                if vals.len() == 3 {
                    self.write_f32(slot.offset + 12, 1.0);
                }
            }
        }
        true
    }

    /// Update the per-frame builtins.
    pub fn set_frame(&mut self, width: u32, height: u32, time: f32, dt: f32, frame: i32, pass: i32) {
        self.set_floats("RENDERSIZE", &[width as f32, height as f32]);
        self.set_floats("TIME", &[time]);
        self.set_floats("TIMEDELTA", &[dt]);
        self.set_floats("FRAMEINDEX", &[frame as f32]);
        self.set_floats("PASSINDEX", &[pass as f32]);
        self.set_floats("DATE", &date_vec4());
    }

    /// Call after a frame has been submitted: event inputs are one-shot.
    pub fn end_frame(&mut self) {
        let pending = std::mem::take(&mut self.pending_events);
        for i in pending {
            let off = self.layout.slots[i].offset;
            self.write_i32(off, 0);
        }
    }
}

/// ISF DATE: (year, month, day, seconds since midnight), local time not
/// attempted; UTC is good enough for shaders.
fn date_vec4() -> [f32; 4] {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let sod = (secs % 86400) as f32;
    // Howard Hinnant's civil_from_days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    [y as f32, m as f32, d as f32, sod]
}
