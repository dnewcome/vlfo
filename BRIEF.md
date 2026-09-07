## vlfo — kickoff brief

Spiritual successor of glfo (the Pd/GEM live-visuals framework): video LFOs, scenes and
presets driving a modern, safe, Linux-native renderer.

- **Problem:** GEM is the only thing that couples Pd's control and audio rates to a
  renderer, and it is rotting. glfo's modulation ideas need a modern backend that also
  renders offline at any size.
- **Done looks like:** A window showing ISF shader layers whose parameters move live from a
  Pd patch over OSC, with audio from PipeWire visibly driving a shader. Then the same graph
  rendered headless to a 4800x7200 frame sequence.
- **Not now:** Video clip playback, any GUI (phase 2, the keyboard-first patcher, owns UI).
  Geometry and multi-output stay nominally in scope but are held to a textured quad until
  the shader path is solid.
- **First slice:** One ISF shader on a full-screen quad in a wgpu window, its published
  inputs exposed as OSC paths, one float parameter moved by a Pd [lfo] sending OSC. No
  audio, no layers yet. Proves naga's ISF translation and the parameter bus.
- **Open question:** How much real-world ISF GLSL naga accepts unmodified. If coverage is
  poor, fall back to a GLSL preprocessing pass or wgpu's GL backend; decide in the first
  slice.

Phases:
1. Renderer: ISF layers on wgpu/naga, PipeWire audio into shader-readable buffers, Pd over
   OSC (libpd as a second control source later), headless offline render.
2. Patcher: keyboard-first, pipe-syntax graph DSL with vim-style modal editing. Separate.
