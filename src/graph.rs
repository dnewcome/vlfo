//! Node graphs -> ISF shaders.
//!
//! A `.vlfo` file is a list of chains, GEM-style, one per line:
//!
//!     in spread 60 0 400                 # a declared input (float, default, min, max)
//!     a = translate -3 0 | grid 5 6 spread*0.01 1 | rect 0.3 0.1 | gray 0.5
//!     b = circle 1 | color 1 0.2 0.1
//!
//! Each chain draws one thing. Transform nodes move the sample point (like
//! translateXYZ / rotateXYZ), a shape node yields a signed distance, paint
//! nodes set colour and alpha. Chains are composited in file order, later
//! ones over earlier ones. Coordinates are GEM world units as seen through a
//! glfo card: 8 units tall, 8 * aspect wide, centred, y up.
//!
//! Every numeric literal becomes an ISF input named `<chain>_<node>_<param>`
//! so it is a knob (OSC, Pd panel) without declaring anything. An argument
//! that is not a plain number is a GLSL expression and is inlined; it may use
//! declared inputs, TIME, and any GLSL math.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{anyhow, bail, Context, Result};

#[derive(Clone, Copy)]
enum Kind {
    Shape,
    Transform,
    Paint,
}

/// (name, params: (name, default, min, max), kind)
struct NodeDef {
    name: &'static str,
    params: &'static [(&'static str, f32, f32, f32)],
    kind: Kind,
}

const NODES: &[NodeDef] = &[
    // shapes (set d)
    NodeDef { name: "rect", params: &[("w", 1.0, 0.0, 8.0), ("h", 1.0, 0.0, 8.0)], kind: Kind::Shape },
    NodeDef { name: "square", params: &[("s", 1.0, 0.0, 8.0)], kind: Kind::Shape },
    NodeDef { name: "circle", params: &[("r", 1.0, 0.0, 8.0)], kind: Kind::Shape },
    NodeDef { name: "ring", params: &[("r", 1.0, 0.0, 8.0), ("t", 0.1, 0.0, 2.0)], kind: Kind::Shape },
    NodeDef { name: "line", params: &[("x", 1.0, -8.0, 8.0), ("y", 0.0, -8.0, 8.0), ("t", 0.05, 0.0, 2.0)], kind: Kind::Shape },
    NodeDef { name: "fill", params: &[], kind: Kind::Shape },
    // transforms (move the sample point)
    NodeDef { name: "translate", params: &[("x", 0.0, -8.0, 8.0), ("y", 0.0, -8.0, 8.0)], kind: Kind::Transform },
    NodeDef { name: "rotate", params: &[("deg", 0.0, -360.0, 360.0)], kind: Kind::Transform },
    NodeDef { name: "scale", params: &[("s", 1.0, 0.0, 8.0)], kind: Kind::Transform },
    NodeDef { name: "grid", params: &[("cols", 1.0, 1.0, 64.0), ("rows", 1.0, 1.0, 64.0), ("sx", 1.0, 0.0, 8.0), ("sy", 1.0, 0.0, 8.0)], kind: Kind::Transform },
    NodeDef { name: "repeat", params: &[("sx", 1.0, 0.0, 8.0), ("sy", 1.0, 0.0, 8.0)], kind: Kind::Transform },
    NodeDef { name: "mirror", params: &[], kind: Kind::Transform },
    // paint / modifiers
    NodeDef { name: "color", params: &[("r", 1.0, 0.0, 1.0), ("g", 1.0, 0.0, 1.0), ("b", 1.0, 0.0, 1.0)], kind: Kind::Paint },
    NodeDef { name: "gray", params: &[("v", 1.0, 0.0, 1.0)], kind: Kind::Paint },
    NodeDef { name: "hsv", params: &[("h", 0.0, 0.0, 1.0), ("s", 1.0, 0.0, 1.0), ("v", 1.0, 0.0, 1.0)], kind: Kind::Paint },
    NodeDef { name: "alpha", params: &[("a", 1.0, 0.0, 1.0)], kind: Kind::Paint },
    NodeDef { name: "soften", params: &[("s", 0.1, 0.0, 4.0)], kind: Kind::Paint },
    NodeDef { name: "outline", params: &[("t", 0.05, 0.0, 2.0)], kind: Kind::Paint },
    NodeDef { name: "invert", params: &[], kind: Kind::Paint },
];

fn node_def(name: &str) -> Option<&'static NodeDef> {
    NODES.iter().find(|n| n.name == name)
}

pub fn node_help() -> String {
    let mut s = String::new();
    for n in NODES {
        let kind = match n.kind {
            Kind::Shape => "shape",
            Kind::Transform => "transform",
            Kind::Paint => "paint",
        };
        let ps: Vec<String> = n.params.iter().map(|(p, d, _, _)| format!("{p}={d}")).collect();
        let _ = writeln!(s, "{:<10} {:<10} {}", n.name, kind, ps.join(" "));
    }
    s
}

