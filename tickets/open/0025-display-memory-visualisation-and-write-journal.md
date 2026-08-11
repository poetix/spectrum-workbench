---
id: "0025"
title: Display memory visualisation and write journal
priority: medium
created: 2026-08-11
---

## Summary

See what a rendering routine is doing while stepping it: the display file — or
any buffer with the same layout — drawn as pixels at every stop, with the writes
that produced the change since the previous stop marked and attributable to the
instructions that made them (ADR-0016).

## Acceptance criteria

**Core — the journal**

- [ ] Write journal armed over an address range, sharing the watchpoint bitmap
      so an unarmed journal costs nothing on the hot path (ADR-0008)
- [ ] Fixed-size ring, overwrite-oldest, drop count surfaced; no allocation on
      the emulation thread (ADR-0007)
- [ ] Records carry address, value written, the PC that wrote it, and the
      T-state, so a multi-write instruction is legible after the fact
- [ ] Journal drained at each stop and readable as an ordered list; cleared on
      resume so a stop reports only that step's writes
- [ ] Test: one `LDIR` across the armed range yields one journal record per byte
      in order, all attributed to the `LDIR`'s PC

**Core — the screen query**

- [ ] Display decode exposed as a query: base address in, indexed pixels plus
      palette out, no formatting
- [ ] Base address defaults to $4000 and accepts any address, so back buffers
      and the 128K shadow screen render through the same path
- [ ] Attribute plane located relative to the base, ink/paper/bright honoured
- [ ] Flash rendered in a fixed phase, so two stops at the same place produce
      identical pixels
- [ ] Optional border colour from the last port $FE write

**REPL**

- [ ] `screen [address]` renders to the terminal with half-block characters
- [ ] `x` memory dump and the register/flag dump consume the same executor
      results the adapter does (0010)

**Adapter**

- [ ] Custom request returning pixels, palette, and the journal entries since
      the previous stop, coalesced to the pixels they changed
- [ ] Response sized for a stop, not a frame: indexed pixels, no image codec
- [ ] Journal entries carry their source position where debug info has one, so a
      changed pixel is traceable to a line (0011)

**Extension**

- [ ] Screen pane refreshing on every `stopped` event, drawn to a canvas
- [ ] Pixels changed since the previous stop highlighted, with the highlight
      distinguishable from ink and paper in every attribute combination
- [ ] Selecting a changed pixel reveals the write that made it: address, value,
      PC, and source line
- [ ] Pane address configurable, defaulting to $4000, so a back buffer can be
      watched instead
- [ ] Zoom to at least 3×, with an accurate aspect ratio at every zoom

## Notes

Depends on 0012 for the screen layout, 0009 for the ring, 0010 for the executor
and 0023 for the adapter. The journal half is useful before any pane exists —
`info writes` after a step is a real debugging tool on its own.

Attaching the visualiser to a variable through VSCode's debug visualizer API is
better than a standalone panel if that API is available and stable at the time;
the fallback is a command-opened webview, and the adapter side is identical
either way.

Deliberately not here: watching the screen while running free. That is the
frontend's job (0019), and per-write journalling at uncapped speed would be the
same problem the trace ring has (0022). This feature is for a stopped machine.
