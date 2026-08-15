---
id: "ADR-0001"
title: The playfield is a window on a mirrored ring, blitted whole every frame
date: 2026-08-15
status: accepted
---

## Context

`games/scroller` scrolls the top two thirds of the screen — 24 characters wide
and 128 pixels tall — by one pixel a frame. The Spectrum has no scroll
register, so a picture that has moved by a pixel is a picture that has been
written again, and the only question is what it is written *from*.

Drawing the terrain straight onto the screen, feature by feature, is the
alternative. It scrolls by moving 3072 bytes of display file up one pixel row
and drawing the newly exposed line, which costs the same as writing the whole
playfield anyway — and it makes every sprite an erase problem, because what was
under one is no longer anywhere.

## Decision

Terrain lives in a ring buffer of 256 lines of 24 bytes, written one line per
frame at the edge that is coming into view. The screen is a window on it,
copied whole every frame, and sprites are plotted on top of the copy
afterwards.

Three things follow, and they are the reason for it:

- **The scroll costs the same whatever is on the screen.** A frame writes one
  line of terrain and copies 3072 bytes. Nothing is scrolled in place, and
  nothing is redrawn feature by feature.
- **Sprites are never erased.** The blit has already rewritten every pixel of
  the playfield by the time they are drawn.
- **The screen cannot drift from the model.** It is derived from the buffer
  every frame, so there is no incremental state to get wrong — which is what
  `the_playfield_is_the_window_the_ring_buffer_says_it_is` asserts.

**The first 128 lines of the ring exist twice.** A window that wrapped part way
down would need a second source pointer and a test per row. Writing each of the
first PF_H lines a second time, 6144 bytes further on, makes every window
contiguous for the price of 24 bytes a frame and 3 KB of RAM — and a contiguous
source is what lets `SP` read the whole 3072 bytes without ever being reloaded.

**The blit reads through `POP` and writes through `LD (HL),r`.** Reading with
the stack pointer is 5 T-states a byte and, on a contiguous window, costs
nothing to maintain. Writing with `PUSH` would be 5.5 — the usual stack blit —
but `SP` is already the source, so each row would have to swap it to the
destination and back twice, because only 12 bytes of registers survive a swap.
Costed out that is 324 T-states a row plus a per-frame pass to patch the
self-modified addresses, against 390 for the plain loop: three per cent, for
self-modifying code. It is not worth it *at this row length*. The trade turns
over for wider rows, which is the thing to remember if the playfield ever grows.

## Consequences

Measured, from the border stripes the game paints (`rkwshot --profile`):

| Phase | T-states | Share of a frame |
| --- | --- | --- |
| Blit | 54,880 | 78.5% |
| Sprites, bullets, attributes | 7,616 | 10.9% |
| Terrain line | 1,568 | 2.2% |
| Input and movement | 896 | 1.3% |
| Idle | 4,928 | 7.1% |

The blit's 49,920 T-states of instructions plus about 5,000 of ULA contention
on the screen writes is the whole of the 78.5%. That is the budget this game
has to live inside, and it says what the levers are, in the order they pay:
fewer bytes moved — a narrower playfield or a shorter window — beats any
rewriting of the loop, and scrolling two pixels every other frame halves the
cost outright at the same apparent speed.

Attributes are not part of the window. The blit copies pixels only, so the
cells a ship stamps have to be handed back explicitly when it moves, and that
is the one piece of erase bookkeeping in the game.

**Ships stand on the character grid and wear a black outline.** A ship moves
eight pixels at a time in both directions, so the two cells by two it covers
are exactly its own and stamping them takes nothing from anything else. Moving
anywhere else would mean straddling cells it would have to claim and could not
fill.

The mask is then the shape of the ship rather than the shape of its cells: the
artwork grown by `MASK_GROW` pixels in every direction and cleared to black, so
what the ship carries is an outline hugging it and not a box around it. That is
what keeps the terrain inside its cells — which is now drawn in the ship's ink —
far enough away to read as the ship's own edge.

It is a distance, not a proof. Terrain in the far corner of a covered cell is
still terrain in the ship's ink, and `MASK_GROW` is the dial between how much of
that is left and how fat the ship looks. A blackout of the whole cell would be
the proof, and would put the ship in a box; it was tried, and the box is worse
than the residue.

The mask is grown from the artwork at startup rather than drawn beside it, so
there is one copy of the shape and the outline cannot disagree with it —
`the_mask_is_the_artwork_grown_by_mask_grow_pixels` checks the Z80 that does it
against the same growth done in Rust.

Bullets are the counterexample that shows what the outline is for. They are
pixel-positioned, but they stamp no attribute, so they have no cell to keep
clean, need no outline, and are OR-plotted in whatever colour they are flying
through.
