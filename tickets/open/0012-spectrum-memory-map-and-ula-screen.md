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
- [ ] Attribute area: ink, paper, bright, flash
- [ ] Flash attribute alternates on the correct 16-frame cadence
- [ ] Border colour from port 0xFE writes, rendered per scanline
- [ ] 50 Hz frame interrupt at the correct point in the frame
- [ ] Test: a known screen image renders to an expected framebuffer hash

## Notes

Per-scanline border rendering is needed even before contention, because
border effects are how a lot of Spectrum software signals timing.
