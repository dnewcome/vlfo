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
