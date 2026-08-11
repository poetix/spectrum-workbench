---
id: "0012"
title: Spectrum memory map and ULA screen
priority: high
created: 2026-08-11
---

## Summary

First hardware: the 48K memory map (16K ROM, 48K RAM), the screen memory
layout, and rendering it to a framebuffer. No contention yet (0020).

## Acceptance criteria

- [ ] `Bus` implementation with ROM write protection
- [ ] Screen memory decoded from its interleaved layout (third, character row,
      pixel row) to a linear framebuffer
- [ ] The decode is a function of a byte source and a base address, not of the
      live machine at $4000, so the debugger can render a back buffer through
      the same code (0025); flash phase is a parameter rather than read from a
      frame counter
- [ ] Attribute area: ink, paper, bright, flash
- [ ] Flash attribute alternates on the correct 16-frame cadence
- [ ] Border colour from port 0xFE writes, rendered per scanline
- [ ] 50 Hz frame interrupt at the correct point in the frame
- [ ] Test: a known screen image renders to an expected framebuffer hash

## Notes

Per-scanline border rendering is needed even before contention, because
border effects are how a lot of Spectrum software signals timing.
