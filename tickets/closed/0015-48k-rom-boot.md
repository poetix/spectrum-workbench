---
id: "0015"
title: 48K ROM boot
priority: high
created: 2026-08-11
closed: 2026-08-13
---

## Summary

Boot the real 48K ROM to the copyright message and a usable BASIC prompt. The
integration milestone for 0012 and 0013.

## Acceptance criteria

- [x] ROM image loads and executes from reset
- [x] Copyright screen appears and matches a reference image
- [x] BASIC prompt accepts keyboard input
- [x] A short BASIC program can be typed and run
- [x] Regression test: boot for N frames headless and hash the framebuffer

## Notes

The ROM is not redistributable, so it is fetched or supplied by the user in
the same way as the conformance data (ADR-0005).

This is the point at which raxoft's `z80test` suite becomes runnable, which is
the only thing that will validate the `Q` latch behaviour (ADR-0003).

## As built

`scripts/fetch-rom.sh`, `crates/rkw-spectrum/tests/boot.rs` with its harness in
`tests/common/`, and `rkwdbg --rom`.

It boots. 150 frames of emulated time — three seconds, most of it the ROM's own
RAM check — gets to `© 1982 Sinclair Research Ltd`, `PRINT` comes out of one
keypress at the `K` cursor, and `10 PRINT "hello"` / `RUN` prints `hello` and
reports `0 OK, 10:1`.

### The reference image is the ROM's own font

The acceptance criterion said "matches a reference image" and that turned out to
be the wrong artefact twice over. A picture of the ROM's output is the ROM's
content in a repository that deliberately does not carry the ROM, and a pixel
comparison reports forty thousand differences when what happened was one wrong
character.

So the screen is read back *as text*: characters 32 to 127 are eight bytes each
at `$3D00`, the display is cell-aligned, and matching each cell against that
table — inverted as well, which is how the cursor and the bottom-line prompts
are drawn — turns the screen into twenty-four lines of text. The tests then
assert on `10>PRINT "hello"`, and a failure prints the screen.

The framebuffer hash is what covers the rest: the pixels within a cell, the
attributes, the white border, and the geometry that puts them where they are.
Pinned as a constant rather than stored as an image, for the same licensing
reason. It is only meaningful for one ROM image, which is why the fetch script
verifies the SHA-256 rather than trusting whatever comes back — a 128K ROM pair
or a Spanish 48K image would boot to something that looked almost right.

### A keystroke is a duration

Nothing in the machine buffers a keypress: the ROM samples the matrix in its
interrupt routine and debounces in software, so a key that appears and vanishes
between two frames was never pressed. `Board::press` holds for three frames and
releases for eight.

The eight is `KSTATE` at `$5C00`, and it is not padding. The ROM keeps two
four-byte sets, each holding the key that claimed it, a countdown, the repeat
delay and the character it decoded to. A set is claimed at the press and freed
only when the countdown, which starts at 5 and is decremented once per
interrupt, reaches zero — so the set outlives the release by five frames, and a
press of the *same* key inside that window is classified as a repeat of the set
rather than a new keypress. Repeats emit nothing until `REPDEL`, which is 35.

Watched live at three frames of gap: the set reads `[50 05 21 F5]` at the
release and holds `50` for five more frames. So the second `L` of `HELLO`
lands on it and is swallowed, and so does the `"` after `PRINT` — SYMBOL SHIFT
and `P` scans as the `P` key, which is the key that set is still holding.

Nothing in `rkw-spectrum` was wrong: the matrix reported exactly what was held,
and every test against it passed before and after. The defect was in the harness
above it, which typed faster than any hand can. What running the real ROM found
is not an emulator bug but a *requirement on the emulator's callers* — 0019's
frontend gets this for free from a human's fingers, and 0026's recorded input
will have to reproduce these durations rather than the keystrokes alone.

### `rkwdbg --rom`

The debugger was already generic over `Machine`, so `--rom` is a machine
constructor and one trait. `load::Image` is "somewhere a loader can put bytes",
implemented for `FlatMemory` and for `Spectrum` — deliberately not `Bus::write`,
because a bus write below `$4000` does nothing, which is right for the emulated
program and useless for a loader. `run_on` is the old `run` with the machine
made a parameter.

A binary loaded beside a ROM keeps its own entry point, so `--rom 48.rom --load
$8000=test.bin` runs the test with the ROM present, which is what a suite that
calls ROM routines needs.

### What is not here

`z80test` has not been run. It is runnable now — that is what this ticket
unblocks — but reconciling what it says about the `Q` latch and the remaining
undocumented behaviour is 0021, and it wants a tape or a snapshot loader (0016,
0018) to be convenient rather than a `--load` of a raw image.

The copyright line appears at frame 84 and the tests run 150, which is margin
rather than measurement: contention (0020) will slow the RAM check down by
however much of it touches the bottom 16K of RAM, and a bound with room in it is
one fewer thing to adjust then.
