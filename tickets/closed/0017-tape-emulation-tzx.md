---
id: "0017"
title: "Tape emulation: TZX"
priority: low
created: 2026-08-11
closed: 2026-08-14
---

## Summary

TZX support, which is TAP plus a block type system covering custom loaders,
turbo loading, pure tones and direct recordings.

## Acceptance criteria

- [x] Standard speed data blocks (0x10) and turbo blocks (0x11)
- [x] Pure tone (0x12), pulse sequence (0x13), pure data (0x14)
- [x] Direct recording (0x15)
- [x] Flow control blocks: pause, jump, loop, group start/end
- [x] Text and archive info blocks parsed and displayed
- [x] Unknown block types are skipped using their length field rather than
      aborting the load

## Notes

Depends on 0016 for the waveform replay machinery. Lower priority: TAP covers
most of what one actually wants to run early on.

## As built

`crates/rkw-tape/src/tzx.rs` for the format, `image.rs` for the thing a deck
mounts, a rewritten `pulse.rs`, and `crates/rkw-spectrum/tests/tzx.rs` for the
ROM's opinion of the result.

### The player stopped knowing what a file is

0016's `Player` walked a `Tap` and asked it for block *n*'s bytes. TZX has ten
kinds of block and only three of them are bytes, so the player now asks the
image for a `Plan` — a data block and its lengths, a run of identical pulses, a
list of pulse lengths, a recording of the line, or an instruction to jump, loop,
set the level or stop — and plays that. TAP produces one variant of it and TZX
produces all of them.

That is the whole of the generalisation, and it is what ADR-0022 predicted when
it said 0017 would be "a second `Timing` and a second source of pulses, not a
second design". The plan is fetched per pulse rather than held in the player,
which keeps `Player` at forty bytes of `Copy` state for the checkpoint ring and
means an image swapped under a running player cannot leave stale timings behind.

Three things fell out of the shape rather than being written twice:

- **Pure data is a data block with no pilot and no sync.** Zero-length pulses
  are not emitted, so a pilot count of zero and a sync of zero simply skip those
  phases.
- **A pause block is a data block with no data.** The reason that is not a trick
  is that it is what a pause block is *for*: a data block written with no pause
  of its own has no edge to end its last bit, and the block after it supplies
  one.
- **TAP's tail is TZX's first millisecond of pause.** Both formats have to
  manufacture an edge after the last bit; the ROM leaves 945 T-states and TZX
  leaves a millisecond, so `Timing::tail_for` and `pause_for` split a pause in
  milliseconds into the two of them. `Timing` gained `ms` because a pause in
  milliseconds cannot be turned into T-states without the clock, which is the
  one thing a tape image cannot know.

### The deck holds an `Image` and can be stopped by the tape

`Image` is `Arc<Tap>` or `Arc<Tzx>` — the `Arc` moved into `rkw-tape` so that
mounting and cloning a machine both cost a reference count — and `mount_tape`
takes anything that converts into one, which left every existing call site
alone.

The new state is stopping. A TZX can ask the tape to stop in the middle of
itself, which is what a two-part game does between levels, and that is not the
same as running out: `Player::stopped` is distinct from `finished`, and
`Tape::play` resumes at the block after the one that asked. A deck that
conflated them would play the first half of every multi-load game and then sit
there.

### Loops do not nest and jumps can spin

The format says loops do not nest, so the loop state is a start block and a
counter rather than a stack. A jump to its own block is the format's way of
writing "loop forever", and a file that does that with no waveform inside the
loop would spin the player without ever producing a pulse — so control blocks
crossed in one call are capped, and a tape that has made no sound in 65536 of
them has run out. `Player::duration` has the same problem from the other end and
the same answer: a tape that loops forever has no duration, and the walk gives
up rather than hanging.

### Direct recordings say what the level was

Every other block flips the line; a recording states it. Runs of equal samples
are merged into one pulse, which matters more than it looks: a recording sampled
at 70 T-states would otherwise put fifty scheduled events on the emulation
thread for one millisecond of silence.

### The ROM is the test

The unit tests check that each block produces the pulses the format says it
does, which is not the same as checking that those pulses are a tape. So
`tests/tzx.rs` plays them at the real `LD-BYTES`:

- a standard block behind an archive info and a text block, which the loader
  must never hear;
- a turbo block whose pilot is 2500 pulses — a number that exists nowhere but in
  that block, and under about 1900 the ROM's `LD-START` delay outlasts the pilot
  and nothing loads;
- a block assembled out of a pure tone, a two-pulse sequence and a pure data
  block, which is what a custom loader's tape looks like written down;
- that same waveform sampled at 70 T-states and put back as a direct recording,
  which is the ROM absorbing up to 70 T-states of quantisation on every edge;
- `LOAD ""` from BASIC off a TZX carrying a group, a message and an unknown
  0x19 block;
- a tape that stops itself, and starts again into the block after.

### What "displayed" turned out to mean

Text and archive info are parsed (`Tzx::archive_info`, `Tzx::descriptions`) and
`Display for Tzx` prints the listing: version, title, publisher, the
descriptions and a line per block. Nothing shows it to a person yet, because
mounting a tape is not a command in `rkwdbg` at all — that belongs with the
front end of 0019, and the string it will print exists and is tested.
