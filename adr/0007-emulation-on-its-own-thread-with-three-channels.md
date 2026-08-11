---
id: "ADR-0007"
title: Emulation on its own thread with three channels
date: 2026-08-11
status: accepted
---

## Context

Measured on the development machine, the core runs at roughly 360× real time:
a 3.5 MHz Spectrum retires about 432,000 instructions per second, and this core
does 157 million. There is no performance problem in the CPU itself, and none
available either — it is a linear state machine, and the emulated machine's
memory layout is not ours to choose.

The entire headroom is nonetheless spendable, by doing work *per instruction*
on behalf of the debugger. A `HashMap` probe per step, a formatted trace line
per step, or a mutex acquisition per step would each consume most of it.

Debugging also needs data to flow both ways: observations out, control in.

## Decision

Emulation runs on its own thread in slices bounded by a T-state deadline, the
deadline being the earliest of the next scheduled hardware event and the next
control tick. Three channels connect it to the debug/UI thread, with
deliberately different guarantees:

| Channel | Direction | Guarantee | Volume |
| --- | --- | --- | --- |
| Events | out | Lossy, overwrite-oldest, drop count reported | High |
| Stop | out | Lossless — a state transition, not a ring entry | Rare |
| Commands | in | Lossless, applied at control rate | Rare |

The event ring is single-producer single-consumer, 16-byte records, power-of-
two capacity. It never blocks the producer; on overflow it overwrites and
increments a drop counter the consumer reports.

Stop notifications do not go through that ring — a missed breakpoint is a
broken debugger. A stop is a release-store of `RunState::Paused` plus a
`StopReason`, after which the thread parks.

Commands are drained once per scanline (224 T-states, about 64 µs). They do not
need to be applied synchronously; the one case that must be exact — a
breakpoint set while stopped — falls out free, because the queue is always
drained before resuming.

Nothing on the emulation thread allocates.

## Consequences

**Positive:**
- The hot loop's only debugger cost is a deadline comparison it needs anyway
  and a bit test (ADR-0008).
- The UI cannot stall emulation, and emulation cannot stall the UI.
- Control latency of 64 µs is far below perception, at under 1% overhead.
- The slice loop is the same event-scheduler shape the ULA needs regardless,
  so control polling costs nothing structurally.
- Commands are applied at deterministic points, because the control tick is a
  T-state deadline rather than a wall-clock moment. Stamping each applied
  command with its T-state makes a recorded command log replay exactly — "it
  crashed after I poked that byte" becomes a reproducible test case.

**Negative:**
- Three channels is more machinery than one. Collapsing them would force the
  weakest guarantee onto all of them, which is why they are separate.
- Lossy tracing means a trace can be incomplete. Mitigated by reporting the
  drop count, never presenting a gappy trace as though it were whole.
- The no-allocation rule constrains code that would otherwise be simpler —
  notably it requires splitting the disassembler (ticket 0007).
