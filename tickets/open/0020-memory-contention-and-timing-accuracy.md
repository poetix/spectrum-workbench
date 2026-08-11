---
id: "0020"
title: Memory contention and timing accuracy
priority: medium
created: 2026-08-11
---

## Summary

The ULA's contention of CPU access to the lower 16K of RAM, computed rather
than tabulated (ADR-0009), plus the floating bus and I/O contention.

## Acceptance criteria

- [ ] Contention delay computed arithmetically from an 8-byte pattern table
- [ ] Applied only to addresses 0x4000-0x7FFF and only during the display
      period
- [ ] I/O contention for ports, which follows different rules to memory
- [ ] Floating bus reads return the byte the ULA is fetching
- [ ] Frame constants verified against a published reference before landing
- [ ] Test: a timing-sensitive demo effect renders correctly

## Notes

The machine-cycle-granular bus (ADR-0002) means this should touch no
instruction implementation — only the `Bus` implementation. If it turns out
otherwise, something in the core is wrong.

The constants in `docs/architecture.md` are from memory and must be checked.
