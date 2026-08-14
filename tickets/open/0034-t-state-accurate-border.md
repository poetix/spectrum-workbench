---
id: "0034"
title: T-state-accurate border
priority: medium
created: 2026-08-14
---

## Summary

The border is recorded as one colour per scanline (`Ula::presented`, 312 bytes),
which was the right size when 0012 built it and is now the coarser half of a
beam-accurate machine. The border is not fetched from anywhere: it is whatever
the last write to port `$FE` left when the beam passes, and the beam covers two
pixels per T-state. So a program that writes the border twice within a line gets
two colours on that line on real hardware and one here.

That is the effect the ticket exists for. Sub-scanline border work is how a
great deal of software makes its timing visible, and it is the cheapest thing on
the machine to synchronise against.

## Acceptance criteria

**The log**

- [ ] The border log becomes `(T-state, colour)` rather than one byte per line:
      a fixed-capacity array of packed `u32`s — 17 bits of frame T-state, three
      of colour — appended to on any write to `$FE` that changes the border bits
- [ ] Capacity sized for a real effect rather than for the worst case: a border
      change every scanline is 312 entries, four a line is 1248. Cap it, count
      what was dropped, and surface the count rather than reallocating or
      silently losing the tail
- [ ] Double-buffered as now, for the reason `ula.rs` already gives: a frame
      rendered part way through otherwise shows half of one border and half of
      the previous one
- [ ] Nothing allocates, and the log is plain fixed-size state that a checkpoint
      clones (0027)

**The render**

- [ ] The colour of a border pixel is the last entry at or before the T-state
      that pixel is painted, plus the ULA's latency between the `OUT` completing
      and the change reaching the beam
- [ ] That latency is one named, documented constant with its derivation beside
      it, not a number spread through the renderer
- [ ] Two pixels per T-state across the line, so the 48-pixel side borders are
      24 T-states each and the geometry in `frame.rs` is the same geometry
- [ ] Border and display capture (0033) are painted from one clock; a stripe
      that starts mid-line lines up with the cell the display shows at that
      T-state

**Tests**

- [ ] A routine that writes `$FE` four times within one scanline renders four
      colours on that line, at the pixel columns the T-states predict
- [ ] A rainbow border — one colour per line — renders exactly as it does today,
      so the existing pinned hashes are unmoved
- [ ] A write in the horizontal retrace affects the next line and not the
      current one
- [ ] The drop counter fires for a program that writes `$FE` every eleven
      T-states, and the frame it produces is still coherent

## Notes

0012 recorded the reason this was deferred and it has now been met:

> Sub-scanline effects are not modelled. They need the position within the line,
> which is contention's arithmetic (0020), and they are worth having once that
> exists.

0020 has landed. `contention::delay` already carries the line and the position
within it, and the border wants the same decomposition with a different modulus.

The latency constant is the one number here that cannot be derived from what is
already in the tree, and it is the one to be careful about: it is what decides
whether a stripe lands where the author put it or eight pixels to one side.
Fuse's equivalent is buried in its `display_dirty`/critical-region path rather
than named, so it is a check and not a source. Prefer a documented hardware
reference and pin the choice with the assembled test above, which will fail
loudly if the constant moves.

Independent of 0033 in principle — a border log needs no display capture — but
worth doing after it, because "one clock paints the whole picture" is easier to
assert than to retrofit.
