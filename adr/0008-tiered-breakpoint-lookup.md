---
id: "ADR-0008"
title: Tiered breakpoint lookup
date: 2026-08-11
status: accepted
---

## Context

Every instruction must ask "is there a breakpoint here?". At roughly 6 ns per
instruction, a `HashMap<u16, Breakpoint>` probe at 20-50 ns is a five- to
tenfold slowdown — and it is paid whether or not any breakpoints are set, so
it penalises running the emulator normally.

Memory watchpoints are worse: the question is asked on every memory access,
roughly three times per instruction.

## Decision

Three tiers, cheapest first:

| Tier | Structure | Cost |
| --- | --- | --- |
| Is anything armed? | `bool` or count | Predictable branch; free when off |
| Is *this* address armed? | 8 KB bitmap, 65536 bits | One bit test |
| What kind of breakpoint? | `HashMap<u16, Breakpoint>` | Only on a bitmap hit |

Conditions, hit counts and source locations live in the map and are consulted
only when the bitmap says yes. Read and write watchpoints get their own
bitmaps in the `Bus` path.

The bitmaps are owned by the emulation thread and mutated only when applying
commands, so they need no atomics.

## Consequences

**Positive:**
- With nothing armed, the cost is one well-predicted branch.
- With breakpoints armed but not hit, the cost is one bit test.
- Rich breakpoint semantics — conditions, counts, source mapping — cost
  nothing until a breakpoint actually fires.

**Negative:**
- Three representations of the same information, which must be kept in step.
  Adding a breakpoint means touching the flag, the bitmap and the map.
- 16 KB of bitmaps (execute, read, write) is a quarter of an x86 L1d. This is
  not the problem it appears: the bitmaps are touched sparsely, one bit per
  accessed address, so only the lines covering current activity become
  resident. It is address space, not working set.
