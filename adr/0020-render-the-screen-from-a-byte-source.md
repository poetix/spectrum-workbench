---
id: "ADR-0020"
title: Render the screen from a byte source, not from the machine
date: 2026-08-13
status: accepted
---

## Context

Ticket 0012 adds the display: the interleaved layout of the display file, the
attribute area, flash, and a framebuffer to draw it into. The obvious signature
is a method on the machine — `spectrum.render()` — reading the display file at
`0x4000` and taking the flash phase from the ULA's frame counter.

Almost every other thing that wants a picture is then a special case of it:

- The debugger's screen pane (ticket 0025) renders a *back buffer*, because
  watching a game draw into a buffer at `0xC000` is most of the point of
  watching at all.
- The 128K machine has a shadow screen at a second address.
- A `.scr` file, a `.sna` snapshot (0018) and a tape block are display files
  with no machine attached.
- A test wants a stable picture, and a flash phase read from a frame counter
  makes the same breakpoint render two ways depending on when it was reached.

## Decision

The decode takes a byte source, a base address and a flash phase, and knows
nothing else:

```rust
pub fn decode_into<S: Peek>(src: &S, base: u16, flash: Flash, out: &mut [u8], stride: usize)
```

`Peek` is the debugger's existing "read a byte, and the machine cannot tell"
trait, which slices, arrays, closures and the machine's memory all satisfy.
`Flash` is an argument rather than a lookup, so a caller that wants a stable
picture says which half of the cycle it wants and gets it every time.

`Spectrum::render` remains, as the composition that passes `0x4000` and the
ULA's phase. It is a convenience over the general form and not the other way
round.

Rendering is also not done at the end of a frame. The ULA records the border
per scanline as it goes, because that is information which is lost if it is not
recorded when it happens, and the picture is composed when somebody asks for
one.

## Consequences

**Positive:**
- The debugger's screen pane, the frontend, a snapshot viewer and a `.scr`
  loader are one code path with different arguments.
- A rendered screen is a pure function of bytes, so a test asserts a hash
  without running a machine to a particular frame.
- Nothing is painted when nobody is looking: a headless run at 350× real time
  does not compose 104 KB of framebuffer fifty times a second of emulated time.

**Negative:**
- Two calls rather than one where a caller wants border and display, and a
  reader has to know that the border comes from the ULA's log while the pixels
  come from memory.
- The generic decode is monomorphised per byte source. It is not on the
  emulation thread, so this costs code size and nothing else.

**Caveat:** the screen bytes are read at the moment of rendering rather than
latched during the frame, so a picture composed part way through a frame can
catch a routine mid-draw. That is what a debugger wants and what a frontend
never sees, because a frontend renders at the frame boundary.
