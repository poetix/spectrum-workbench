---
id: "0027"
title: Checkpoint ring
priority: medium
created: 2026-08-12
---

## Summary

Periodic in-memory whole-machine checkpoints on the emulation thread, into
buffers allocated once, so that any past T-state is reachable by restoring the
nearest checkpoint and replaying forward (ADR-0017).

## Acceptance criteria

- [ ] Capture and restore on `Machine`, into and out of a caller-owned buffer;
      no allocation on the emulation thread
- [ ] State captured is whole: registers including the shadow set, `WZ`, `Q`,
      `R`, `IFF1`/`IFF2`, interrupt mode, a pending `EI`, the clock, all of
      memory, and the device schedule `next_event` depends on
- [ ] Fixed-size ring of checkpoints, overwrite-oldest, capacity and cadence
      configurable; default one checkpoint per emulated second
- [ ] Each checkpoint records the run state and the index it had reached in the
      command log
- [ ] Restore re-applies arming commands (`Break`, `Unbreak`, `Watch`,
      `Unwatch`, `ClearAll`) from the start of the log, and machine-mutating
      commands only from the checkpoint's log index onwards
- [ ] `restore(t)` then replay to `u` produces the same machine as running
      straight through from the start to `u`
- [ ] Self-check mode: replay checkpoint *N* to checkpoint *N+1* and compare,
      reporting the first differing register or byte and the T-state
- [ ] Overhead measured on the throughput harness and under 1% at the default
      cadence
- [ ] Checkpointing happens at a control tick, never mid-instruction

## Notes

Depends on 0026 — a checkpoint plus an untrustworthy log restores to the wrong
machine, and does it silently.

Distinct from 0018. The SNA and Z80 formats are a user feature and are lossy by
design; SNA restores `PC` from the stack. A checkpoint that loses `WZ`, `Q` or a
pending `EI` does not restore the same machine. The two may share a
serialisation of the parts that overlap, but the file formats must not become
the checkpoint's definition of state.

Budget: a 48K checkpoint is about 64 KB, so 64 of them is 4 MB and a minute of
history. At 360× real time one emulated second is about 2.8 ms of wall clock, so
the default cadence is a 64 KB copy every 2.8 ms.

Paging (0012 onwards) makes a checkpoint bigger and makes "all of memory" mean
all banks plus the paging state. Sizing the ring in bytes rather than in
checkpoints is probably the right shape from the start.
