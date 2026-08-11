---
id: "ADR-0012"
title: Pure-Rust frontend stack
date: 2026-08-11
status: accepted
---

## Context

The emulator eventually needs a window, a framebuffer blit, keyboard input and
audio output. The realistic options were SDL2, which is the traditional choice
for emulators and has the simplest audio story; `winit` + `pixels` + `cpal`,
which is pure Rust; or `egui`/`eframe`, which would also provide debugger
panels.

## Decision

`winit` for windowing and input, `pixels` for blitting the 256x192 framebuffer
via wgpu, `cpal` for audio.

## Consequences

**Positive:**
- `cargo build` is the whole build. No system package manager step, no
  `brew install sdl2`, no bundling of native libraries for distribution.
- Cross-compilation and CI are straightforward.
- `pixels` is a good fit for exactly this shape of problem: a small
  fixed-resolution framebuffer scaled to a window.

**Negative:**
- The audio path is more work than SDL2's. `cpal` gives a callback and a
  sample rate; ring buffering and resampling from the Z80 clock are ours to
  write (ticket 0014).
- wgpu is a heavy dependency for blitting a 48 KB image, and pulls in a large
  compile-time cost.
- Choosing `egui` would have given debugger panels for free. Rejected because
  it couples the debugger to the frontend, which ADR-0013 avoids.
