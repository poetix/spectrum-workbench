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
| Sprites, bullets, attributes | 5,824 | 8.3% |
| Terrain line | 1,568 | 2.2% |
| Input and movement | 896 | 1.3% |
| Idle | 6,720 | 9.6% |

The blit's 49,920 T-states of instructions plus about 5,000 of ULA contention
on the screen writes is the whole of the 78.5%. That is the budget this game
has to live inside, and it says what the levers are, in the order they pay:
fewer bytes moved — a narrower playfield or a shorter window — beats any
rewriting of the loop, and scrolling two pixels every other frame halves the
cost outright at the same apparent speed.

Attributes are not part of the window. The blit copies pixels only, so the
cells a ship stamps have to be handed back explicitly when it moves, and that
is the one piece of erase bookkeeping in the game.

**Ships stand on the character grid and fill it.** A ship moves eight pixels at
a time in both directions, so the two cells by two it covers are exactly its
own and stamping them takes nothing from anything else. Its artwork is then
*written* rather than masked in — sixteen rows of two bytes, replacing what was
there — so every pixel of every stamped cell is the ship's, artwork where the
artwork is and black where it is not. A clash is not mitigated but impossible.

That puts the whole of the problem in the artwork, which is where it belongs
and where it costs nothing. A shape that fills its square leaves nothing to
notice; a round one gives up four corners to black, which at this size is the
edge of the character and reads as nothing; a small shape in a big square would
read as a black box. `the_artwork_fills_the_square_it_is_drawn_in` holds new
artwork to it, because the rule is invisible until it is broken.

Two other schemes were built and measured before this one, and both were worse
in the same way — they let the sprite be somewhere the cell grid was not:

- *Pixel-positioned, with the covered cells blacked out.* A 16x16 ship at a
  pixel offset straddles three cells each way, so the blackout is 24x24 and the
  ship flies in a visible box. Clash-free, and it looks it. 9,632 T-states.
- *Character-positioned, with a mask grown from the shape.* An outline a few
  pixels wide, so terrain survives further out in the cell — an AND and an OR
  per byte, a table twice the size, a startup pass to grow the mask, and it only
  ever pushes the clash further away rather than removing it. 7,616 T-states.

Frame-filling artwork on the grid removes the clash outright and is the
cheapest of the three at 5,824. The lesson is the one Lightforce already knew:
the sprite that does not clash is the one drawn to the shape of the cells it
stands on.

Bullets are the counterexample that shows what all of this is for. They are
pixel-positioned, but they stamp no attribute, so they have no cell to keep
clean, nothing to clear, and are OR-plotted in whatever colour they are flying
through.
