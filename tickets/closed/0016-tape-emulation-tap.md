---
id: "0016"
title: "Tape emulation: TAP"
priority: medium
created: 2026-08-11
closed: 2026-08-14
---

## Summary

Load and save in TAP format, both through ROM routine emulation (fast) and by
replaying the audio waveform (accurate).

## Acceptance criteria

- [x] TAP parsing: block structure, flag and checksum bytes
- [x] Waveform replay driving the EAR bit with correct pulse timings
- [x] The ROM loader loads a real tape image successfully
- [x] Saving produces a TAP a real Spectrum would load
- [x] Optional accelerated loading by trapping the ROM load routine
- [x] Test: round-trip save then load recovers identical data

## Notes

Waveform replay must come first even though trapping is faster, because a lot
of software uses custom loaders that never call the ROM routine.

## As built

`crates/rkw-tape`, a deck in `Spectrum`, and a second wrapping machine. See
[ADR-0022](../../adr/0022-the-tape-is-a-waveform-and-lives-in-the-machine.md)
for why the pieces fall where they do.

### The tape is a waveform, and the file format is a detail of it

`rkw-tape` is three modules and no dependencies. `tap` is the container —
length-prefixed blocks, a flag byte, an XOR checksum — and parsing it is
validation only: a `Tap` keeps the bytes it was given and an index of where its
blocks are. `pulse` is what the machine actually consumes, a `Player` yielding
pilot, sync, two pulses a bit, a tail and a pause. `save` is the way back.

Pulse lengths are a `Timing` struct rather than constants, which is what 0017's
TZX needs and what let the arithmetic be tested at numbers small enough to work
out on paper — a pilot of 100 T-states and a bit of one or two.

The `Player` is a block index and a phase with the tape passed in, not an
iterator borrowing one, because it lives inside a machine that 0027 will clone
into a checkpoint ring and a borrow has nowhere to live across a clone. It is
sixteen bytes.

### The deck is machine state; the recorder is not

`Spectrum` gains a `Tape`: an `Arc<Tap>`, the player, the T-state the pulse in
progress ends at, and the level it put on `EAR`. That is the opposite of what
ADR-0021 did with the beeper, and the difference is whether the machine's own
behaviour depends on it: a loader in the middle of timing a pulse is machine
state, and a checkpoint restored without the tape position resumes into a
measurement of a waveform that is no longer playing.

Saving is an output, so it is outside — and it costs nothing to put there,
because the `MIC` edges it needs are already in the log the beeper drains.
`Saving<M>` wraps a machine, reads the frame's `MIC` transitions before the log
rolls on, and decodes them back into blocks. It is generic, so
`Saving<AudioMachine>` and `Saving<Spectrum>` are both machines.

### Edges arrive as scheduled events

`next_event` returns the earlier of the frame interrupt and the next tape edge.
A running tape stops the slice loop every few hundred T-states — about forty
times more often than a frame — and costs nothing per instruction; a stopped
one costs a branch.

An edge lands late by however far the last instruction overran its deadline,
which is twenty-odd T-states against a data pulse of 855. What matters is that
it does not accumulate: each pulse is added to the edge that was *due* rather
than to the clock that arrived, so a block's thousandth pulse is where the
waveform says it is and not a millisecond downstream.

### `service_event` had to learn what woke it

It used to mean "the frame ended", and the beeper was built on that. With two
schedules it means "something was due", so `Spectrum::frame_due` is the
question a wrapper now asks before doing per-frame work. Missing it renders a
frame of sound per tape pulse — audible immediately, and only while a tape is
running, which is not where anyone would look for it.

### The tail turned out to be load-bearing

A loader reads a bit by timing between two edges, so the last bit of a block is
not readable until an edge arrives after it. The ROM leaves one about 945
T-states past the last bit; a waveform generated without it hands the loader
half a bit and a silence. It cannot be left to the pause either, because a
pause is a level and the last data pulse may already be at it. The recorder has
the mirror of the problem: the tail is 945 against a zero of 855 and a one of
1710, so it reads as half a zero at exact timings and as *neither* on a
recording running a fifth slow — a block thrown away for its own last edge.
A pulse that is not a bit, arriving where a block is entitled to end, ends the
block instead of failing it.

### Discard rather than truncate

The recorder's buffers are fixed, because it runs on the emulation thread. A
block that overran them, stopped in the middle of a byte, or contained a pulse
that was neither a zero nor a one is thrown away and counted, never written out
short: a truncated block in a TAP file is a corrupt tape that looks fine until
somebody tries to load it, which is the failure worth going out of the way to
avoid.

### The ROM is the test

`tests/tape.rs` skips without a ROM like everything else here, and the tests
that need one are the ones that matter:

- `LD-BYTES` called directly at a played tape loads 256 bytes and returns with
  carry set; a tape with one bit flipped in the middle returns with it clear.
- `LOAD ""` typed at the BASIC prompt reads a header and a program off the
  waveform and reports `0 OK`, with the program where `PROG` says it is.
- What the ROM's `SA-BYTES` writes is recorded off the `MIC` bit, and it is bit
  for bit the TAP this crate would have written for the same data.
- That recorded tape is then mounted and loaded back by the ROM, which is the
  round-trip criterion with a real loader at both ends.

`tests/no_alloc.rs` gains a third case, `Emu<Saving<Spectrum>>` with a tape
running, for the reason the beeper needed its own: the per-frame work of a
wrapper is not reached by a test that runs the machine underneath it.

### The trap

`ld_bytes` does what `LD-BYTES` would have done and returns in the same
T-state: flag byte, length, checksum, `IX` advanced, `DE` zeroed, carry set or
clear. It is a function a front end calls when the program counter reaches
`LD_BYTES`, not a hook in the bus, because a host-side convenience has no
business inside the thing whose bit-for-bit equality 0029 exists to check.

It is a convenience over the waveform rather than an alternative to it, and
the ticket asked for it in that order for a good reason: a trap works for
exactly the software that calls the ROM, which excludes most of the commercial
catalogue, and when it fails it fails invisibly — the tape simply never loads,
with nothing to say the program was never going to call the routine being
trapped.

### Found while building

- Recording bounded at the frame end rather than at the clock. The edge log
  carries edges that overran the frame and `roll` rebases them onto the next
  one, so a recorder reading to the end of the log would see those edges twice
  — and a duplicated pulse inside a block is a byte that decodes to something
  else.
- The `Timing` pause is a second, so a two-block tape is two seconds of
  emulated silence. Tests that do not want it use `with_pause`; the ROM tests
  keep it, because that is what a real tape has and the ROM's caller uses it.

### What is not here

- **Mount and play are not in the command log.** They are direct calls on the
  machine, so a session that loads a tape does not replay yet. `Command` is a
  sixteen-byte record and cannot carry a path; naming the tape by the content
  hash that already exists belongs with 0026, which is where log integrity
  lives. ADR-0017's table stands; this is the part of it that is owed.
- **No front end.** `rkwdbg` still has no way to press play, for the same
  reason. Ticket 0019's front end and 0026's command work are where the buttons
  go.
- **Loading is silent.** A real 48K machine feeds the `EAR` input into its own
  amplifier, which is why loading screeches. That is a third level in the edge
  log and belongs with the front end that could play it.
- **TZX is 0017**, and is a second `Timing` and a second source of pulses
  rather than a second design. Custom-loader pulse trains, turbo timings and
  the block types TAP cannot express all live there.
