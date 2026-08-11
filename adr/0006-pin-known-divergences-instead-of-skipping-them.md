---
id: "ADR-0006"
title: Pin known divergences instead of skipping them
date: 2026-08-11
status: accepted
---

## Context

Four cases in the Fuse suite disagree with this core, all `BIT n,(HL)`. The
suite predates the MEMPTR research and takes the undocumented flags from the
byte read out of memory; the researched behaviour takes them from `WZ`, which
`BIT n,(HL)` does not itself set. The correct result therefore depends on state
the input files do not specify, and the cases cannot be satisfied by a
MEMPTR-accurate core.

The obvious handling is to skip them. But a skipped test is silent in both
directions: it says nothing if the divergence is fixed, and nothing if a fifth
case joins it.

## Decision

Name the divergent cases in a `KNOWN_DIVERGENCES` list, exclude them from the
failure report, and then assert that the set of cases which actually diverged
is *exactly* that list.

A divergence that starts passing fails the test. A new divergence fails the
test. The list carries a comment explaining why each entry is there and what
would settle the question.

## Consequences

**Positive:**
- The four exceptions are visible, explained, and cannot rot silently.
- Changing CPU behaviour that accidentally fixes or breaks one of them is
  surfaced immediately, with a message saying the divergence set changed.

**Negative:**
- Slightly more machinery than a skip list.
- The assertion compares an ordered vector against an ordered slice, so the
  list must stay in case order. A set comparison would be more robust if this
  grows.

**Related:** the same reasoning applies to the elided-read handling in the same
harness. Fuse's own core skips reads whose results it does not need, so its log
holds a contention marker with no matching read. Rather than allowlisting the
nineteen affected cases, the comparison accepts an extra read of ours when the
expected log has a contention marker at the same address exactly three
T-states earlier — structural, so it cannot mask an unrelated mismatch.
