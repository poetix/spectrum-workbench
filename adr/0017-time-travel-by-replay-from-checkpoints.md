---
id: "ADR-0017"
title: Time travel by replay from checkpoints
date: 2026-08-12
status: accepted
---

## Context

"Run it backwards from the crash" is the one debugger capability this project is
unusually well placed to offer and does not yet have. A Z80 is small enough that
its whole state fits in a memcpy, and ADR-0007 already made the emulation thread
deterministic in the only way that matters — a command is applied at a T-state,
not at a wall-clock moment, and `Emu::replay` re-applies a stamped log to a fresh
machine and lands in the same place.

That is half of reverse execution. Going backwards to *t* is going forwards to
*t* from somewhere earlier. What is missing is the somewhere earlier, and a
guarantee that the forward run is a function of what was recorded.

There are two established designs and they are not close on this machine.

**An inverse-operation journal** records, per instruction, enough to undo it: the
overwritten register, the previous byte at a written address, the device state a
port read consumed. Stepping back is then popping the journal. It is O(1) per
step and it costs the hot loop on every instruction whether or not anyone ever
steps back. ADR-0007 measured that headroom precisely and spent it deliberately:
a `HashMap` probe, a formatted line, or a mutex per instruction each consumes
most of it, which is why ticket 0022 makes tracing opt-in. An undo record per
instruction at 157 M instructions/s is the same order of cost. It also does not
close: a port read changes the device, not only the register, and an inverse for
"the ULA was asked for the keyboard" does not exist.

**Re-execution from checkpoints** costs the hot loop nothing. The core runs at
about 360× real time, so one second of emulated history replays in roughly 3 ms
of wall clock. A checkpoint of a 48K machine is about 64 KB, so a minute of
history at one checkpoint per emulated second is 4 MB. Stepping back one
instruction is a checkpoint restore plus a replay bounded by the checkpoint
interval — milliseconds, not microseconds, but the user is a person and the
budget is a frame.

The second is obviously right here, and it reuses machinery that exists.

## Decision

### Backwards is forwards from a checkpoint

There is no undo journal. Reverse execution restores the newest checkpoint at or
before the target T-state, replays the recorded commands from there, and runs
forward to the target. The hot loop is not modified, and a session in which
nobody steps back pays only for checkpointing.

### The log records what the machine cannot recompute

Determinism is a property the machine has to hold up, not something the log can
buy. So the log records *nondeterministic* input only, and everything derivable
from machine state stays where it already is — the `next_event`/`service_event`
scheduler of `machine.rs`.

| Input | Path | Rate | In the log |
| --- | --- | --- | --- |
| Keystrokes (0013) | command ring | human | yes |
| Pokes, register writes, run control | command ring | human | yes |
| Tape mount, play, stop, seek (0016/0017) | command ring | a few per session | yes |
| Tape edges | `next_event` | ~2 kHz | no — regenerated |
| Frame interrupt (0012) | `next_event` | 50 Hz | no — derived |
| Contention (0020) | bus | per cycle | no — derived |

Routing tape *edges* through the command ring would be wrong twice over: the
ring drains once per scanline, which quantises a 855 T-state pulse to a 224
T-state grid and corrupts the loading routine's own timing measurement; and the
edge schedule is a pure function of the tape data and the T-state at which
transport started, which the emulation thread can compute for itself. The same
argument covers the frame interrupt and contention.

The rule is therefore: **an input crosses the command ring if and only if the
emulation thread could not have worked it out.** Anything reaching the machine by
another route makes replay a lie, silently.

### Recorded artefacts are referenced by content hash

A log that mounts a tape names it by hash, not only by path. Replay verifies the
hash and refuses on mismatch. A tape that was edited between the recording and
the replay is otherwise a divergence with no visible cause, which is the failure
mode this whole design most needs to avoid.

### The log must report its own truncation

The command log stops recording when full rather than growing, because growing it
would allocate on the emulation thread. A truncated log replays to a *different
machine* and looks exactly like a complete one — the one failure mode the ring
already refuses to have, since ADR-0007 requires the event ring's drop count to
be reported alongside anything built from it. The log gains the same discipline:
a drop count, and a `replay` that refuses a log known to be incomplete rather
than returning a machine that is quietly wrong.

### A checkpoint is whole-machine state, and is not ticket 0018

Checkpoints are in-memory, periodic, and captured on the emulation thread into
buffers allocated once. They are not the SNA and Z80 file formats: those are a
user feature, they are lossy by design — SNA restores `PC` from the stack — and
ticket 0018 says so. A checkpoint that loses `WZ`, `Q`, a pending `EI` or the
interrupt mode does not restore to the same machine, and the whole value of this
feature is that it does.

A checkpoint carries CPU and machine state, the run state, and the index it had
reached in the command log. It does *not* carry the breakpoint table, which is a
`HashMap` and would allocate. Instead, restoring re-applies the arming commands
(`Break`, `Unbreak`, `Watch`, `Unwatch`, `ClearAll`) from the start of the log —
they are ordered, they are idempotent with respect to machine state, and there
are as many of them as a person has typed. Commands that move the machine or
mutate it are applied only from the checkpoint's log index onwards, because the
checkpoint already has their effect baked in.

### Checkpoints are free assertions

Holding checkpoint *N* and checkpoint *N+1* means the interval between them can
be replayed and the result compared. That turns "replay diverged" from a wrong
answer nobody notices into a failing check with a T-state on it, and it is the
regression test that keeps 0012, 0013, 0016 and 0020 honest as they land.

## Consequences

**Positive:**

- The hot loop is untouched. ADR-0007's headroom argument is not reopened.
- Most of the work is already done: `Emu::replay` and its stamped log exist and
  are tested against whole-machine equality.
- Reverse execution has a front end waiting for it. DAP has `supportsStepBack`
  and `reverseContinue`, so ADR-0016's adapter gets the buttons for the cost of
  two request handlers.
- The determinism the design needs is worth having on its own: a crash becomes a
  log plus a hash, which is a bug report that replays.
- Post-mortem inspection (ticket 0018) gets better for free — the checkpoint ring
  means the state a few seconds *before* the breakpoint is also still there.

**Negative:**

- Memory. A minute of 48K history is 4 MB; a 128K machine with paging is more,
  and the cadence is the knob. Checkpointing is a copy on the emulation thread,
  budgeted at under 1% and to be measured rather than assumed.
- Stepping back is milliseconds, not instant, and the cost is set by the
  checkpoint interval rather than by the distance travelled. Stepping back a
  hundred instructions costs about what stepping back one costs.
- The determinism requirement now binds every future device. Audio (0014) and the
  windowed front end (0019) touch host state and wall-clock time, and the
  boundary between them and the machine has to stay clean or replay breaks.
- A divergence is hard to debug by nature: the symptom is a machine that is
  different, arbitrarily far from the cause. The self-check above is the
  mitigation and is not optional.
- History has an edge. Reverse-continue that runs out of checkpoints must say so
  as a distinct outcome, never as "no breakpoint was hit".