struct Input {
    name: String,
    default: f32,
    min: f32,
    max: f32,
}

struct Node {
    def: &'static NodeDef,
    /// GLSL expression per parameter (input name or inlined expression)
    args: Vec<String>,
}

struct Chain {
    name: String,
    nodes: Vec<Node>,
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Compile a graph source into ISF (JSON header + GLSL body).
pub fn compile(src: &str, title: &str) -> Result<String> {
    let mut inputs: Vec<Input> = Vec::new();
    let mut chains: Vec<Chain> = Vec::new();
    let mut declared: HashMap<String, ()> = HashMap::new();

    for (ln, raw) in src.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = |e: String| anyhow!("line {}: {e}\n    {raw}", ln + 1);
        if let Some(rest) = line.strip_prefix("in ") {
            let t: Vec<&str> = rest.split_whitespace().collect();
            if t.is_empty() || !is_ident(t[0]) {
                return Err(at("expected: in NAME DEFAULT [MIN MAX]".into()));
            }
            let f = |i: usize, d: f32| -> Result<f32> {
                t.get(i).map(|v| v.parse::<f32>().map_err(|_| at(format!("bad number {v:?}")))).unwrap_or(Ok(d))
            };
            let default = f(1, 0.0)?;
            let (min, max) = (f(2, default.min(0.0))?, f(3, default.max(1.0))?);
            declared.insert(t[0].to_string(), ());
            inputs.push(Input { name: t[0].to_string(), default, min, max });
            continue;
        }
        let (name, body) = line.split_once('=').ok_or_else(|| at("expected `name = node ... | node ...` or `in NAME ...`".into()))?;
        let name = name.trim();
        if !is_ident(name) {
            return Err(at(format!("bad chain name {name:?}")));
        }
        if chains.iter().any(|c| c.name == name) || declared.contains_key(name) {
            return Err(at(format!("{name:?} already defined")));
        }
        let mut nodes = Vec::new();
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for seg in body.split('|') {
            let toks: Vec<&str> = seg.split_whitespace().collect();
            let Some(&nname) = toks.first() else { return Err(at("empty node".into())) };
            let def = node_def(nname).ok_or_else(|| {
                at(format!("unknown node {nname:?}; known: {}", NODES.iter().map(|n| n.name).collect::<Vec<_>>().join(" ")))
            })?;
            let n = seen.entry(def.name).or_insert(0);
            *n += 1;
            let suffix = if *n == 1 { String::new() } else { n.to_string() };
            let mut args: Vec<Option<String>> = vec![None; def.params.len()];
            let mut pos = 0;
            for tok in &toks[1..] {
                let (pi, val) = if let Some((k, v)) = tok.split_once('=').filter(|(k, _)| is_ident(k)) {
                    let pi = def.params.iter().position(|p| p.0 == k).ok_or_else(|| at(format!("{nname} has no parameter {k:?}")))?;
                    (pi, v)
                } else {
                    if pos >= def.params.len() {
                        return Err(at(format!("{nname} takes {} arguments", def.params.len())));
                    }
                    pos += 1;
                    (pos - 1, *tok)
                };
                args[pi] = Some(val.to_string());
            }
            let mut exprs = Vec::new();
            for (pi, p) in def.params.iter().enumerate() {
                let (pname, dflt, min, max) = *p;
                let expr = match &args[pi] {
                    None => Some(dflt.to_string()),
                    Some(v) => match v.parse::<f32>() {
                        Ok(lit) => Some(lit.to_string()),
                        Err(_) => None,
                    },
                };
                match expr {
                    Some(lit) => {
                        // a literal (or omitted) parameter becomes an input
                        let lit: f32 = lit.parse().unwrap();
                        let iname = format!("{name}_{}{suffix}_{pname}", def.name);
                        let (lo, hi) = (min.min(lit), max.max(lit));
                        inputs.push(Input { name: iname.clone(), default: lit, min: lo, max: hi });
                        exprs.push(iname);
                    }
                    None => exprs.push(format!("({})", args[pi].as_ref().unwrap())),
                }
            }
            nodes.push(Node { def, args: exprs });
        }
        chains.push(Chain { name: name.to_string(), nodes });
    }
    if chains.is_empty() {
        bail!("no chains defined");
    }

    // ---- header
    let mut json = String::from("{\n");
    let _ = writeln!(json, "  \"DESCRIPTION\": \"generated by vlfo from {}\",", title.replace('"', ""));
    json.push_str("  \"CATEGORIES\": [\"Generator\", \"vlfo-graph\"],\n  \"INPUTS\": [\n");
    let items: Vec<String> = inputs
        .iter()
        .map(|i| {
            format!(
                "    {{\"NAME\": \"{}\", \"TYPE\": \"float\", \"DEFAULT\": {}, \"MIN\": {}, \"MAX\": {}}}",
                i.name, i.default, i.min, i.max
            )
        })
        .collect();
    json.push_str(&items.join(",\n"));
    json.push_str("\n  ]\n}");

