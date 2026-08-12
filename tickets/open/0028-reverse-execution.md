---
id: "0028"
title: Reverse execution: step back and reverse continue
priority: medium
created: 2026-08-12
---

## Summary

Run the machine backwards from a crash. Restore the nearest checkpoint at or
before the target, replay to it, and present the result as though the machine had
simply moved (ADR-0017).

## Acceptance criteria

- [ ] `run_to_t(t)`: the machine as it was at any T-state still covered by the
      checkpoint ring
- [ ] `step_back`: back one instruction, defined as the instruction boundary
      immediately before the current one — found by replaying from the
      checkpoint and keeping the previous boundary, not guessed from a decode
- [ ] `step_back_over`: back to the boundary before the current one, skipping a
      call that returned to it
- [ ] `reverse_continue`: to the most recent breakpoint or watchpoint hit
      strictly before the current T-state, evaluating conditions and ignore
      counts exactly as the forward run does
- [ ] History exhausted is a distinct stop reason, never reported as "nothing
      was hit"; it names the oldest T-state still held
- [ ] Repeated `step_back` is not quadratic: walking back within one checkpoint
      interval does not replay from the checkpoint every time
- [ ] REPL commands over the same executor the forward ones use (0010)
- [ ] Stepping back *N* and then forward *N* returns to the starting state, for
      a session with tape, keyboard and interrupts in it
- [ ] Reverse execution leaves the recorded log alone: it moves a view of
      history, and does not append to it

## Notes

Depends on 0026 and 0027, and on 0010 for the command layer.

Worst-case cost is one checkpoint interval of replay — a few milliseconds at the
default cadence — so the cost of stepping back is set by the cadence and not by
the distance travelled. Stepping back a hundred instructions costs about what
stepping back one costs, which is worth saying in the documentation because it is
the opposite of the intuition an undo-journal debugger gives.

The interaction to get right is what happens when the user steps back and then
does something. Resuming forward from a past T-state abandons the recorded
future; that is the useful behaviour, and it needs to be an explicit branch in
the log rather than an append that leaves the log describing two different runs.
