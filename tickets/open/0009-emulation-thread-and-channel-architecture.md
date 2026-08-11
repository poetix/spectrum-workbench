---
id: "0009"
title: Emulation thread and channel architecture
priority: high
created: 2026-08-11
---

## Summary

Move emulation onto its own thread driven by a T-state slice loop, with the
three channels described in ADR-0007: a lossy outbound event ring, a lossless
stop notification, and a lossless inbound command ring drained at control rate.

## Acceptance criteria

- [ ] `run_slice(deadline)` loop, deadline being the earliest scheduled event
- [ ] Outbound SPSC event ring, 16-byte records, overwrite-oldest with a drop
      counter surfaced to the consumer
- [ ] Stop notification as a state transition, not a ring message
- [ ] Inbound SPSC command ring drained once per scanline
- [ ] Producer and consumer indices padded apart using a per-target constant
      (ADR-0010), not a hardcoded 128
- [ ] No allocation on the emulation thread; enforced by test or by inspection
- [ ] Commands stamped with the T-state at which they were applied, so a
      command log replays deterministically

## Notes

The deterministic replay property is easy to lose by applying a command at
whatever moment is convenient. It is worth a test that replays a recorded
command log and asserts an identical final state.