    // ---- GLSL
    let mut g = String::new();
    g.push_str("/*");
    g.push_str(&json);
    g.push_str("*/\n\n");
    g.push_str(concat!(
        "vec3 vlfo_hsv2rgb(vec3 c) {\n",
        "    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);\n",
        "    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);\n",
        "    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);\n}\n\n",
    ));
    for ch in &chains {
        let _ = writeln!(g, "vec4 chain_{}(vec2 p) {{", ch.name);
        g.push_str("    vec2 q = p; float sc = 1.0; float d = -1.0; vec3 col = vec3(1.0);\n");
        g.push_str("    float al = 1.0; float soft = 0.0; bool inv = false;\n");
        for n in &ch.nodes {
            let a = &n.args;
            let _ = writeln!(g, "    // {}", n.def.name);
            let code = match n.def.name {
                "rect" => format!("{{ vec2 e = abs(q) - vec2({}, {}); d = length(max(e, 0.0)) + min(max(e.x, e.y), 0.0); }}", a[0], a[1]),
                "square" => format!("{{ vec2 e = abs(q) - vec2({0}, {0}); d = length(max(e, 0.0)) + min(max(e.x, e.y), 0.0); }}", a[0]),
                "circle" => format!("d = length(q) - {};", a[0]),
                "ring" => format!("d = abs(length(q) - {}) - {};", a[0], a[1]),
                "line" => format!(
                    "{{ vec2 b = vec2({}, {}); float h = clamp(dot(q, b) / dot(b, b), 0.0, 1.0); d = length(q - b * h) - {}; }}",
                    a[0], a[1], a[2]
                ),
                "fill" => "d = -1.0;".to_string(),
                "translate" => format!("q -= vec2({}, {});", a[0], a[1]),
                "rotate" => format!("{{ float r = radians({}); q = mat2(cos(r), -sin(r), sin(r), cos(r)) * q; }}", a[0]),
                "scale" => format!("{{ float s = max({}, 1e-5); q /= s; sc *= s; }}", a[0]),
                "grid" => format!(
                    "{{ vec2 sp = vec2({}, {}); vec2 c = clamp(floor(q / sp + 0.5), vec2(0.0), vec2({}, {}) - 1.0); q -= c * sp; }}",
                    a[2], a[3], a[0], a[1]
                ),
                "repeat" => format!("{{ vec2 sp = vec2({}, {}); q -= sp * floor(q / sp + 0.5); }}", a[0], a[1]),
                "mirror" => "q = abs(q);".to_string(),
                "color" => format!("col = vec3({}, {}, {});", a[0], a[1], a[2]),
                "gray" => format!("col = vec3({});", a[0]),
                "hsv" => format!("col = vlfo_hsv2rgb(vec3({}, {}, {}));", a[0], a[1], a[2]),
                "alpha" => format!("al = {};", a[0]),
                "soften" => format!("soft = {};", a[0]),
                "outline" => format!("d = abs(d) - {};", a[0]),
                "invert" => "inv = true;".to_string(),
                other => bail!("no codegen for {other}"),
            };
            let _ = writeln!(g, "    {code}");
        }
        g.push_str(concat!(
            "    d *= sc;\n",
            "    float aa = fwidth(d) + soft;\n",
            "    float cov = 1.0 - smoothstep(0.0, aa, d);\n",
            "    if (inv) cov = 1.0 - cov;\n",
            "    return vec4(col, al * cov);\n}\n\n",
        ));
    }
    g.push_str("void main() {\n");
    g.push_str("    float aspect = RENDERSIZE.x / RENDERSIZE.y;\n");
    g.push_str("    vec2 p = (isf_FragNormCoord - 0.5) * vec2(8.0 * aspect, 8.0);\n");
    g.push_str("    vec3 acc = vec3(0.0);\n");
    for ch in &chains {
        let _ = writeln!(g, "    {{ vec4 c = chain_{}(p); acc = mix(acc, c.rgb, clamp(c.a, 0.0, 1.0)); }}", ch.name);
    }
    g.push_str("    gl_FragColor = vec4(acc, 1.0);\n}\n");
    Ok(g)
}

/// Load an ISF from either a .fs/.isf file or a .vlfo graph.
pub fn load_isf_source(path: &std::path::Path) -> Result<String> {
    let src = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if path.extension().and_then(|e| e.to_str()) == Some("vlfo") {
        let title = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        compile(&src, &title).with_context(|| format!("compile graph {}", path.display()))
    } else {
        Ok(src)
    }
}
