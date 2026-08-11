---
id: "ADR-0003"
title: Model WZ/MEMPTR and Q from the outset
date: 2026-08-11
status: accepted
---

## Context

The Z80 has two pieces of internal state that are not in the programmer's model
and are not mentioned in Zilog's documentation, but are observable:

**WZ (MEMPTR)** is a 16-bit scratch register written by almost every
instruction that touches memory or branches, each with its own rule — `LD
A,(BC)` sets it to `BC+1`; `LD (nn),A` puts `nn+1` in the low byte and `A` in
the high byte; `CPI` increments it. It leaks through exactly one door:
`BIT n,(HL)` has no register to source the undocumented X and Y flags from and
takes them from WZ's high byte instead.

**Q** is a latch holding whatever `F` the last instruction wrote, or zero if
the last instruction did not write flags. `SCF` and `CCF` consult it: if the
previous instruction wrote flags, their X/Y bits come from `A` alone; if not,
the bits are OR-ed with what is already in `F`.

Both were discovered by community research long after the chip shipped, and
both are needed to run real software and pass conformance suites.

## Decision

Model both from the first commit, before anything depends on them.

`wz` is set inline in each instruction as it is written, following the
published per-instruction table. `Q` is split into two fields — `q`, which the
ALU helpers set whenever they write `F`, and `q_prev`, which holds the previous
instruction's value and is what `SCF`/`CCF` read. The rollover happens once per
instruction in `step`, not in `execute`, because `execute` re-enters itself for
the `DD` and `FD` prefixes.

## Consequences

**Positive:**
- Writing `self.wz = ...` while writing each instruction costs seconds.
  Retrofitting means revisiting roughly forty instruction implementations, each
  needing a different rule looked up from a table.
- The failure mode avoided is severe: `zexall` reports a CRC mismatch over
  millions of instructions with no indication of which one, and the difference
  is two flag bits nobody printed.
- `Q`'s default is *clear*, so adding it later would mean auditing every
  instruction for "does this touch flags?" — including all the ones that do
  not, which is where the bugs would hide.

**Negative:**
- Neither is validated by the conformance suites we run. The Fuse suite
  predates both discoveries and expects the older behaviour; `zexall` was
  measured to pass with the `Q` rule deliberately broken, so its CRCs do not
  depend on it.
- So this behaviour rests on published research plus unit tests, and will not
  be externally confirmed until raxoft's `z80ccf` can run — which needs a
  Spectrum (ticket 0021).

**Note:** the split into `q` and `q_prev` was not the original design. The
first implementation cleared `Q` at the top of `execute`, which meant `SCF`
read its own already-cleared latch. The Fuse suite caught it.
