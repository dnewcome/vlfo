mod gpu;
mod graph;
mod hot;
mod isf;
mod ndi;
mod offscreen;
mod osc;
mod output;
mod params;
mod pdgen;
mod window;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use crate::isf::InputKind;

#[derive(Parser)]
#[command(name = "vlfo", version, about = "video LFO: ISF shaders driven from Pure Data over OSC")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open a window and run an ISF shader live, inputs driven over OSC.
    Run {
        shader: PathBuf,
        /// Window size, e.g. 1280x720
        #[arg(long, default_value = "1280x720")]
        size: String,
        /// Render resolution (default: the window's pixel size)
        #[arg(long)]
        output: Option<String>,
        /// Also publish the output as an NDI source with this name
        #[arg(long)]
        ndi: Option<String>,
        /// Nominal frame rate reported to NDI
        #[arg(long, default_value_t = 60.0)]
        fps: f32,
        /// OSC UDP port to listen on
        #[arg(long, default_value_t = 9000)]
        port: u16,
    },
    /// Run headless (no window) and publish the output over NDI.
    Serve {
        shader: PathBuf,
        /// NDI source name
        #[arg(long)]
        ndi: String,
        #[arg(long, default_value = "1920x1080")]
        output: String,
        #[arg(long, default_value_t = 60.0)]
        fps: f32,
        #[arg(long, default_value_t = 9000)]
        port: u16,
        /// Stop after this many frames (0 = run until killed)
        #[arg(long, default_value_t = 0)]
        frames: u64,
    },
    /// List NDI sources visible on the network.
    NdiFind {
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u32,
    },
    /// Receive one frame from an NDI source and save it as PNG.
    NdiGrab {
        source: String,
        out: PathBuf,
        #[arg(long, default_value_t = 8000)]
        timeout_ms: u32,
    },
    /// Render an ISF shader headless to a PNG sequence.
    Render {
        shader: PathBuf,
        #[arg(short, long, default_value = "out")]
        out: PathBuf,
        #[arg(long, default_value_t = 90)]
        frames: u32,
        #[arg(long, default_value_t = 30.0)]
        fps: f32,
        #[arg(long, default_value = "1280x720")]
        size: String,
        /// Pace frames to wall-clock and listen for OSC while rendering.
        #[arg(long)]
        realtime: bool,
        #[arg(long, default_value_t = 9000)]
        port: u16,
    },
    /// List an ISF shader's inputs and their OSC addresses.
    Inputs {
        shader: PathBuf,
        /// Emit a Pure Data control panel patch instead (redirect to a .pd file)
        #[arg(long)]
        pd: bool,
        /// OSC port the generated panel sends to
        #[arg(long, default_value_t = 9000)]
        port: u16,
    },
    /// Translate ISF files through naga and report which ones compile.
    Check {
        files: Vec<PathBuf>,
        /// Print the generated WGSL for the (single) file.
        #[arg(long)]
        wgsl: bool,
        /// Print the generated GLSL for the (single) file.
        #[arg(long)]
        glsl: bool,
    },
    /// Run a raw GLSL 450 fragment shader through naga (debugging aid).
    TranslateGlsl { file: PathBuf },
    /// Compile a .vlfo node graph and print the generated ISF shader.
    Compile { graph: PathBuf },
    /// List the node types available in .vlfo graphs.
    Nodes,
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s.split_once('x').context("size must be WxH")?;
    Ok((w.parse()?, h.parse()?))
}

fn load(path: &PathBuf) -> Result<isf::Isf> {
    let src = graph::load_isf_source(path)?;
    isf::parse(&src).with_context(|| format!("parse {}", path.display()))
}

