---
id: "ADR-0025"
title: Frames and sound leave by their own doors
date: 2026-08-14
status: accepted
---

## Context

ADR-0007 gives the emulation thread three channels: a lossy event ring out, a
stop channel out, and a lossless command ring in. Every record is sixteen
bytes, which is what makes the event ring cheap enough to write to per
instruction.

A frontend needs two things that do not fit through any of them. A frame is
`352 x 296` palette indices — 104 KB, six thousand records — and it is produced
fifty times a second. Sound is produced in twenty-millisecond batches by the
machine and consumed in few-millisecond bites by a device on its own clock,
which is not a rate either end controls.

Both also have to be produced *on* the emulation thread, at the end of a frame,
from state the machine has just finished writing. And ADR-0017 says the
`Spectrum` may not grow host state, because ticket 0027 will clone it into a
checkpoint ring and 0029 will compare clones for divergence.

## Decision

### Two more channels, each shaped like what it carries

Sound goes through the sample ring of ADR-0021, which already exists: a
single-producer single-consumer ring of `f32`, written a frame at a time and
read by the device's callback.

Frames go through a swap chain: two framebuffers and a slot, in which
publishing is `mem::swap` under a `try_lock` and taking is `mem::swap` under a
`lock`. Neither end copies a frame, neither end allocates, and the producer
never waits — a publication that finds the consumer mid-swap is dropped and
counted, which is the same bargain the event ring makes. The consumer is told
how many it missed.

The lock is taken once when the channel is built, because on some platforms the
first lock of a `Mutex` allocates and the first lock would otherwise happen on
the emulation thread.

### The work happens in a wrapper machine, not in the machine

`Presenting<M>` is a `Machine` that holds a `Machine`, delegates everything,
and adds one thing to `service_event`: when the frame is the event that came
due, paint it and publish. That is the pattern `AudioMachine` (ADR-0021) and
`Saving` (ADR-0022) already use, and it is what keeps a framebuffer, a swap
chain and a window's rate out of `Spectrum`.

The order within `service_event` is the opposite of the beeper's, and
deliberately: the beeper reads the edge log, which `Ula::end_frame` rolls on,
so it must read first; the border is *presented* by `end_frame`, so the picture
must be painted after. Both mistakes are silent — silence in one case, a frame
of border lag in the other — so both have tests.

### The window is a consumer, not a clock

Nothing on the window's side decides when a frame happens. The emulation thread
paces itself on how full the sample ring is, so the frame rate is the audio
device's rate expressed in frames, and the window's job is to notice that a
frame has appeared. This is why there is no 50 Hz timer anywhere in the
frontend: a timer and an audio clock disagree by a few parts per million
forever, and reconciling them is a drift correction nobody needs to write if
there is only one clock.

## Consequences

**Positive:**
- A 104 KB frame crosses threads as two pointer swaps, at any rate either side
  likes, and the emulation thread never blocks on the window.
- Frames nobody drew are dropped and counted rather than queued, so a stalled
  window costs memory that does not grow.
- `Spectrum` gains nothing. A headless run paints nothing, because the wrapper
  that paints is not in the stack.
- The frontend has no frame clock to drift, and audio never has to be
  resampled to chase one.

**Negative:**
- Three wrappers deep is a lot of delegation, and a `Bus` method added to the
  trait has to be added to each of them — the failure mode being a wrapper that
  silently delegates to a default body instead of the machine's own.
- The picture is a frame behind what the machine is doing when the window
  redraws for its own reasons, because the swap chain holds one frame and not
  a queue of them. At 50 Hz this is 20 ms and nobody has noticed yet.
- Pacing on the ring means the frame rate follows the audio device. A machine
  with no working sound card paces on nothing, and the fallback — the wall
  clock — is the same code path the non-normal speeds use.
