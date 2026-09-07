//! Minimal NDI 6 binding, loaded at runtime from libndi.so.6 so the SDK is a
//! runtime dependency only. Covers what vlfo needs: send RGBA video, and find
//! + receive one frame (used by `vlfo ndi-grab` to verify streams).
//!
//! Struct layouts follow Processing.NDI.structs.h / Send.h / Find.h / Recv.h
//! of the NDI 5/6 SDK.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use libloading::{Library, Symbol};

#[repr(C)]
struct SendCreate {
    p_ndi_name: *const c_char,
    p_groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

#[repr(C)]
pub struct VideoFrameV2 {
    xres: i32,
    yres: i32,
    four_cc: i32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    picture_aspect_ratio: f32,
    frame_format_type: i32,
    timecode: i64,
    p_data: *mut u8,
    line_stride_in_bytes: i32,
    p_metadata: *const c_char,
    timestamp: i64,
}

#[repr(C)]
struct FindCreate {
    show_local_sources: bool,
    p_groups: *const c_char,
    p_extra_ips: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    p_ndi_name: *const c_char,
    p_url_address: *const c_char,
}

#[repr(C)]
struct RecvCreateV3 {
    source_to_connect_to: Source,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    p_ndi_recv_name: *const c_char,
}

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> i32 {
    (a as i32) | ((b as i32) << 8) | ((c as i32) << 16) | ((d as i32) << 24)
}
const FOURCC_RGBA: i32 = fourcc(b'R', b'G', b'B', b'A');
const FOURCC_RGBX: i32 = fourcc(b'R', b'G', b'B', b'X');
const FOURCC_BGRA: i32 = fourcc(b'B', b'G', b'R', b'A');
const FOURCC_BGRX: i32 = fourcc(b'B', b'G', b'R', b'X');
const FRAME_FORMAT_PROGRESSIVE: i32 = 1;
const TIMECODE_SYNTHESIZE: i64 = i64::MAX;
const RECV_COLOR_RGBX_RGBA: i32 = 2;
const RECV_BANDWIDTH_HIGHEST: i32 = 100;
const FRAME_TYPE_VIDEO: i32 = 1;

type Instance = *mut c_void;

struct Api {
    _lib: Library,
    initialize: unsafe extern "C" fn() -> bool,
    version: unsafe extern "C" fn() -> *const c_char,
    send_create: unsafe extern "C" fn(*const SendCreate) -> Instance,
    send_destroy: unsafe extern "C" fn(Instance),
    send_video_v2: unsafe extern "C" fn(Instance, *const VideoFrameV2),
    find_create_v2: unsafe extern "C" fn(*const FindCreate) -> Instance,
    find_wait: unsafe extern "C" fn(Instance, u32) -> bool,
    find_sources: unsafe extern "C" fn(Instance, *mut u32) -> *const Source,
    find_destroy: unsafe extern "C" fn(Instance),
    recv_create_v3: unsafe extern "C" fn(*const RecvCreateV3) -> Instance,
    recv_capture_v2: unsafe extern "C" fn(Instance, *mut VideoFrameV2, *mut c_void, *mut c_void, u32) -> i32,
    recv_free_video_v2: unsafe extern "C" fn(Instance, *const VideoFrameV2),
    recv_destroy: unsafe extern "C" fn(Instance),
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn load() -> Result<Api> {
    let candidates: Vec<String> = std::env::var("NDI_RUNTIME_DIR_V6")
        .ok()
        .map(|d| format!("{d}/libndi.so.6"))
        .into_iter()
        .chain(["libndi.so.6".to_string(), "/usr/local/lib/libndi.so.6".to_string(), "libndi.so".to_string()])
        .collect();
    let mut last = None;
    for c in &candidates {
        match unsafe { Library::new(c) } {
            Ok(lib) => {
                let api = unsafe { bind(lib) }?;
                if !unsafe { (api.initialize)() } {
                    bail!("NDIlib_initialize failed (unsupported CPU?)");
                }
                let v = unsafe { CStr::from_ptr((api.version)()) }.to_string_lossy().into_owned();
                log::info!("NDI runtime: {v} ({c})");
                return Ok(api);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow!("cannot load libndi.so.6 (install the NDI SDK/runtime or set NDI_RUNTIME_DIR_V6): {last:?}"))
}

unsafe fn bind(lib: Library) -> Result<Api> {
    macro_rules! sym {
        ($name:literal) => {{
            let s: Symbol<_> = lib.get(concat!($name, "\0").as_bytes()).with_context(|| format!("missing symbol {}", $name))?;
            *s
        }};
    }
    Ok(Api {
        initialize: sym!("NDIlib_initialize"),
        version: sym!("NDIlib_version"),
        send_create: sym!("NDIlib_send_create"),
        send_destroy: sym!("NDIlib_send_destroy"),
        send_video_v2: sym!("NDIlib_send_send_video_v2"),
        find_create_v2: sym!("NDIlib_find_create_v2"),
        find_wait: sym!("NDIlib_find_wait_for_sources"),
        find_sources: sym!("NDIlib_find_get_current_sources"),
        find_destroy: sym!("NDIlib_find_destroy"),
        recv_create_v3: sym!("NDIlib_recv_create_v3"),
        recv_capture_v2: sym!("NDIlib_recv_capture_v2"),
        recv_free_video_v2: sym!("NDIlib_recv_free_video_v2"),
        recv_destroy: sym!("NDIlib_recv_destroy"),
        _lib: lib,
    })
}

fn api() -> Result<&'static Api> {
    match API.get_or_init(|| load().map_err(|e| format!("{e:#}"))) {
        Ok(a) => Ok(a),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// An NDI video sender. Frames are RGBA8, top row first.
pub struct Sender {
    inst: Instance,
    _name: CString,
    fps: f32,
}

unsafe impl Send for Sender {}

impl Sender {
    pub fn new(name: &str, fps: f32) -> Result<Self> {
        let api = api()?;
        let cname = CString::new(name)?;
        let create = SendCreate {
            p_ndi_name: cname.as_ptr(),
            p_groups: std::ptr::null(),
            clock_video: false,
            clock_audio: false,
        };
        let inst = unsafe { (api.send_create)(&create) };
        if inst.is_null() {
            bail!("NDIlib_send_create failed for {name:?}");
        }
        log::info!("NDI source {name:?} created");
        Ok(Sender { inst, _name: cname, fps })
    }

    pub fn send_rgba(&self, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
        if rgba.len() < (width * height * 4) as usize {
            bail!("frame buffer too small");
        }
        let (n, d) = fps_fraction(self.fps);
        let frame = VideoFrameV2 {
            xres: width as i32,
            yres: height as i32,
            four_cc: FOURCC_RGBA,
            frame_rate_n: n,
            frame_rate_d: d,
            picture_aspect_ratio: width as f32 / height as f32,
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: TIMECODE_SYNTHESIZE,
            p_data: rgba.as_ptr() as *mut u8,
            line_stride_in_bytes: (width * 4) as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        unsafe { (api()?.send_video_v2)(self.inst, &frame) };
        Ok(())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        if let Ok(api) = api() {
            unsafe { (api.send_destroy)(self.inst) };
        }
    }
}

fn fps_fraction(fps: f32) -> (i32, i32) {
    let r = fps.round();
    if (fps - r).abs() < 0.01 {
        (r as i32, 1)
    } else {
        ((fps * 1001.0).round() as i32, 1001)
    }
}

/// Discover NDI sources on the network (names as "HOST (name)").
pub fn find(timeout_ms: u32) -> Result<Vec<(String, String)>> {
    let api = api()?;
    let create = FindCreate { show_local_sources: true, p_groups: std::ptr::null(), p_extra_ips: std::ptr::null() };
    let inst = unsafe { (api.find_create_v2)(&create) };
    if inst.is_null() {
        bail!("NDIlib_find_create_v2 failed");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
    let mut out = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        unsafe { (api.find_wait)(inst, remaining.as_millis() as u32) };
        let mut n = 0u32;
        let p = unsafe { (api.find_sources)(inst, &mut n) };
        out.clear();
        for i in 0..n as usize {
            let s = unsafe { *p.add(i) };
            let name = unsafe { CStr::from_ptr(s.p_ndi_name) }.to_string_lossy().into_owned();
            let url = if s.p_url_address.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(s.p_url_address) }.to_string_lossy().into_owned()
            };
            out.push((name, url));
        }
    }
    unsafe { (api.find_destroy)(inst) };
    Ok(out)
}

/// Receive a single video frame from a named source. Returns (w, h, rgba).
pub fn grab(source_name: &str, timeout_ms: u32) -> Result<(u32, u32, Vec<u8>)> {
    let api = api()?;
    let sources = find(timeout_ms.min(3000))?;
    let (name, url) = sources
        .iter()
        .find(|(n, _)| n == source_name || n.ends_with(&format!("({source_name})")))
        .ok_or_else(|| anyhow!("source {source_name:?} not found; available: {:?}", sources.iter().map(|s| &s.0).collect::<Vec<_>>()))?
        .clone();
    let cname = CString::new(name.as_str())?;
    let curl = CString::new(url.as_str())?;
    let recv_name = CString::new("vlfo ndi-grab")?;
    let create = RecvCreateV3 {
        source_to_connect_to: Source {
            p_ndi_name: cname.as_ptr(),
            p_url_address: if url.is_empty() { std::ptr::null() } else { curl.as_ptr() },
        },
        color_format: RECV_COLOR_RGBX_RGBA,
        bandwidth: RECV_BANDWIDTH_HIGHEST,
        allow_video_fields: false,
        p_ndi_recv_name: recv_name.as_ptr(),
    };
    let inst = unsafe { (api.recv_create_v3)(&create) };
    if inst.is_null() {
        bail!("NDIlib_recv_create_v3 failed");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
    let result = loop {
        if std::time::Instant::now() > deadline {
            break Err(anyhow!("no video frame from {name:?} within {timeout_ms} ms"));
        }
        let mut frame: VideoFrameV2 = unsafe { std::mem::zeroed() };
        let ty = unsafe { (api.recv_capture_v2)(inst, &mut frame, std::ptr::null_mut(), std::ptr::null_mut(), 1000) };
        if ty != FRAME_TYPE_VIDEO {
            continue;
        }
        let (w, h) = (frame.xres as u32, frame.yres as u32);
        let stride = frame.line_stride_in_bytes as usize;
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        let bgr = frame.four_cc == FOURCC_BGRA || frame.four_cc == FOURCC_BGRX;
        let opaque = frame.four_cc == FOURCC_RGBX || frame.four_cc == FOURCC_BGRX;
        for y in 0..h as usize {
            let row = unsafe { std::slice::from_raw_parts(frame.p_data.add(y * stride), (w * 4) as usize) };
            for px in row.chunks_exact(4) {
                let (r, g, b) = if bgr { (px[2], px[1], px[0]) } else { (px[0], px[1], px[2]) };
                rgba.extend_from_slice(&[r, g, b, if opaque { 255 } else { px[3] }]);
            }
        }
        unsafe { (api.recv_free_video_v2)(inst, &frame) };
        break Ok((w, h, rgba));
    };
    unsafe { (api.recv_destroy)(inst) };
    result
}
