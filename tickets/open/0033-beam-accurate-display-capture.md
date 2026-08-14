---
id: "0033"
title: Beam-accurate display capture: record what the ULA fetched
priority: medium
created: 2026-08-14
---

## Summary

The machine's timing is beam-accurate and its picture is not. Contention stalls
the CPU at the right T-state and `float_bus` answers with the byte the ULA is
fetching *at that instant*, so a program can synchronise itself to the beam and
does. But `Spectrum::render` reads display memory at the moment it is called
(`spectrum.rs`, "the screen bytes are read live"), so the frame a frontend gets
is one instantaneous snapshot of `$4000`. Everything a program achieves by
rewriting the display file ahead of the beam — multicolour, 8x1 attribute
effects, mid-line attribute changes — collapses to whatever was left in memory
at the end.

Record what the ULA fetched, as it fetches it, and render from that.

## What the hardware does

For the 128 T-states of each display line the ULA fetches in groups of eight:
bitmap, attribute, bitmap, attribute — two character cells per group, sixteen
groups across the line, four T-states per cell. `contention::PATTERN` is the
same eight T-states seen from the CPU's side. So the attribute byte for a cell
is latched at *its own* T-state, and a write that lands after that T-state
changes the cells to the right of the beam and not the ones to its left.

Cell granularity is the floor for the display area, not an approximation: the
ULA cannot change ink or paper part way through a cell. Nothing finer exists to
model here. The border is finer and is 0034.

## Acceptance criteria

**The capture**

- [ ] A fixed-size frame capture holding what the ULA fetched: 6144 bytes of
      bitmap and one attribute byte per cell *per scanline* — 32 x 192 — for
      12 KB in total
- [ ] Filled by catch-up rather than by event: a write into `$4000..$5B00`, the
      end of a line and the end of a frame fill the capture forward to the
      current T-state. No per-group scheduled event
- [ ] Catch-up is O(groups elapsed) and allocates nothing, so a program writing
      the display file in a tight loop pays for the groups that actually
      elapsed and not for the writes
- [ ] The fetch schedule is *derived from the same function* that answers
      `float_bus`, not written a second time. A program that reads a byte off
      the floating bus and a frame that shows that byte cannot disagree
- [ ] The capture is plain fixed-size machine state, `Clone`, no host state, so
      a checkpoint (0027) round-trips it and a replay (0029) can compare it

**The decode**

- [ ] `decode` keeps its present signature for a display file — a `Peek`, a base
      address and a flash phase — so `.scr` files, back buffers and the 128K
      shadow screen are unaffected (ADR-0020)
- [ ] A second entry point paints from a frame capture, where the attribute
      plane is per cell per line rather than per cell per row
- [ ] Both paint through the same code; the difference is where the attribute
      byte for a cell comes from
- [ ] Flash still a parameter, still not read from a frame counter

**Tests**

- [ ] A frame in which nothing is written mid-frame renders identically through
      both paths, so the existing pinned hashes and the boot test are unmoved
- [ ] A routine assembled by `rkw-asm` that rewrites an attribute after the beam
      has passed cell 10 of a line: cells 0..10 of that line show the old
      attribute, cells 20..31 the new one, and the whole of the next line the
      new one
- [ ] The same routine synchronised by the floating bus rather than by counted
      T-states produces the same picture, which is the check that the capture
      and `float_bus` share a clock
- [ ] An 8x1 multicolour bar — one attribute per scanline down a column —
      renders as 192 distinct rows rather than as 24
- [ ] `tests/no_alloc.rs` covers the catch-up path
- [ ] `tests/throughput.rs` measures a program that writes the display file
      heavily, against the same program with the capture disabled, in one
      binary (docs/architecture.md, "a caution about quoted throughput figures")

**The record**

- [ ] An ADR amending ADR-0020, because the decode's input model changes: the
      picture stops being a display file and becomes what the ULA fetched, and
      the display file becomes the degenerate case where every line of a row
      shares an attribute

## Notes

ADR-0020 is not weakened by this and should not be rewritten as though it were.
Its claim is that the decode is a pure function of bytes rather than a method on
a running machine, and that survives — the capture is just a different byte
source. What changes is the shape of the bytes.

The 12 KB is per machine, so it is 12 KB in every checkpoint 0027 takes. That is
a real cost against a 64 KB address space and worth stating in that ticket
rather than discovering there. It cannot be regenerated on restore: the whole
point is that it holds history the current memory no longer contains.

Fuse solves the same problem with `display_dirty_ytable`/`xtable` mapping an
address to screen coordinates and a `critical_region_x/y` pair marking how far
the beam has got, drawing incrementally as it goes. The catch-up above is that
design with the drawing deferred: record the bytes, paint later, so the paint
stays a pure function and the emulation thread does no rendering. That matters
here and did not there, because a picture has to be renderable at a breakpoint
stop as well as at a frame boundary.

The offset between a fetch and the pixel it lights is a calibration constant and
belongs with the border's (0034), since they are the same measurement from two
directions. `FIRST_CONTENDED_T = FIRST_DISPLAY_T - 1` is the one already in the
tree and the capture must agree with it.
