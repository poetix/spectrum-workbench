---
id: "0022"
title: Trace ring and instruction history
priority: low
created: 2026-08-11
---

## Summary

Record recent execution so "how did I get here" is answerable. A ring of
compact records on the event channel, formatted only on the debug thread.

## Acceptance criteria

- [ ] 16-byte trace records: PC, raw instruction bytes, length
- [ ] Ring sized to stay L2-resident on the smaller target
- [ ] Tracing is opt-in; disabled costs nothing measurable
- [ ] Drop count surfaced, so a truncated trace is never shown as complete
- [ ] `backtrace`-style command showing the last N instructions disassembled
- [ ] Optional filtering by address range so a trace can target one routine

## Notes

Depends on 0007 for non-allocating decode and 0009 for the ring.

Full tracing at real-time speed is about 7 MB/s, which is nothing; at uncapped
speed it would be 2.5 GB/s and would dominate. Opt-in is not a nicety.
