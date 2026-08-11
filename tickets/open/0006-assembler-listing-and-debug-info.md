---
id: "0006"
title: Assembler listing and debug info
priority: medium
created: 2026-08-11
---

## Summary

Human-readable listing output, and the machine-readable debug information the
debugger needs to map addresses back to source.

## Acceptance criteria

- [ ] Listing shows address, encoded bytes, source line, and macro expansion
      nesting
- [ ] Symbol table dump, sorted by name and by address
- [ ] Debug info sidecar: address to (file, line, column) line table, symbol
      table with types where known, and macro expansion records
- [ ] Line table is queryable in both directions — address to source, and
      source to the addresses a line generated
- [ ] Format is stable and documented, since the debugger depends on it

## Notes

Source-to-address must be one-to-many: a line inside a macro used five times
generates five addresses, and "set a breakpoint on this line" should set five.
