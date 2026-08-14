---
id: "ADR-0024"
title: Input goes through the command log
date: 2026-08-14
status: accepted
---

## Context

A frontend has a keyboard and the machine has a matrix, and something has to
carry forty bits from one thread to the other. The obvious way is the cheap
one: put the matrix in an `AtomicU64`, have the window store to it and the ULA
load it on each read of port `0xFE`. Nothing is queued, nothing is dropped,
and the latency is zero.

The other way is the command ring of ADR-0007, which already carries
everything else the outside world asks the machine to do. It costs a control
tick of latency — 224 T-states, 64 µs — and a command's worth of ring.

The difference that matters is not latency. ADR-0007 says a command is applied
at a T-state rather than at a wall-clock instant, and that a log of stamped
commands replays exactly; `Emu::replay` and `tests/replay.rs` are that claim.
Ticket 0026 is going to record sessions and ticket 0029 is going to compare a
replay against the original run to find divergence. A keypress that arrived
through an atomic is invisible to both: the recorded log would replay a machine
nobody typed into, and it would diverge from the recording immediately, with
nothing in the log to explain why.

There is also the question of where the bits land. `rkw-debug` does not know
what a Spectrum is — that is the point of the `Machine` trait — so a command
that reached into a ULA would be the first thing in the crate that did.

## Decision

### A keypress is a command

`Command::Keys(u64)`: the whole matrix, every key that is down, as forty bits
above the record's kind byte. It goes through the lossless inbound ring, is
applied at the control tick like everything else, is stamped with the T-state
it was applied at, and lands in the command log.

### The state, not the change

The command carries the whole matrix rather than a press or a release. A
frontend rebuilds the matrix from the host keys it is holding (`HostKeys`), so
it has the whole thing to hand anyway, and sending state is idempotent where
sending edges is not: a lost release leaves a key down forever, and a lost
state is corrected by the next event. It is also what makes "the window lost
focus" expressible — a matrix of zero — rather than a special case.

### The machine decides what the bits mean

`Machine::set_keys(&mut self, matrix: u64)`, defaulting to doing nothing. The
debugger's crate carries the word and never interprets it; `Spectrum` unpacks
it into eight half-rows of five, and the wrappers — `AudioMachine`, `Saving`,
`Presenting` — delegate it down. A machine with no keyboard ignores it.

## Consequences

**Positive:**
- A recorded session includes the typing, so it replays: "it crashed when I
  pressed M" becomes a test case, and 0029's divergence check has something to
  compare.
- Input is applied at a deterministic point, so two runs of the same log take
  the same instruction boundaries.
- Nothing about a keyboard appears in `rkw-debug`, and nothing about a host
  appears in `Spectrum`.
- The frontend sends a command only when the matrix changes, so auto-repeat and
  modifiers the table has no use for cost nothing.

**Negative:**
- 64 µs of latency on a keypress, which is a fortieth of a frame and below
  what anybody can perceive, but is not nothing on paper.
- Forty bits is what fits above the kind byte in a sixteen-byte record. A
  machine with a wider matrix than a Spectrum's — a Spectrum +2's second
  joystick, say — needs a wider record rather than a wider field, and the
  encoder masks rather than growing silently.
- The frontend has to hold the matrix it last sent to avoid resending it, which
  is state a shared atomic would not have needed.
