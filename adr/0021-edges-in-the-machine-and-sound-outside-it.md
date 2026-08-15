---
id: "ADR-0021"
title: Edges in the machine, and sound outside it
date: 2026-08-13
status: accepted
---

## Context

Sound out of a Spectrum is bit 4 of port `0xFE`, and it is entirely a matter of
timing: the level tells you nothing, and a program plays a note by flipping the
bit at the right rate. So the machine's part in this is small and obvious — a
list of when the bit moved. Everything after it is not small at all: resampling
from 3.5 MHz to whatever the device reports, band-limiting well enough that
beeper music does not alias, modelling the cone, buffering across a thread
boundary, and a volume knob.

That second part is made almost entirely of host state. The sample rate comes
from `cpal` at runtime; the filter coefficients are computed from it; the
volume comes from a person. ADR-0017 draws a hard line around exactly this,
and names this ticket while doing it: host state and wall-clock time stay out
of the machine, or replay stops being deterministic. Ticket 0027 will clone
whole machines into a checkpoint ring and 0029 will compare them for
divergence, so anything in `Spectrum` has to be plain, fixed-size, replayable
data.

There is also nowhere obvious to put the per-frame work. It has to run on the
emulation thread, once a frame, from state the machine has just written — and
`Machine::service_event` is the only moment shaped like that. But that method
belongs to `Spectrum`, the one type that must not grow a sample rate.

## Decision

### The edge log is machine state; nothing else is

`Ula` gains one field: a fixed array of 2048 packed `u32`s, each a T-state
offset into the frame and the two audio bits it moved to. A write to port
`0xFE` that leaves both bits alone records nothing, which is what keeps a
border-striping program — thousands of writes a frame, none of them a sound —
from filling it.

It saturates rather than wrapping. Overwrite-oldest, which the event ring uses,
is not merely worse here but unavailable: everything downstream is a forward
walk over offsets that only increase, and a wrapped buffer has no such order.
Saturating loses the tail of one frame; wrapping would deliver that frame's
transitions out of order, which is a bang rather than a gap. What saturation
must not lose is the *level*, so the log tracks where the speaker is
independently of what it had room to write down.

It is not double-buffered, which the border is. The border is rendered later,
on another thread, from a frame that has to have stopped changing; the sound is
drained inside the same `service_event` that ends the frame, before the log
rolls on. There is no second reader, so there is nothing to present to, and a
second buffer would be 8 KB more in every checkpoint and a memcpy a frame to
guard against a consumer that does not exist.

### The rest is a crate that has never heard of a Spectrum

`rkw-audio` takes the clock rate, the frame length and the sample rate as
parameters. `rkw-spectrum` depends on it; it depends on nothing.

The crate boundary is doing real work — it is the only *mechanical* enforcement
of ADR-0017's rule. A module inside `rkw-spectrum` would put `sample_rate` one
line away from being in `Ula`, and nothing would catch it until 0029's
divergence detector fired, months later, on a symptom with no obvious cause. A
crate that cannot name `Spectrum` cannot acquire machine state by accident, and
a machine that depends on it cannot acquire a sample rate by accident either.

It also makes the arithmetic testable. A resampler parameterised by its rates
can be run against a 1 kHz clock at ten samples a second, where every boundary
is an integer and the right answer can be worked out on paper — which is how
the window-straddling case got pinned exactly rather than approximately.

### The hook is a composed machine, not a callback in the slice loop

`Emu` is generic over its machine and `Machine` is two methods, so rather than
adding a hook to the slice loop or a beeper to the `Spectrum`, `AudioMachine`
wraps one machine in another. It delegates every part of being a Spectrum and
adds one thing to `service_event`: make this frame's sound, then end the frame.

The beeper therefore runs on the emulation thread without being machine state,
which is what ADR-0017 asks for — the rule is about what is in the machine, not
about which thread the work happens on. What the front end does differently is
spawn `Emu<AudioMachine>` rather than `Emu<Spectrum>`; the debugger, the
commands and the events are untouched. When 0027 lands it will checkpoint the
`Spectrum` and not the beeper, so restoring clicks at the seam. That is the
right trade: the alternative is a filter's state inside the thing whose
bit-for-bit equality 0029 exists to check.

### Time is counted in units smaller than a T-state

A sample is not a reading of the speaker but the average level over the window
it covers, computed exactly from the edges inside it. The window boundaries
have to be exact, and not merely unbiased. Computing `i * clock / rate` in
whole T-states moves each boundary by up to half a T-state, and reconstructing
samples off a uniform grid as though they were on one is the same thing as
adding noise: measured against a 7 kHz square wave it put a floor 61 dB down —
*worse* than the images the oversampling was there to remove, and worse the
faster the windows ran.

So time is counted in T-states divided by the window rate. A window is then
exactly `clock_hz` of those units, a T-state is exactly `inner_rate` of them,
and both the boundaries and the edges land on whole numbers. The counter is
`u128` because the product of the two rates overruns a `u64` after about a year
of continuous emulation. The floor dropped to 84 dB, and started improving with
oversampling instead of degrading.

### Volume is applied by the consumer

Not upstream, where it would be easier. Volume is host state, so getting it
onto the emulation thread means either putting it through the command ring —
which ADR-0017 reserves for input the emulation thread could not have worked
out for itself, and which would write a volume knob into the replay log as
though it were a keystroke — or having that thread read a shared atomic
mid-run, which is the same leak through a different hole.

It also costs a buffer's depth of latency, and it is what keeps the ring's
contents a pure function of the machine, which is what lets the spectrum tests
measure the resampler without knowing the gain and would let 0029 compare a
replay's audio against the original's.

## Consequences

**Positive:**
- `Spectrum` stays plain replayable data, and gains 8 KB rather than a
  dependency on the host's audio configuration.
- The resampler and the filters are testable without a machine, at rates chosen
  to make the answers exact.
- The seam between this ticket and 0019 is a sample ring and a `Volume`, both
  of which exist and are tested, so the front end has nothing to design.
- The ring's fill level is a pacing signal the front end gets for free: an
  emulator paced by its audio buffer needs no other clock and never has to
  correct for drift.

**Negative:**
- A second `Machine` monomorphisation, so `docs/architecture.md`'s caution
  applies — any audio-cost figure has to be a ratio measured inside one binary.
- Delegating `Bus` means delegating the machine-cycle wrappers as well as the
  raw accessors, because the `Spectrum` overrides them for contention (0020)
  and a wrapper left on the trait's default body would quietly run an
  uncontended machine.
- Restoring a checkpoint will click, because the beeper is not in it.
- `rkw-audio` cannot use `crossbeam_utils` for ADR-0010's cache-padding
  constant, so it hard-codes the pessimistic 128 bytes instead. That is a
  deliberate deviation and the only one.

**Caveat:**

The device's sample rate is fixed when the `AudioMachine` is built, because the
beeper lives on the emulation thread and a reconfiguration command must not
enter the replay log. Changing devices mid-session therefore has nowhere to go
today. If it ever needs to: render at a fixed internal rate on the emulation
thread and decimate to the device's rate in the callback, which moves every
rate-dependent piece of state to the consumer where it belongs.
