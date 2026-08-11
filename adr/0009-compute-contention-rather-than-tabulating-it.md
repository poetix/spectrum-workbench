---
id: "ADR-0009"
title: Compute contention rather than tabulating it
date: 2026-08-11
status: accepted
---

## Context

ULA contention delays depend on the T-state position within the frame. The
obvious implementation is a lookup table with one entry per T-state: for the
48K machine that is 69,888 entries, 68 KB as bytes.

That table would be streamed continuously alongside the 64 KB of emulated RAM.
On the development machine it is half of L1d; on a typical x86 desktop with
32-48 KB of L1d it is larger than the entire cache. It would evict the emulated
machine out from under itself.

The pattern is periodic: within the display portion of a scanline, delays
repeat every eight T-states as `6, 5, 4, 3, 2, 1, 0, 0`.

## Decision

Compute the delay arithmetically from an eight-byte pattern table:

```rust
const PATTERN: [u8; 8] = [6, 5, 4, 3, 2, 1, 0, 0];

fn contention(t: u64) -> u32 {
    let t = t % FRAME_TSTATES;
    if t < FIRST_CONTENDED || t >= FIRST_CONTENDED + 192 * LINE_TSTATES {
        return 0;
    }
    let into_line = (t - FIRST_CONTENDED) % LINE_TSTATES;
    if into_line >= 128 { return 0 }
    PATTERN[(into_line % 8) as usize] as u32
}
```

Applied only to addresses in 0x4000-0x7FFF.

## Consequences

**Positive:**
- Eight bytes, one cache line, permanently resident, instead of 68 KB streamed.
- A handful of arithmetic operations is cheaper than a cache miss.
- The relationship to the hardware is legible in the code rather than baked
  into an opaque generated table.

**Negative:**
- Two divisions and a modulo on a path taken for every contended access.
  `FRAME_TSTATES` and `LINE_TSTATES` are compile-time constants so these
  become multiplications, but it is not free.
- Harder to adapt if a machine variant has an irregular pattern. The 128K
  timings differ in their constants but not in shape, so this is not expected
  to bite.

**Caveat:** the constants above are from memory and must be checked against a
published reference before ticket 0020 lands. The shape of the solution is the
decision here; the numbers are not yet load-bearing.