pub fn print_inputs(isf: &isf::Isf) {
    if !isf.description.is_empty() {
        println!("{}", isf.description);
    }
    println!("{:<20} {:<9} {:<28} osc", "input", "type", "default");
    for i in &isf.inputs {
        let (ty, dflt) = match &i.kind {
            InputKind::Float { default, min, max } => ("float", format!("{default} [{min}..{max}]")),
            InputKind::Bool { default } => ("bool", format!("{default}")),
            InputKind::Event => ("event", String::new()),
            InputKind::Long { default, min, max } => ("long", format!("{default} [{min}..{max}]")),
            InputKind::Color { default } => ("color", format!("{default:?}")),
            InputKind::Point2D { default } => ("point2D", format!("{default:?}")),
            InputKind::Image => ("image", "(unsupported)".into()),
        };
        println!("{:<20} {:<9} {:<28} {}{}", i.name, ty, dflt, osc::PREFIX, i.name);
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("vlfo=info")).init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Inputs { shader, pd, port } => {
            let isf = load(&shader)?;
            if pd {
                let title = shader.file_name().unwrap_or_default().to_string_lossy().into_owned();
                print!("{}", pdgen::generate(&isf, &title, port));
            } else {
                print_inputs(&isf);
            }
        }
        Cmd::Check { files, wgsl, glsl } => check(&files, wgsl, glsl)?,
        Cmd::Compile { graph } => print!("{}", graph::load_isf_source(&graph)?),
        Cmd::Nodes => print!("{}", graph::node_help()),
        Cmd::TranslateGlsl { file } => {
            let src = std::fs::read_to_string(&file)?;
            match gpu::translate(&src) {
                Ok(_) => println!("OK"),
                Err(e) => println!("FAIL: {e:#}"),
            }
        }
        Cmd::Run { shader, size, output, ndi, fps, port } => {
            let isf = load(&shader)?;
            print_inputs(&isf);
            let layout = isf::layout(&isf);
            let params = Arc::new(Mutex::new(params::Params::new(&isf, layout.clone())));
            osc::spawn(port, params.clone())?;
            let title = format!("vlfo - {}", shader.file_name().unwrap_or_default().to_string_lossy());
            let settings = window::Settings {
                path: shader.clone(),
                window: parse_size(&size)?,
                output: output.as_deref().map(parse_size).transpose()?,
                ndi,
                fps,
                title,
            };
            window::App::new(isf, layout, params, settings).run()?;
        }
        Cmd::Serve { shader, ndi, output, fps, port, frames } => {
            let isf = load(&shader)?;
            print_inputs(&isf);
            let layout = isf::layout(&isf);
            let params = Arc::new(Mutex::new(params::Params::new(&isf, layout.clone())));
            osc::spawn(port, params.clone())?;
            let (w, h) = parse_size(&output)?;
            let gpu = gpu::Gpu::new()?;
            let pipe = gpu::IsfPipeline::new(&gpu, &isf, &layout, offscreen::Offscreen::format())?;
            let mut out = output::Output::new(&gpu, pipe, w, h, Some(&ndi), fps)?;
            log::info!("serving {w}x{h} @ {fps} fps as NDI {ndi:?}; Ctrl-C to stop");
            let start = std::time::Instant::now();
            let dt = 1.0 / fps;
            let mut n: u64 = 0;
            let mut watch = hot::Watch::new(&shader);
            loop {
                if watch.changed() {
                    match hot::rebuild(&gpu, &shader, offscreen::Offscreen::format(), &params) {
                        Ok((isf, _layout, pipe)) => {
                            out.set_pipeline(pipe);
                            log::info!("reloaded {}", shader.display());
                            print_inputs(&isf);
                        }
                        Err(e) => log::error!("reload failed, keeping the old shader:\n{e:#}"),
                    }
                }
                let due = start + std::time::Duration::from_secs_f64(n as f64 * dt as f64);
                if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
                    std::thread::sleep(wait);
                }
                out.frame(&gpu, &params, n as f32 * dt, dt)?;
                n += 1;
                if frames > 0 && n >= frames {
                    break;
                }
            }
        }
        Cmd::NdiFind { timeout_ms } => {
            for (name, url) in ndi::find(timeout_ms)? {
                println!("{name}\t{url}");
            }
        }
        Cmd::NdiGrab { source, out, timeout_ms } => {
            let (w, h, rgba) = ndi::grab(&source, timeout_ms)?;
            offscreen::write_png(&out, w, h, &rgba)?;
            println!("wrote {} ({w}x{h})", out.display());
        }
        Cmd::Render { shader, out, frames, fps, size, realtime, port } => {
            let isf = load(&shader)?;
            let layout = isf::layout(&isf);
            let params = Arc::new(Mutex::new(params::Params::new(&isf, layout.clone())));
            if realtime {
                osc::spawn(port, params.clone())?;
            }
            let (w, h) = parse_size(&size)?;
            let gpu = gpu::Gpu::new()?;
            let pipe = gpu::IsfPipeline::new(&gpu, &isf, &layout, offscreen::Offscreen::format())?;
            let target = offscreen::Offscreen::new(&gpu, w, h);
            offscreen::render_sequence(
                &gpu,
                &pipe,
                &target,
                &params,
                offscreen::RenderJob { out_dir: &out, frames, fps, realtime },
            )?;
        }
    }
    Ok(())
}

fn check(files: &[PathBuf], wgsl: bool, glsl_out: bool) -> Result<()> {
    if files.is_empty() {
        bail!("no files given");
    }
    let (mut ok, mut parse_fail, mut translate_fail) = (0, 0, 0);
    for path in files {
        let name = path.display();
        let isf = match load(path) {
            Ok(i) => i,
            Err(e) => {
                parse_fail += 1;
                println!("PARSE  {name}: {}", first_line(&format!("{e:#}")));
                continue;
            }
        };
        let layout = isf::layout(&isf);
        let glsl = isf::generate_glsl(&isf, &layout);
        if glsl_out {
            println!("{glsl}");
        }
        match gpu::translate(&glsl) {
            Ok((module, info)) => {
                ok += 1;
                let imgs = isf.image_inputs().count();
                let tag = if imgs > 0 { format!(" ({imgs} image inputs)") } else { String::new() };
                println!("OK     {name}{tag}");
                if wgsl {
                    let out = naga::back::wgsl::write_string(
                        &module,
                        &info,
                        naga::back::wgsl::WriterFlags::empty(),
                    )?;
                    println!("{out}");
                }
            }
            Err(e) => {
                translate_fail += 1;
                println!("FAIL   {name}: {}", first_error(&format!("{e:#}")));
            }
        }
    }
    println!(
        "\n{ok} ok, {translate_fail} failed to translate, {parse_fail} failed to parse ({} total)",
        files.len()
    );
    Ok(())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// naga's emit_to_string output: find the first line that looks like the message.
fn first_error(s: &str) -> String {
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let head = lines.next().unwrap_or("").trim().to_string();
    let loc = lines.find(|l| l.contains("┌─") || l.contains("-->")).map(|l| l.trim().to_string());
    match loc {
        Some(l) => format!("{head}  {l}"),
        None => head,
    }
}
