---
id: "ADR-0010"
title: Cache padding is a per-target constant
date: 2026-08-11
status: accepted
---

## Context

The ring buffer's producer index, consumer index and run-state word are written
by different threads. If any two share a cache line, every write by one thread
invalidates the other's copy and a nearly-free ring becomes a bus-traffic
generator. They must be padded apart.

The development machine reports `hw.cachelinesize: 128`, and an early draft of
the design justified `#[repr(align(128))]` on that basis. That reasoning is
wrong twice over. It is a measurement of one laptop, and the number it produces
happens to be right on x86-64 for an entirely different reason — the line size
there is 64 bytes, but Intel's spatial prefetcher pulls lines in adjacent
pairs, so two variables 64 bytes apart still interfere.

## Decision

Take the padding from `crossbeam_utils::CachePadded` rather than writing our
own. Its table:

| Target | Padding |
| --- | --- |
| x86-64, aarch64, powerpc64 | 128 |
| s390x | 256 |
| arm, mips, riscv64, sparc | 32 |
| everything else | 64 |

If the dependency is unwanted, replicate the same table behind
`cfg(target_arch)`. What must not appear is a bare `#[repr(align(128))]`
justified by a local measurement.

More generally: sizing arguments assume the *smaller* of the cache hierarchies
we target, not the development machine's. The 64 KB emulated address space fits
in an Apple M3's 128 KB L1d twice over and does not fit in a typical x86 L1d at
all. Design to the smaller machine.

## Consequences

**Positive:**
- Correct on every target rather than on this laptop.
- The reasoning is recorded, so the next person to see `128` does not
  "simplify" it to 64.
- The general rule — design to the smaller cache — makes the argument against
  large hot-path tables (ADR-0009) stronger rather than accidental.

**Negative:**
- A dependency on `crossbeam-utils` for what looks like a constant, or a
  hand-maintained table that will drift from upstream.
- Padding to 128 bytes on a target that only needs 64 wastes a little space.
  Irrelevant at the handful of instances involved here.
