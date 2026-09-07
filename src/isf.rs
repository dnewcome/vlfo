//! ISF (Interactive Shader Format) parsing and GLSL code generation.
//!
//! An ISF file is a GLSL fragment shader with a JSON header in a leading
//! `/*{ ... }*/` comment. We keep the body untouched and prepend a prelude that
//! supplies everything the ISF host is expected to provide: the standard
//! uniforms (RENDERSIZE, TIME, ...), the declared INPUTS as a uniform block,
//! `isf_FragNormCoord`, `gl_FragColor` and the IMG_* macros. The result is
//! GLSL 450 that naga can translate.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    Float { default: f32, min: f32, max: f32 },
    Bool { default: bool },
    Event,
    Long { default: i32, min: i32, max: i32 },
    Color { default: [f32; 4] },
    Point2D { default: [f32; 2] },
    Image,
}

#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub label: Option<String>,
    pub kind: InputKind,
}

#[derive(Debug, Clone)]
pub struct Isf {
    pub description: String,
    pub credit: String,
    pub categories: Vec<String>,
    pub inputs: Vec<Input>,
    pub passes: usize,
    /// GLSL body with the header comment removed.
    pub body: String,
}

impl Isf {
    pub fn image_inputs(&self) -> impl Iterator<Item = &Input> {
        self.inputs.iter().filter(|i| i.kind == InputKind::Image)
    }
}

fn f32_at(v: &Value, i: usize, dflt: f32) -> f32 {
    v.get(i).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(dflt)
}

fn parse_input(v: &Value) -> Result<Input> {
    let name = v
        .get("NAME")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("input without NAME"))?
        .to_string();
    let ty = v.get("TYPE").and_then(Value::as_str).unwrap_or("float");
    let label = v.get("LABEL").and_then(Value::as_str).map(str::to_string);
    let d = v.get("DEFAULT");
    let kind = match ty {
        "float" => InputKind::Float {
            default: d.and_then(Value::as_f64).unwrap_or(0.0) as f32,
            min: v.get("MIN").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            max: v.get("MAX").and_then(Value::as_f64).unwrap_or(1.0) as f32,
        },
        "bool" => InputKind::Bool {
            default: match d {
                Some(Value::Bool(b)) => *b,
                Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
                _ => false,
            },
        },
        "event" => InputKind::Event,
        "long" => InputKind::Long {
            default: d.and_then(Value::as_f64).unwrap_or(0.0) as i32,
            min: v.get("MIN").and_then(Value::as_f64).unwrap_or(0.0) as i32,
            max: v.get("MAX").and_then(Value::as_f64).unwrap_or(1.0) as i32,
        },
        "color" => InputKind::Color {
            default: match d {
                Some(arr @ Value::Array(_)) => [
                    f32_at(arr, 0, 1.0),
                    f32_at(arr, 1, 1.0),
                    f32_at(arr, 2, 1.0),
                    f32_at(arr, 3, 1.0),
                ],
                _ => [1.0; 4],
            },
        },
        "point2D" => InputKind::Point2D {
            default: match d {
                Some(arr @ Value::Array(_)) => [f32_at(arr, 0, 0.5), f32_at(arr, 1, 0.5)],
                _ => [0.5, 0.5],
            },
        },
        "image" | "audio" | "audioFFT" => InputKind::Image,
        other => bail!("input {name}: unsupported TYPE {other:?}"),
    };
    Ok(Input { name, label, kind })
}

/// Split an ISF source into (header json, body).
pub fn split_header(src: &str) -> Result<(&str, &str)> {
    let start = src.find("/*").context("no ISF header comment (/*{ ... }*/) found")?;
    let end_rel = src[start..].find("*/").context("unterminated ISF header comment")?;
    let header = &src[start + 2..start + end_rel];
    let body = &src[start + end_rel + 2..];
    Ok((header.trim(), body))
}

pub fn parse(src: &str) -> Result<Isf> {
    let (header, body) = split_header(src)?;
    let json: Value = serde_json::from_str(header)
        .or_else(|e| lenient_json(header).ok_or(e))
        .context("ISF header is not valid JSON")?;
    let str_field = |k: &str| json.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let categories = json
        .get("CATEGORIES")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let inputs = json
        .get("INPUTS")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(parse_input).collect::<Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    let passes = json
        .get("PASSES")
        .and_then(Value::as_array)
        .map(|a| a.len().max(1))
        .unwrap_or(1);
    Ok(Isf {
        description: str_field("DESCRIPTION"),
        credit: str_field("CREDIT"),
        categories,
        inputs,
        passes,
        body: body.to_string(),
    })
}

/// Some ISF headers in the wild have trailing commas. Strip them and retry.
fn lenient_json(header: &str) -> Option<Value> {
    let mut out = String::with_capacity(header.len());
    let chars: Vec<char> = header.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    serde_json::from_str(&out).ok()
}

/// One member of the generated uniform block.
#[derive(Debug, Clone)]
pub struct Slot {
    /// Name as addressed from outside (the ISF input NAME, or a builtin).
    pub name: String,
    /// Name of the member inside the GLSL block.
    pub member: String,
    pub kind: SlotKind,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlotKind {
    F32,
    I32,
    Vec2,
    Vec4,
}

impl SlotKind {
    pub fn size(self) -> usize {
        match self {
            SlotKind::F32 | SlotKind::I32 => 4,
            SlotKind::Vec2 => 8,
            SlotKind::Vec4 => 16,
        }
    }
    pub fn align(self) -> usize {
        self.size()
    }
    fn glsl(self) -> &'static str {
        match self {
            SlotKind::F32 => "float",
            SlotKind::I32 => "int",
            SlotKind::Vec2 => "vec2",
            SlotKind::Vec4 => "vec4",
        }
    }
}

