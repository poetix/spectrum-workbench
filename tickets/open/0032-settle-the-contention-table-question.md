---
id: "0032"
title: Settle the contention table question on x86
priority: low
created: 2026-08-14
---

## Summary

ADR-0009 chose to compute contention rather than tabulate it, on the grounds
that a 68 KB table streamed alongside 64 KB of emulated RAM would evict the
emulated machine out of cache. Ticket 0020 measured it and got the opposite
answer.

## What was measured

`crates/rkw-spectrum/tests/throughput.rs` builds the table and runs the machine
against it:

| | computed | tabulated |
| --- | --- | --- |
| the lookup alone | 892 M/s | 2266 M/s |
| in the machine | 114 M inst/s | 150 M inst/s |

## Why that does not settle it

The ADR's argument has two conditions, and the measurement meets neither:

- It sizes against a 32-48 KB x86 L1d, where 68 KB does not fit. The
  development machine has 128 KB and the table fits.
- The emulated working set in the benchmark is 256 bytes. A real program
  touches its code, its screen, its stack and its data — kilobytes — and it is
  competition with *that* the table is supposed to lose.

## Acceptance criteria

- [ ] The same comparison run on an x86-64 part with a 32-48 KB L1d
- [ ] A benchmark whose emulated working set is a real program's rather than a
      256-byte loop — the ROM booting, or a game loading, rather than a
      synthetic loop
- [ ] ADR-0009 either confirmed with the numbers, or superseded and the table
      adopted
- [ ] If the table wins, `contention::delay` is the only thing that has to
      change; nothing outside that module knows how the answer is arrived at

## Notes

Not urgent. The arithmetic is correct either way, and the worst case is that
the emulator is 30% slower than it could be at something it already does at
about 340x real time.
