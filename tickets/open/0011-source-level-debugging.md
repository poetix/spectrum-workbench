---
id: "0011"
title: Source-level debugging
priority: medium
created: 2026-08-11
---

## Summary

Join the assembler's debug info (0006) to the debugger (0008, 0010) so
breakpoints can be set on source lines and the current position shown as source
rather than an address.

## Acceptance criteria

- [ ] `break file.asm:42` resolves through the line table, setting a breakpoint
      at every address that line generated
- [ ] Stopping displays the source line, with macro expansion context when
      inside an expansion
- [ ] `list` shows source around the current position
- [ ] Symbol names resolve in expressions: `break main`, `x/16 screen_buffer`
- [ ] Stale debug info is detected (source newer than the binary) and warned
      about rather than silently misleading

## Notes

Depends on the debug info format from 0006 being stable.
