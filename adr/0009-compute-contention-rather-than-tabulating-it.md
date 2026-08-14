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

**Checked, in ticket 0020.** The constants were verified against Fuse — the
project this repository's conformance data and ROM already come from (ADR-0005)
— at `libspectrum/timings.c`, `fuse/spectrum.c` and `fuse/machines/spec48.c`.
Everything in `crates/rkw-spectrum/src/frame.rs` agrees with the 48K row of
Fuse's table. The pattern is right. `FIRST_CONTENDED` is 14,335 and not 14,336:
contention starts one T-state *before* the ULA's first fetch, because an access
beginning on that T-state would still be holding the bus when the ULA wants it.
The sketch above is otherwise correct as written.

**The performance claim did not survive measurement.**
`crates/rkw-spectrum/tests/throughput.rs` builds the 68 KB table this ADR
refused and runs the machine on it. On the development machine the table is
about 2.5x faster as a lookup and about 1.3x faster in the emulator, both of
which are the opposite of what is argued above.

That is not enough to overturn the decision, and it is not enough to keep it
either. The argument rests on two conditions the measurement does not meet: an
L1d of 32-48 KB, where the table does not fit, against the 128 KB of the machine
it was measured on; and an emulated program with a working set of kilobytes,
against a benchmark loop that touches 256 bytes. Both would move the result
towards the table losing, and neither can be tested here. Ticket 0032 is to
settle it on an x86 part with a realistic workload. Until then the arithmetic
stays, on the understanding that its justification is now an open question
rather than a finding.
