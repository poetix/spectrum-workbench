---
id: "0019"
title: Frontend: winit, pixels and cpal
priority: medium
created: 2026-08-11
closed: 2026-08-14
---

## Summary

The windowed application: framebuffer presentation, keyboard input and audio
output, on a pure-Rust stack with no system library dependencies (ADR-0012).

## Acceptance criteria

- [x] Window with the framebuffer blitted through `pixels`, correct aspect
      ratio, integer scaling option
- [x] Keyboard events mapped to the matrix (0013)
- [x] `cpal` audio output fed from the beeper (0014) without underruns
- [x] Frame pacing at 50 Hz, decoupled from the emulation thread (0009)
- [x] Fullscreen, pause, reset, and speed control
- [ ] Runs on macOS and at least one of Linux or Windows — built and run on
      macOS; the Linux and Windows halves are untested here (see below)

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

## As built

### The two things that could not go through ADR-0007's channels

A frame is 104 KB and the event ring carries sixteen bytes a record, so the
picture needed a channel of its own; sound needed one because the device pulls
at its own rate rather than the machine's. Both are produced on the emulation
thread at the end of a frame, and neither may live in `Spectrum`, which ADR-0017
keeps as plain replayable state. So both are wrapper machines in the shape
`AudioMachine` and `Saving` already had: `Presenting<M>` paints the finished
frame into a two-buffer swap chain, publication is a `mem::swap` under a
`try_lock`, and the window swaps the other way. ADR-0025 records it.

The order inside `service_event` is the opposite of the beeper's, which is the
one thing here that is easy to get backwards and silent when you do: the beeper
reads the edge log *before* `Ula::end_frame` rolls it on, and the border is
*presented* by `end_frame`, so the picture is painted after. Both are tested.

### Input is a command, not a shared word

The cheap way — a matrix in an `AtomicU64` the ULA reads — would have made every
recorded session unreplayable, because the typing would be invisible to the
command log that tickets 0026 and 0029 are built on. So `Command::Keys(u64)`
carries the whole forty-bit matrix through the lossless ring, applied at the
control tick like everything else and stamped with the T-state it landed on.
`Machine::set_keys` is the hook, defaulted to nothing, so `rkw-debug` still does
not know what a Spectrum is. ADR-0024 records it;
`crates/rkw-spectrum/tests/input.rs` asserts both halves, including that a
session with typing in it replays to the same machine.

The command carries state and not edges, which is what makes "the window lost
focus" a matrix of zero rather than a special case, and what stops a lost
release from leaving a key down for ever.

### Nothing in the frontend keeps time

`Emu::run_paced` and `emu::spawn_paced` were added: after each slice the thread
asks a closure how long to wait, and waits with `park_timeout` so a command
still wakes it. The frontend's answer is the sample ring's fill level — run
until a few frames ahead of the speaker, then wait for the excess to drain —
which means the audio device's crystal is the only clock in the program and
there is no 50 Hz timer to drift against it. Measured on the development
machine: 152 frames in three seconds of wall clock, no underruns.

At 2x the pacing falls back to the wall clock and the sound is muted, because
the alternative is resampling to keep it in tune and nobody wants to listen to
it anyway; at full speed there is no pacing at all. Muting is a policy applied
once per redraw rather than at each transition, because one of the transitions
— a breakpoint stopping the machine — happens on the other thread.

### Scaling, and what `pixels` will and will not do

`pixels`'s scaling renderer takes the floor of the window-to-texture ratio, so
the picture is always an integer multiple and always aspect correct, with a
letterbox where the window is not an exact multiple. There is therefore no
non-integer stretch on offer, and `--scale N` picks the multiple the window
opens at, which is the case with no letterbox at all. A smooth stretch would
need a render pass of our own, and 352x296 stretched by 1.3 looks worse than a
border.

### A host with no sound card

`Session::new` does not fail on one. The stream is opened before the thread is
spawned, so a device that fails half way has not left a machine running behind
it; without a device the pacer falls back to the wall clock, because a sample
ring nobody drains fills once and would stop the machine for good. The window
says so on stderr and runs silent.

### What is not done

Only macOS has been run. The dependencies are the ones ADR-0012 chose precisely
because they are pure Rust and cross-compile, and nothing in `rkw-gui` is
platform-specific except `key_without_modifiers`, which `winit` provides on
macOS, Windows, X11 and Wayland alike — but "builds elsewhere" is not "runs
elsewhere", and the second half of that acceptance criterion is untested.

The window has no menu, no file dialogue and no on-screen status: mounting a
tape and choosing a ROM are command-line arguments, and the title bar carries
the run state, the speed and the mute. A debugger pane is deliberately absent
(ADR-0013), and the counters that would feed a status line — missed frames,
underruns — are on `Session` waiting for one.
