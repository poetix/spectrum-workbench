---
id: "0008"
title: Debugger core: tiered breakpoints and watchpoints
priority: high
created: 2026-08-11
---

## Summary

Breakpoint and watchpoint storage and lookup, structured so the per-instruction
cost is a predictable branch and a bit test rather than a hash probe
(ADR-0008).

## Acceptance criteria

- [ ] Three tiers: armed flag, 8 KB address bitmap, detail map
- [ ] Execution breakpoints, with optional condition and hit count
- [ ] Memory watchpoints, separate read and write bitmaps, checked in the `Bus`
      path
- [ ] Port I/O watchpoints
- [ ] Benchmark: with nothing armed, throughput is within a few percent of the
      bare core; with breakpoints armed but not hit, still no hash lookups
- [ ] Step, step-over (uses 0007 for instruction length), step-out, run-to-
      cursor

## Notes

Step-over sets a temporary breakpoint at the address after a `Call` or `Rst`
and resumes; step-out needs the return address, which means tracking stack
depth or reading it from `SP` at the point of entry. Both use `disasm::Flow`.
