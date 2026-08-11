# Performance analysis: where the headroom is and how it gets spent

This document holds the measurements and reasoning. The decisions that follow
from them are recorded separately as ADRs, which this links to rather than
restating:

- [ADR-0007](../adr/0007-emulation-on-its-own-thread-with-three-channels.md) —
  emulation on its own thread, three channels
- [ADR-0008](../adr/0008-tiered-breakpoint-lookup.md) — tiered breakpoints
- [ADR-0009](../adr/0009-compute-contention-rather-than-tabulating-it.md) —
  contention computed, not tabulated
- [ADR-0010](../adr/0010-cache-padding-is-a-per-target-constant.md) — cache
  padding per target

## There is nothing to optimise in the core

The emulated CPU is a strictly linear state machine: no vectorisation and no
parallelism to be had inside it. Nor is there much of a memory layout to win
back, because the layout is not ours to choose — a Z80's address space is flat,
and the emulated program decides what it touches.

Our own structures are fixed and small:

|   |   |
| --- | --- |
| `Regs` | 34 bytes |
| `Cpu` | 48 bytes |
| Spectrum address space | 64 KB |

How that lands against a cache hierarchy depends on the target, and the
difference is not cosmetic:

| | Apple M3 (dev machine) | Typical x86-64 desktop |
| --- | --- | --- |
| L1d per core | 128 KB | 32-48 KB |
| Cache line | 128 B | 64 B (128 B effective) |
| L2 | 16 MB | 256 KB - 2 MB |
| 64 KB address space | fits in L1d twice over | **does not fit in L1d**, fits in L2 |

On the development machine the entire emulated computer sits in L1. On an x86
desktop it does not, and anything outside the current working set of code,
stack and screen falls back to L2.

This changes no decision — there is nothing to reorganise either way — but it
does mean the rule *do not add large auxiliary tables to the hot path* binds
considerably harder on x86, where such a table competes for a cache a quarter
the size. **Sizing arguments assume the smaller machine.**

## How much headroom there is

From the `zexall` run: 46.7e9 T-states across 5.76e9 instructions, so 8.1
T-states per instruction, so a real 3.5 MHz Spectrum retires about 432,000
instructions per second.

This core does 157 million on the dev machine — roughly **360× real time**.
That figure is indicative, not a specification; a given x86 part will differ,
plausibly by a factor of two either way. The margin is large enough that the
conclusion survives regardless:

> The only thing that can plausibly take that headroom away is work done per
> instruction on behalf of the debugger.

A `HashMap` probe per step, a formatted trace line per step, or a mutex
acquisition per step would each consume most of it. Hence ADR-0007 and
ADR-0008.

## Worked numbers behind the decisions

**Control tick cadence.** One scanline is 224 T-states, about 27 instructions,
or 64 µs of emulated time. Draining an empty command ring is a single acquire
load of the producer index — a couple of nanoseconds against roughly 160 ns of
emulated work, so under 1% overhead, bounding command latency well below
perception.

**Trace volume.** Full instruction tracing at real-time speed is 432K records
per second at 16 bytes each: 7 MB/s, which is nothing. At full uncapped speed
it would be 2.5 GB/s, which would dominate everything. Tracing must be opt-in,
and when it is on the emulator is generally running near real time anyway.

**Ring sizing.** 64K records × 16 B = 1 MB, comfortably L2-resident on either
target.

**Breakpoint bitmap footprint.** 8 KB is a quarter of an x86 L1d, which sounds
alarming and is not: the bitmap is touched *sparsely*, one bit per executed
address, so only the lines covering currently-executing code become resident —
typically one or two. The 8 KB is address space, not working set.

**Contention table.** One entry per T-state per frame is 69,888 entries, 68 KB.
On the dev machine that is half of L1d; on a typical x86 desktop it is larger
than the whole of it, streamed continuously alongside the emulated RAM. The
periodic form costs eight bytes.

## What the debugger turned out to cost

Ticket 0008 built the tiers and then measured them
(`crates/rkw-debug/tests/throughput.rs`). Two findings, one expected and one
not.

**The tiers cost nothing, as designed.** Attaching the debugger with nothing
armed, and arming execution breakpoints that never fire, both measure at 0.99
to 1.01 of the bare core. The per-instruction addition is a predictable branch
and a bit test, and it does not show up above the noise.

**Watching memory costs more than watching execution, and not for the reason
you would guess.** A memory or port watchpoint has to see every bus access, so
the CPU is monomorphised against a wrapper that reaches the real bus through a
pointer. The bit test that wrapper carries is free; the indirection is not, and
it costs about 40%. So a run with no watchpoints armed is given the machine's
own bus and pays nothing at all — the decision is made once per run, which is
sound because commands are applied between slices rather than mid-slice.

**A caution about quoted throughput figures.** When a binary contains exactly
one call site for `Cpu::step` at a given bus type, LLVM inlines the entire
interpreter into that loop — inlining into a sole call site costs no code
growth — and it runs at about 270 M instructions/s on the dev machine. A second
call site anywhere in the binary and both drop to about 165, a difference of
60% for identical source. Any figure quoted for this core is meaningless
without saying which regime it was measured in, and a benchmark that compares
two loops in the same binary has already lost the effect for both. Comparisons
here are therefore ratios measured within one binary.

## Keeping the no-allocation rule testable

ADR-0007 ends with "nothing on the emulation thread allocates", which is the
kind of rule that decays quietly: a `Vec` added inside a decode breaks no test
and costs nothing visible until something is measured under load.

`crates/alloc-check` makes it an assertion instead. It is a global allocator
that counts allocations *per thread* — a process-wide counter would mostly be
measuring the test harness — and a `count` function returning what a closure
allocated. A test installs it, measures the path in question, and asserts zero,
having first asserted that something known to allocate does, so an uninstalled
allocator fails loudly rather than passing everything vacuously.

It is a separate crate for one reason: the workspace forbids `unsafe_code`, and
`forbid` cannot be lifted by an inner `allow`, so the single `unsafe impl
GlobalAlloc` lives in the one crate that opts out of the workspace lints and is
a dev-dependency of nothing that ships.

The first user is the disassembler's decode path (ticket 0007). The trace ring
and the emulation thread proper are the ones it exists for.

## Performance builds

`[profile.release]` uses fat LTO and a single codegen unit. An interpreter is
one hot loop whose inner workings are spread across `cpu`, `alu` and `bus`, so
cross-module inlining is worth more here than in typical code, at the cost of
slower builds.

For a machine-specific build:

```sh
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

That is a local optimisation only — it produces binaries that will not run on
older parts of the same architecture, so it belongs in a developer's shell and
not in the committed configuration.
