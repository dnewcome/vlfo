# vlfo

Video LFO. ISF shaders driven from Pure Data over OSC, live in a window or rendered
offline at any size. Spiritual successor of [glfo](https://github.com/dnewcome/glfo);
see `BRIEF.md` for where this is going.

Status: first slice. One ISF shader on a full-screen quad, every input addressable over
OSC, headless PNG rendering. No layers, audio or images yet.

## Build

    cargo build --release

Rust stable, wgpu (Vulkan on Linux), naga for ISF GLSL translation.

## Use

    vlfo inputs shaders/vlfo-test.fs          # inputs and their OSC addresses
    vlfo run shaders/vlfo-test.fs             # live window, OSC on udp 9000
    vlfo render shaders/vlfo-test.fs -o out --frames 300 --fps 30 --size 4800x7200
    vlfo render ... --realtime                # pace to wall clock and take OSC while rendering
    vlfo check shaders/isf-files/*.fs         # which ISF files naga can translate

Every ISF input is reachable as `/vlfo/<NAME>` with floats (or ints/bools). Colors take
3 or 4 floats, point2D takes 2, events take a bang or any nonzero value.

`pd/lfo-test.pd` is a vanilla Pd patch that sends two LFOs to `/vlfo/speed` and
`/vlfo/hue`; open it while `vlfo run shaders/vlfo-test.fs` is up.

Rendering is deterministic: logical time is `frame / fps`, so a 4800x7200 render is
just slower, never sped up.

## Tweaking shaders like a patch

Every ISF input is a knob. `vlfo inputs --pd shader.fs > panel.pd` writes a Pd control
panel with a box per input (number boxes, toggles, bangs, packed colours and points),
already wired to `[vlfo/param]`; `make live SHADER=...` generates it automatically into
`pd/panels/` when no PATCH is given.

Shaders reload live: `run` and `serve` watch the shader file and rebuild it on save,
keeping the current input values. A shader that fails to compile is reported and the
previous one keeps running.

## Node graphs (.vlfo)

Shaders can be built from nodes instead of written. A `.vlfo` file is a list of
chains, one per line, GEM-style:

    in spread 60 0 400                 # declared input: name default min max
    a = translate -3 0 | grid 5 6 spread*0.01 1 | rect 0.3 0.1 | gray 0.5
    b = rotate TIME*20 | translate 2.5 0 | square 0.4 | hsv TIME*0.1 0.8 1

Transform nodes move the sample point (translate, rotate, scale, grid, repeat, mirror),
one shape node gives the distance field (rect, square, circle, ring, line, fill), paint
nodes set colour and alpha (color, gray, hsv, alpha, outline, soften, invert). Chains are
composited in file order. Coordinates are GEM world units: 8 tall, 8 x aspect wide,
centred, y up, so numbers from old scenes carry over.

Every numeric literal in a chain becomes an input named `<chain>_<node>_<param>`, so it
is a knob over OSC and in the generated Pd panel without declaring anything. Any argument
that is not a plain number is a GLSL expression, inlined as written; it may use declared
inputs, `TIME`, and GLSL math. Parameters can also be given by name: `rect h=0.1`.

    vlfo nodes                       # the node reference
    vlfo compile graphs/squares.vlfo # read the generated ISF shader
    make live SHADER=graphs/squares.vlfo

Graphs go through the same path as ISF files everywhere: `run`, `serve`, `render`,
`inputs --pd`, `check`, and hot reload on save.

## Porting GEM / glfo scenes

The Pd side of a scene stays in Pd; only the drawing becomes an ISF shader.
`pd/vlfo/param.pd` is a drop-in for `glfo/param`: `[vlfo/param spread]` sends
`/vlfo/spread`. One `[vlfo/send 9000]` per patch does the network. `pd/squares.pd` +
`shaders/squares.fs` is the worked example, ported from blitbomb's `scenes/squares.pd`:

    make live SHADER=shaders/squares.fs PATCH=pd/squares.pd

GEM world units as seen through a glfo card are 8 units tall and 8 x aspect wide,
centred; the shader maps `isf_FragNormCoord` to that so the original numbers carry over.

## NDI

Needs the NDI runtime (`libndi.so.6`, from the NDI SDK) on the machine; it is loaded at
run time, nothing links against it at build time.

    vlfo serve shaders/vlfo-test.fs --ndi vlfo-1 --output 1920x1080 --fps 60 --port 9001
    vlfo run shaders/vlfo-test.fs --ndi vlfo-1 --output 1920x1080   # window preview + NDI
    vlfo ndi-find                                                    # what is on the network
    vlfo ndi-grab vlfo-1 frame.png                                   # receive one frame

    make serve NDI=vlfo-1
    make ndi4 SHADERS="a.fs b.fs c.fs d.fs"    # vlfo-1..vlfo-4, OSC on udp 9001..9004
    make ndi-find
    make ndi-grab NDI=vlfo-1

The render resolution (`--output`) is independent of the window; the window is only a
preview. Frames are read back once per frame and handed to an NDI thread; if the sender
falls behind, frames are dropped and a warning is logged. Sources appear on the network
as `HOSTNAME (name)`.

Four streams today are four `serve` processes, one shader each. Once layers exist, one
process will publish several outputs.

## ISF coverage

`scripts/fetch-isf-files.sh` pulls the public Vidvox ISF-Files corpus into
`shaders/isf-files/` (not committed). `vlfo check` translates each file through naga.
What is handled so far: all INPUT types except images at runtime (image inputs
translate but cannot yet be bound), `isf_FragNormCoord`, `gl_FragColor`, the standard
uniforms, `IMG_*` macros. Not yet: multi-pass `PASSES` with `TARGET` buffers,
`PERSISTENT` buffers, `IMPORTED` images. naga notes: combined `sampler2D` uniforms are
rejected, so images are declared as a `texture2D` + `sampler` pair behind a macro;
`varying` and `precision` declarations in the body are dropped.
