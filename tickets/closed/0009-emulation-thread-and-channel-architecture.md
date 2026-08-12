---
id: "0009"
title: Emulation thread and channel architecture
priority: high
created: 2026-08-11
closed: 2026-08-12
---

## Summary

Move emulation onto its own thread driven by a T-state slice loop, with the
three channels described in ADR-0007: a lossy outbound event ring, a lossless
stop notification, and a lossless inbound command ring drained at control rate.

## Acceptance criteria

- [x] `run_slice(deadline)` loop, deadline being the earliest scheduled event
- [x] Outbound SPSC event ring, 16-byte records, overwrite-oldest with a drop
      counter surfaced to the consumer
- [x] Stop notification as a state transition, not a ring message
- [x] Inbound SPSC command ring drained once per scanline
- [x] Producer and consumer indices padded apart using a per-target constant
      (ADR-0010), not a hardcoded 128
- [x] No allocation on the emulation thread; enforced by test or by inspection
- [x] Commands stamped with the T-state at which they were applied, so a
      command log replays deterministically

## Notes

The deterministic replay property is easy to lose by applying a command at
whatever moment is convenient. It is worth a test that replays a recorded
command log and asserts an identical final state.

## As built

Four modules in `rkw-debug`: `ring` (the queue both channels are made of),
`event` and `command` (what travels on them), and `emu` (the loop). `machine`
adds the two things the loop needs of a machine beyond a bus — what the time is,
and when the next hardware event is due.

### The loop is a struct, and the thread is twelve lines

`Emu` owns the CPU, the machine and the debugger, and `Emu::slice` is one
control tick: drain the command ring, then run to the earlier of the next
scheduled hardware event and the next control tick. `spawn` puts `Emu::run` on a
thread and hands back a `Handle` and a `JoinHandle` that gives the whole machine
back when it quits.

Keeping the two apart is what made the interesting properties testable. The
allocation test calls `slice` inside a counting allocator and knows exactly what
ran; the replay test drives thousands of slices with no thread and no timing;
the throughput benchmark measures the loop rather than the scheduler. A front
end with its own event loop can drive `slice` itself and never spawn anything.

The deadline is a floor rather than an exact stop, because instructions are not
interruptible: the last instruction of a slice overruns by up to twenty-odd
T-states, and the next deadline is measured from where the clock actually got
to. `Debugger::run_until` neither arms nor clears anything, which is what lets a
step-over that takes a million T-states be one arming and several thousand
slices — its landing site has to survive every one of them.

### Movement commands arm; the loop carries them out

`step_over`, `step_out` and `run_to` were written in ticket 0008 as
run-to-completion methods, which a slice loop cannot call. Each is now split
into an arming half and a run: the public methods are the two halves back to
back, and the emulation thread uses the arming half and lets its own loop do the
running. A single step is the exception — it is one instruction, and it is over
before the command has finished being applied.

### Commands name things by address, not by id

Breakpoint ids are minted on the emulation thread, so a command that created one
would have to send the id back, and the way back is lossy. Every command
therefore names what it means by address, which the sender already knows.
Asking the machine questions is a request/response problem rather than a queue
one and belongs to the command layer of ticket 0010.

### Determinism, and the test that would notice losing it

A command is applied at a control tick, and a control tick is a T-state rather
than a wall-clock instant, so "when a command took effect" is a number the
machine agrees with. Each applied command is stamped with it, into a `Vec` whose
capacity is reserved before the thread starts and never grown — a log that fills
up stops recording rather than becoming the thing that breaks the rule it exists
to check.

`Emu::replay` applies a recorded log to a fresh machine, running to each stamp
in turn. The test compares registers, all 64 KB of memory and the clock, and a
companion test moves one poke a lap later and requires the result to differ —
without it the first test would pass just as happily against a replay that
ignored the stamps entirely.

### The ring, and the bug the two-threaded test found

One ring type serves both directions; what differs is what happens when it is
full. The event ring overwrites the oldest record and counts what it lost; the
command ring refuses the write and hands the command back. A slot is two
`AtomicU64` and a record is anything that encodes into that pair, which is how a
lock-free ring is written under a workspace-wide `forbid(unsafe_code)`.

The producer never reads the consumer's index, so the consumer has to establish
for itself that the record it copied out was not being overwritten while it
copied it: it re-reads the producer index afterwards. That check is only sound
with an acquire *fence* between the copy and the re-read, and the first version
did not have one — an acquire load orders what follows it, not what precedes it,
so the relaxed reads of the record were free to be answered after the check that
was meant to validate them. The two-threaded test in `tests/ring.rs` found it
within a hundred records, because every record carries its own index twice over
and the consumer asserts that what comes out is in order. Written up as
ADR-0018.

The consumer treats the record a whole lap behind the producer as already lost,
so a ring of sixteen delivers fifteen. That is the price of not having a
sequence number per slot; the lossless side pays nothing for it, because
`try_push` refuses one record earlier and so never reaches the lap where the
question arises.

Head, tail and the drop counter are `crossbeam_utils::CachePadded` (ADR-0010).

### A stop is a store

Publishing a stop is two relaxed stores of the encoded reason, a release store
of the run state, and a release store of a counter, after which the thread
parks. The counter is what lets `Handle::wait_for_stop` mean "stop again":
a machine that is already paused is the normal state of a debugger, so a wait
that returned on the current state would return immediately after every resume
and report the stop before it.

Dropping the handle asks the thread to quit, because a parked thread with
nothing left to wake it is a leak.

### Measured

A control tick per scanline costs nothing measurable — the sliced loop comes out
a few percent *ahead* of a free-running one, which is the inlining effect from
ticket 0008 rather than slicing paying for itself, so the figures are bounds.
It takes a control tick every sixteen T-states, roughly every second
instruction, before a cost is visible at all. The 1% in ADR-0007 was
pessimistic. Numbers in [docs/architecture.md](../../docs/architecture.md).

The slice loop does not allocate: two thousand slices with commands applied,
events pushed, both rings overflowing and the log filling up, measured with
`alloc-check` at zero. Arming still allocates, and still may — it happens when a
person types.
