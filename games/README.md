# games

Games written against this machine, in the assembler that lives next door.
They are here because they are the thing the workbench is for, and because a
game is the hardest test of an emulator anyone writes: it runs for hours,
touches the interrupt, the contended memory and the display file at once, and
notices timing nobody else does.

Each game keeps its own ADRs and tickets, numbered from 0001 in its own
directory, because a game's decisions are about a game and would otherwise be
interleaved with the emulator's. A bare `ADR-0001` inside `games/` is the
game's; the workbench's are named as such, and linked.

## `scroller`

A vertical scroller. The top two thirds of the screen scroll at one pixel a
frame; the bottom third is a panel. Terrain is written into a ring buffer, one
line a frame, blitted whole into the display file, and sprites are plotted over
it afterwards. [ADR-0001](scroller/adr/0001-the-playfield-is-a-window-on-a-mirrored-ring.md)
says why, and what it costs.

Ships stand on the character grid, so the two cells by two they cover are their
own to stamp with their colour, and they wear a black outline — the artwork
grown by a few pixels — which keeps the terrain inside those cells far enough
from the ship to read as its edge rather than as scenery in the wrong colour.
The outline is grown from the artwork at startup, so there is one copy of the
shape. Bullets stamp no attribute, so they need no outline and go anywhere a
pixel at a time.

Controls are `O` and `P` across, `Q` and `A` up and down, and `SPACE` to fire.

### Running it

Headless, which is what it is developed against — a screenshot every 25 frames
and where the time went:

```
cargo run -p rkw-shot -- games/scroller/main.asm \
    --frames 200 --out /tmp/shots --every 25 --profile
```

`--keys space,o` holds keys down for the run, so a headless capture can fly the
ship. The profile reads the border stripes the game paints around each phase of
its frame, which is the same measurement a Spectrum programmer makes on real
hardware and costs nothing here.

In the debugger, with source-level breakpoints:

```
cargo run -p rkwdbg -- games/scroller/main.asm
```

In the window, which assembles it and runs it as it stands:

```
cargo run --release -p rkw-gui -- --asm games/scroller/main.asm --scale 3
```

`--release` matters — the debug core is far slower than the machine it is
emulating. There is still no `.tap`, so playing it anywhere else waits on
[ticket 0001](scroller/tickets/open/0001-scroller-gameplay.md).

### The files

| File | What is in it |
| --- | --- |
| `main.asm` | The frame loop, the interrupt, input, the actors |
| `equates.asm` | The geometry, the memory map, the border colours |
| `terrain.asm` | The ring buffer, and the canyon that goes into it |
| `blit.asm` | The window to the screen, unrolled, once a frame |
| `sprite.asm` | The two plotters, and the attribute stamp |
| `data.asm` | The graphics |

The mechanics are pinned by `crates/rkw-shot/tests/scroller.rs`, which asserts
that the playfield is exactly the window the ring buffer says it is, that a
frame moves it by exactly one pixel row, that a ship hands its attribute cells
back when it moves, and that a frame's work fits in a frame.
