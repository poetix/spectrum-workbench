---
id: "0019"
title: Frontend: winit, pixels and cpal
priority: medium
created: 2026-08-11
---

## Summary

The windowed application: framebuffer presentation, keyboard input and audio
output, on a pure-Rust stack with no system library dependencies (ADR-0012).

## Acceptance criteria

- [ ] Window with the framebuffer blitted through `pixels`, correct aspect
      ratio, integer scaling option
- [ ] Keyboard events mapped to the matrix (0013)
- [ ] `cpal` audio output fed from the beeper (0014) without underruns
- [ ] Frame pacing at 50 Hz, decoupled from the emulation thread (0009)
- [ ] Fullscreen, pause, reset, and speed control
- [ ] Runs on macOS and at least one of Linux or Windows

## Notes

The emulation thread already runs independently, so the frontend is a consumer
of the event ring rather than a driver of the CPU.
