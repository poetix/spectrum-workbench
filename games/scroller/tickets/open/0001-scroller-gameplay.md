---
id: "0001"
title: The scroller, from mechanics to a game
priority: medium
created: 2026-08-15
---

## Summary

`games/scroller` has the mechanics and nothing else: a canyon rolls past at one
pixel a frame, a ship flies in it, an enemy drifts about, and bullets go up.
Nothing collides, nothing scores, nothing ends. This is what turns it into a
game.

The mechanics it already has, and what they cost, are ADR-0001.

## Acceptance criteria

- [ ] Collision: ship against terrain, bullets against enemies, enemies against
      the ship. Terrain collision reads the ring buffer, not the screen — the
      buffer is the model (ADR-0001)
- [ ] Several enemies from a table, with movement patterns, rather than one
      hard-coded drifter
- [ ] The panel: score, lives, and whatever else the bottom third is for
- [ ] Waves — what arrives, when, and what makes the canyon narrow
- [ ] A `.tap` that loads and runs on another emulator or real hardware
- [ ] Sound, once the workbench's ADR-0021 beeper has something to say

## Notes

**The frame budget is the design constraint.** The blit is 78.5% of a frame and
the rest of the game currently fits in 12%, with 10% spare. Two levers are worth
more than any tuning of the blit, and both change how the game looks rather
than how it is written: a narrower playfield moves fewer bytes, and a two-pixel
scroll every other frame halves the cost at the same apparent speed. Decide
which before the game logic grows into the spare 10%.

**Collision wants the buffer, not the screen.** A ship on cell *cx*, *cy* is
over the ring buffer bytes at `src + cy*8*PF_W + cx`, sixteen rows of two —
the same arithmetic the blit does, no shifting, and no reading back of the
display file. The artwork is the mask to test against.

**A `.tap` is for loading it somewhere else.** `rkw-gui --asm` assembles and
runs a source file directly, so playing it here needs no tape. A tape is what
another emulator, or real hardware, would want: either the assembler grows
`SAVETAP` (the workbench's ticket 0031), or something small builds a BASIC
loader block and a code block with `rkw-tape`.

**New sprites have to be drawn frame-filling.** A ship's artwork is written over
the cells it stands on, so anything it does not fill goes black, and a small
shape in a 16x16 square reads as a box carried about. The test holds new artwork
to better than 60% coverage and to reaching both edges, which is the rule stated
rather than remembered.
