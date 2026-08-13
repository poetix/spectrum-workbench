---
id: "0012"
title: Spectrum memory map and ULA screen
priority: high
created: 2026-08-11
closed: 2026-08-13
---

## Summary

First hardware: the 48K memory map (16K ROM, 48K RAM), the screen memory
layout, and rendering it to a framebuffer. No contention yet (0020).

## Acceptance criteria

- [x] `Bus` implementation with ROM write protection
- [x] Screen memory decoded from its interleaved layout (third, character row,
      pixel row) to a linear framebuffer
- [x] The decode is a function of a byte source and a base address, not of the
      live machine at $4000, so the debugger can render a back buffer through
      the same code (0025); flash phase is a parameter rather than read from a
      frame counter
- [x] Attribute area: ink, paper, bright, flash
- [x] Flash attribute alternates on the correct 16-frame cadence
- [x] Border colour from port 0xFE writes, rendered per scanline
- [x] 50 Hz frame interrupt at the correct point in the frame
- [x] Test: a known screen image renders to an expected framebuffer hash

## Notes

Per-scanline border rendering is needed even before contention, because
border effects are how a lot of Spectrum software signals timing.

## As built

`rkw-spectrum`, in four parts: `frame` (the geometry), `memory` (the map),
`screen` (the decode and the framebuffer) and `ula` (the frame clock, the
interrupt, the flash cadence and the border), with `spectrum` wiring them to
`Bus` and to the emulation thread's `Machine`.

The dependency runs from this crate to `rkw-debug` rather than the other way,
because `Machine` is what the slice loop asks about its schedule and the ULA is
what has a schedule to answer with. `rkw-debug::machine` was written expecting
exactly this — "the ULA implements two methods rather than a loop" — and it
turned out to be those two methods and nothing else.

### The picture is composed on demand, from bytes

[ADR-0020](../../adr/0020-render-the-screen-from-a-byte-source.md). The decode
takes a `Peek`, a base address and a flash phase, so a `.scr` file, a back
buffer at `0xC000` and the live display are the same call with different
arguments, and `tests/screen.rs` asserts that the three agree. `Spectrum::render`
is the composition that passes `0x4000` and the ULA's phase, not the primitive.

Nothing is rendered at the frame boundary. A headless run has nobody to render
for, and a debugger stopped at a breakpoint wants a picture at its moment
rather than at the last frame's.

### The border is a log, and the log is a fill

The border is stored as one colour per line of the frame, and a write to port
`0xFE` fills every line since the last write with the colour that was in force
before taking effect from the current line. That is a few bytes of `fill` per
`OUT`, against 312 bytes per frame for the array — where a per-T-state log
would be 69,888 entries and a single colour would lose the effect entirely.

Sub-scanline effects are not modelled. They need the position within the line,
which is contention's arithmetic (0020), and they are worth having once that
exists.

The last complete frame is what gets rendered, kept apart from the frame in
progress: rendering part way through a frame otherwise shows half of one border
and half of the previous one, which is a torn picture nobody asked for.

### The interrupt is derived from the clock

`INT` is asserted for 32 T-states from the top of each frame, and
`interrupt_pending` computes that from the clock instead of a flag being raised
and cleared. So a machine single-stepped across a frame boundary sees the same
interrupt a free-running one does, and the CPU accepting it does not clear the
line — what stops a second one being taken is `IFF1` going down, as on the
hardware.

The frame clock advances by exactly 69,888 T-states rather than from wherever
the clock got to. A slice ends on the last instruction to *start* before its
deadline, so "now" at the end of a frame is up to twenty-odd T-states late, and
a clock that took its start from that would drift by however much of the frame
the emulated program spent in long instructions.

One consequence is visible in the tests and worth stating: a program that
enables interrupts within 32 T-states of the machine being made takes the
interrupt at the top of frame zero, so eleven interrupts arrive in ten frames.
That is the hardware's behaviour, not an off-by-one.

### Writing and poking are different operations

`Bus::write` is the machine writing and ignores everything below `0x4000`.
`Memory::poke` is a loader or a debugger writing and ignores the map, because
loading a ROM image, restoring a snapshot and patching a ROM routine are things
done *to* the machine rather than *by* it.

The debugger's `poke` command goes through `Machine::write`, so it is currently
refused in ROM. That is the right answer for the emulated program and the wrong
one for a person at a prompt; it wants an unprotected path through the command
layer, and the place to do that is 0015, where the machine is first wired to
the CLI.

### The constants are provisional and say so

`frame.rs` carries the frame geometry — 224 T-states a line, 312 lines, the
display starting at 14,336 — and a note that ticket 0020 has to check them
against a published reference before contention is built on them. A frame
interrupt at the wrong T-state makes software feel wrong; contention at the
wrong T-state makes it render wrong.

The visible picture is 352x296: the whole non-blanking area, 48 border pixels
each side and 48 lines above and 56 below the display. A frontend that wants
less crops.

### What is not here

Contention and the floating bus (0020), the keyboard matrix (0013) and the
speaker (0014) — though the last two read state this already keeps, since port
`0xFE` is one byte and the border is three bits of it. `input` from an
unattached port returns `0xFF` rather than what the ULA was fetching.

Nothing is wired into `rkwdbg` yet: this crate is a machine the emulation
thread can run, and 0015 is the ticket that boots a ROM in it.
