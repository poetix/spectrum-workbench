---
id: "0036"
title: Latch the keyboard to the frame boundary
priority: medium
created: 2026-08-15
closed: 2026-08-15
---

## Summary

A matrix handed over by a frontend is applied the instant the command is
drained, which can be part way through the ROM's keyboard scan. Latch it to the
top of the next frame instead.

## Acceptance criteria

- [x] A matrix set by a frontend takes effect at the next frame boundary, not
      immediately
- [x] A second matrix within one frame replaces the first rather than queueing
- [x] A frame nobody set a matrix in leaves the one in force alone
- [x] Tests and debuggers can still put a key down now, without the latch
- [x] `Command::Keys` remains the frontend's only input path (ADR-0024)

## As built

`Ula::set_keyboard` latches into a `pending: Option<Keyboard>`, and
`Ula::end_frame` applies it. `Machine::set_keys` — the far end of
`Command::Keys` — calls it, so the whole of a frontend's input goes through the
latch and nothing else has to know. `Ula::keyboard` stays public and immediate,
which is what the tests and the debugger use to put a key down at a T-state of
their choosing.

### Why

The ROM does not read the keyboard in one go. `KEY-SCAN` at `$028E` walks a
single low bit up through the eight half-rows — `CAPS SHIFT`'s first at
`$FEFE`, the `0`-to-`6` row fifth at `$EFFE` — and several hundred T-states
separate the two reads. A matrix that changes in between is read half from each
side of the change, so a combination whose keys are on different half-rows
arrives as one key.

`DELETE` is exactly that combination: `CAPS SHIFT` and `0`. Press it in the
wrong few hundred T-states of a frame and the scan sees the `0` without the
shift above it, which is not `DELETE` but the digit — so a `0` is typed, and
the two presses after it delete the `0` and then the character the user meant.
Latching to the frame boundary, which is where the interrupt that runs the scan
is raised, makes the matrix constant for the whole of every scan.

### What was found

The work was written before ticket 0019 and sat uncommitted while 0019 and 0021
landed on top of it, so it arrived needing three separate reconciliations.

**It had its own entry point, and 0019's is better.** The original was an
inherent `Spectrum::set_keys(&HostKeys, &KeyMap)` called directly by a
frontend. 0019 has since routed input through `Command::Keys` and the lossless
command ring, which is what makes a recorded session replay with the typing in
it (ADR-0024, tickets 0026 and 0029). The inherent method is dropped and the
latch sits behind the command instead; the `HostKeys` to matrix conversion
happens in the frontend, where 0019 put it.

**It changed a contract three of 0019's tests assert.** `tests/input.rs` sent a
matrix, resumed, and read the keyboard from a program that ran entirely inside
the first frame — which under the latch reads the matrix from before it. The
program now spends 73,607 T-states in a delay loop first, which is one frame
and a bit. That is not a workaround: a real program scans from the interrupt
handler, which is to say just after a frame boundary, so the test now does what
the thing it is testing does.

**`Ula::set_ear` had changed under it.** Ticket 0021 made it `Option<bool>` to
model the speaker feeding back into the `EAR` input. Mechanical, but it is why
the branch would not compile.

### What was rejected

**Applying the matrix on the command, and latching only the read.** The ULA
could have kept one matrix and made `read_port_fe` answer from a snapshot taken
at the frame boundary. Same effect, but it puts a branch on the port read —
which is on the hot path — to save one on `end_frame`, which runs fifty times a
second.

**A queue of matrices.** Host key events between two frames are a state, not a
sequence: a key pressed and released inside a single frame is a keypress the
hardware would not have shown the ROM either, and replaying a queue would put
keypresses into a machine that never saw them.

### What is not there

`Ula::latched_keyboard` has no caller outside its own test. It reads back what
a frontend last said, which a frontend does not need because it holds that
state itself — it is there because a value you can only write is awkward to
reason about, and the debugger pane of ticket 0025 is the likely first user.
