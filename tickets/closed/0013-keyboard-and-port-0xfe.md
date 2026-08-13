---
id: "0013"
title: Keyboard and port 0xFE
priority: medium
created: 2026-08-11
closed: 2026-08-13
---

## Summary

Keyboard matrix emulation and the rest of port 0xFE.

## Acceptance criteria

- [x] 8 half-rows by 5 bits, correctly decoded from the address lines
- [x] Multiple simultaneous key presses, including the Caps Shift and Symbol
      Shift combinations the ROM expects
- [x] Host key mapping is a table, not hardcoded, so layouts can be swapped
- [x] EAR bit reads (used by tape loading, 0016)
- [x] Test: driving the matrix produces the expected `IN` results for each
      half-row

## Notes

The ROM reads the keyboard by driving one address line low at a time; getting
the decode wrong produces a keyboard that works for single keys and fails for
shifted ones.

## As built

Two modules in `rkw-spectrum`: `keyboard` is the matrix, `keymap` is the bridge
from a host keyboard to it. The ULA owns a `Keyboard` and composes the read of
port `0xFE` from it.

### The decode is the address, not a row number

`Keyboard::read` takes the whole 16-bit port and ANDs together every half-row
whose address line is low, rather than indexing an array with a row number.
That is what the hardware does, and the ROM depends on it in both directions:
`LD BC,$00FE` drives all eight lines low to ask "is any key at all down", which
is how `KEY-SCAN` opens and how the pause loop waits, and `IN A,($FE)` puts the
accumulator on the high byte, which is how a lot of games read a single
half-row. So the whole address goes through `Bus::input` to the ULA and not
just the `0xFE` in its low byte; `tests/keyboard.rs` runs both idioms on a real
CPU.

A key's position is its discriminant. The `Key` enum is declared in matrix
order, five to a half-row, so `half_row` is the discriminant divided by five
and `bit` is the remainder — no table to get out of step with the enum. The
published matrix is written out once more in the unit tests, which is the one
thing that cannot be derived from anything else.

Set means pressed in the stored matrix and low means pressed on the bus. The
inversion happens once, in `read`, because everything above the ULA wants to
talk about keys being down.

### The host map is a table, and the matrix is rebuilt from it

`KeyMap` is `&'static [(HostKey, &'static [Key])]`, and `KeyMap::PC` is the
default UK PC layout; a different layout or a user who wants `SYMBOL SHIFT`
somewhere else is a different table and no different code. The values are
slices because host keys map to *combinations*: `BACKSPACE` is `CAPS SHIFT`
and `0`, the cursor keys are `CAPS SHIFT` and `5` to `8`, `ESCAPE` is `BREAK`.

`HostKeys` holds which host keys are down and builds the whole matrix from them
on every event, rather than applying presses and releases as edits. Editing
gets overlapping combinations wrong the moment two are held: hold `SHIFT`,
press and release `BACKSPACE`, and the release would let `CAPS SHIFT` up while
the host is still holding `SHIFT` down. A rebuild is a walk of at most a dozen
entries and cannot drift.

`HostKey::Char` carries the character the host layout gives a physical key
*unmodified* and lower case. Layout swapping falls out of that, but the reason
it is specified that way is that press and release must name the same key: a
frontend reporting the shifted character would send `Char('A')` down and
`Char('a')` up if the user released shift first, and the matrix would keep a
key down forever. Modifiers arrive on their own.

Everything on the path from a key event to the matrix is fixed arrays —
`MAX_HELD` is twelve, and past it the oldest key held is forgotten, which is
the wrong answer to a twelve-finger chord and the right one to a key-up lost to
a window losing focus. `tests/no_alloc.rs` asserts the path allocates nothing,
because 0026 will make input a `Command` applied on the emulation thread.

### The two halves of port `0xFE` are unrelated

Writing it sets the border, `MIC` and the speaker; reading it returns the
keyboard on bits 0-4, `EAR` on bit 6, and ones on bits 5 and 7, which nothing
drives on a 48K machine. Reading back the border is not possible, which is why
the ROM keeps `BORDCR` in RAM.

`EAR` is a level the ULA holds, high at power-on, with `set_ear` for the tape
to drive as its edges go past (0016). A real 48K feeds some of the `MIC` and
speaker output back into that bit and the exact behaviour differs between issue
2 and issue 3 boards; nothing that has been written yet cares, and the loaders
that do are rare enough to wait for a machine that can load them.

### What is not here

Nothing types at a BASIC prompt yet, because there is no ROM in the machine
(0015) and no window to press keys in (0019). The frontend maps its own key
events onto `HostKey` and hands the matrix over; once 0026 lands that hand-over
is a `Command` applied at a control tick, so a recorded session replays the
same keystrokes at the same T-states.
