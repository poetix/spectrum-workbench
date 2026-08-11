---
id: "ADR-0004"
title: PC remains on the HALT instruction while halted
date: 2026-08-11
status: accepted
---

## Context

On real hardware a halted Z80 keeps fetching the `HALT` opcode from the same
address, executing NOPs internally to maintain DRAM refresh, until an interrupt
arrives. The return address an interrupt pushes is the instruction *after* the
`HALT`.

There are two ways to model this. Keep `PC` on the `HALT` and increment it when
leaving the halt state, or advance `PC` past the `HALT` immediately and set a
flag. Both produce identical externally visible behaviour, including the pushed
return address. Fuse takes the second approach; its test suite therefore
expects `PC` to point past the `HALT`.

## Decision

Keep `PC` on the `HALT` instruction, matching what the address bus does on real
hardware, and increment it when the halt is left.

The Fuse conformance harness normalises the difference by comparing `PC + 1`
when the halted flag is set, with the reasoning recorded at the comparison
site. Exactly one case in the suite is affected.

## Consequences

**Positive:**
- A debugger showing `PC` while stopped on a `HALT` points at the `HALT`,
  which is where the user believes execution is. The alternative shows the
  address after it, which reads as "you are past this instruction" and is
  misleading.
- Matches the address the CPU actually drives during the halt, so the trace
  ring and any bus-level view agree with hardware.

**Negative:**
- Diverges from the most widely used reference implementation, which means the
  test harness needs a documented adjustment rather than a plain comparison.
- Anyone porting logic from Fuse must be aware of the difference.

**Rejected alternative:** adopting Fuse's convention to avoid the adjustment.
It buys one fewer line in a test harness at the cost of a permanently worse
debugger display, which is the wrong trade for a project whose stated purpose
is debugging affordances.
