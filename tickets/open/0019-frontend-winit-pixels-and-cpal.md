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

Ticket 0014 has landed, so the audio side is a wiring job rather than a design
one: spawn `Emu<AudioMachine>` instead of `Emu<Spectrum>`, hand the `cpal`
device's reported sample rate to `rkw_audio::Config`, and have the callback
call `Output::fill`. It never blocks, never allocates, and always fills the
whole buffer. Two policies fall out of that and belong here rather than there:

**Pace on the ring's fill level.** The core runs at about 360× real time, so
left alone it fills the sample ring in a few milliseconds and then drops
everything — which is not a failure but the mechanism. `AudioMachine::fill`
reports how far ahead of the speaker the machine has got. Running frames until
it is within a couple of frames of full and then waiting is how audio-clocked
emulators pace themselves, and it means the 50 Hz frame pacing above needs no
separate clock and never has to correct for drift against the device.

**Mute whenever the machine is not running at normal speed.** A paused
debugger underruns continuously, and fast-forward produces audio nobody wants
to hear. `Output` already fades rather than clicking into an underrun, so this
is a policy rather than a rescue: mute on `RunState != Running` and on any
speed control that is not 1×, and use the counters — `Output::underruns`,
`Output::dropped` — to notice when the assumption is wrong.
