---
id: "0021"
title: Remaining CPU quirks
priority: low
created: 2026-08-11
---

## Summary

The known-unmodelled corners of the CPU core, deferred because they need
hardware around them to be observable or testable.

## Acceptance criteria

- [ ] `ED` page NONI opcodes suppress interrupt sampling for the following
      instruction
- [ ] `LD A,I` / `LD A,R` reset P/V when an interrupt arrives mid-instruction
- [ ] Interrupt line sampled at the correct sub-instruction point
- [ ] raxoft `z80test` suite passes: `z80full`, `z80doc`, `z80flags`,
      `z80memptr`, `z80ccf`
- [ ] `z80ccf` specifically validates the `Q` latch, which no current test
      covers

## Notes

`z80test` ships as `.tap` files and as sjasmplus sources. Running it needs
0015; assembling it from source would also be a serious end-to-end exercise
of the assembler (0005).
