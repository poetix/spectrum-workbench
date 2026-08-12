---
id: "0029"
title: Replay divergence detection
priority: low
created: 2026-08-12
---

## Summary

Catch a machine that has stopped being a function of its recorded inputs, at the
moment it stops, rather than as a reverse-execution answer that is wrong for no
visible reason (ADR-0017).

## Acceptance criteria

- [ ] A mode that replays each checkpoint interval as it is superseded and
      compares the result against the checkpoint that follows it
- [ ] A divergence names the first differing register or byte, the T-state, and
      the commands applied in that interval
- [ ] Off by default and cheap to leave off; on, it costs roughly one extra
      emulation pass
- [ ] A CI test replaying a session that exercises tape loading, keyboard input,
      the frame interrupt and contention
- [ ] The self-check runs against the machines of 0012, 0016 and 0020 as they
      land, not only against `FlatMemory`

## Notes

Depends on 0027, which is what makes the check nearly free: the two checkpoints
being compared are already held.

This exists because a divergence is the worst-shaped bug available here. The
symptom is a machine that is different; the cause is an input that took a route
the log does not see, arbitrarily far back. Without this, the report is "step
back gave me the wrong answer", which is unactionable.

The devices that will break it are the ones with a foot in host state: audio
(0014), the windowed front end (0019), and anything that reads a wall clock. The
check is what keeps that boundary honest as they are written.