/// Uniform block layout shared between the GLSL prelude and the CPU side.
/// std140 and WGSL uniform rules agree for these types: scalars align 4,
/// vec2 align 8, vec4 align 16. Members are emitted largest first so the
/// layout is dense and identical on both sides.
#[derive(Debug, Clone)]
pub struct Layout {
    pub slots: Vec<Slot>,
    pub size: usize,
}

pub const BUILTINS: &[(&str, SlotKind)] = &[
    ("RENDERSIZE", SlotKind::Vec2),
    ("TIME", SlotKind::F32),
    ("TIMEDELTA", SlotKind::F32),
    ("FRAMEINDEX", SlotKind::I32),
    ("PASSINDEX", SlotKind::I32),
    ("DATE", SlotKind::Vec4),
];

fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) / a * a
}

pub fn layout(isf: &Isf) -> Layout {
    let mut slots = Vec::new();
    let mut off = 0usize;
    let mut push = |name: &str, member: &str, kind: SlotKind| {
        off = align_up(off, kind.align());
        slots.push(Slot { name: name.to_string(), member: member.to_string(), kind, offset: off });
        off += kind.size();
    };
    for (n, k) in BUILTINS {
        push(n, n, *k);
    }
    // inputs, largest alignment first
    for pass in [SlotKind::Vec4, SlotKind::Vec2, SlotKind::F32] {
        for inp in &isf.inputs {
            let (kind, member) = match &inp.kind {
                InputKind::Color { .. } => (SlotKind::Vec4, inp.name.clone()),
                InputKind::Point2D { .. } => (SlotKind::Vec2, inp.name.clone()),
                InputKind::Float { .. } => (SlotKind::F32, inp.name.clone()),
                InputKind::Long { .. } => (SlotKind::I32, inp.name.clone()),
                InputKind::Bool { .. } | InputKind::Event => {
                    (SlotKind::I32, format!("{}_vlfo_i", inp.name))
                }
                InputKind::Image => continue,
            };
            let bucket = match kind {
                SlotKind::Vec4 => SlotKind::Vec4,
                SlotKind::Vec2 => SlotKind::Vec2,
                _ => SlotKind::F32,
            };
            if bucket == pass {
                push(&inp.name, &member, kind);
            }
        }
    }
    let size = align_up(off, 16).max(16);
    Layout { slots, size }
}

/// Generate the complete GLSL 450 fragment shader for naga.
pub fn generate_glsl(isf: &Isf, layout: &Layout) -> String {
    let mut s = String::new();
    s.push_str("#version 450\n");
    s.push_str("layout(location = 0) out vec4 vlfo_fragColor;\n");
    s.push_str("#define gl_FragColor vlfo_fragColor\n");
    s.push_str("layout(set = 0, binding = 0) uniform VlfoIsf {\n");
    for slot in &layout.slots {
        s.push_str(&format!("    {} {};\n", slot.kind.glsl(), slot.member));
    }
    s.push_str("};\n");
    for inp in &isf.inputs {
        if matches!(inp.kind, InputKind::Bool { .. } | InputKind::Event) {
            s.push_str(&format!("#define {n} ({n}_vlfo_i != 0)\n", n = inp.name));
        }
    }
    // image inputs. naga's GLSL front end rejects combined `sampler2D`
    // uniforms, so each image is a texture2D + sampler pair (bindings 2i+1,
    // 2i+2) and the ISF name becomes a macro building the combined sampler.
    for (i, inp) in isf.image_inputs().enumerate() {
        s.push_str(&format!(
            "layout(set = 0, binding = {}) uniform texture2D {n}_vlfo_tex;\n\
             layout(set = 0, binding = {}) uniform sampler {n}_vlfo_smp;\n\
             #define {n} sampler2D({n}_vlfo_tex, {n}_vlfo_smp)\n",
            1 + 2 * i,
            2 + 2 * i,
            n = inp.name
        ));
    }
    s.push_str(concat!(
        "#define isf_FragNormCoord (vec2(gl_FragCoord.x / RENDERSIZE.x, 1.0 - gl_FragCoord.y / RENDERSIZE.y))\n",
        "#define vv_FragNormCoord isf_FragNormCoord\n",
        "#define texture2D texture\n",
        "#define texture2DRect texture\n",
        "#define IMG_NORM_PIXEL(img, c) texture(img, (c))\n",
        "#define IMG_PIXEL(img, c) texture(img, (c) / vec2(textureSize(img, 0)))\n",
        "#define IMG_THIS_PIXEL(img) texture(img, isf_FragNormCoord)\n",
        "#define IMG_THIS_NORM_PIXEL(img) texture(img, isf_FragNormCoord)\n",
        "#define IMG_SIZE(img) vec2(textureSize(img, 0))\n",
        "#line 1\n",
    ));
    s.push_str(&sanitize_body(&isf.body));
    s
}

/// Drop legacy declarations that GLSL 450 rejects and the prelude already
/// covers: `varying ...;` (typically isf_FragNormCoord), `precision ...;`,
/// and stray `#version` lines.
fn sanitize_body(body: &str) -> String {
    body.lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("varying ") || t.starts_with("precision ") || t.starts_with("#version") {
                format!("// vlfo: dropped: {l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
