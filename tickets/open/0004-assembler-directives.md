---
id: "0004"
title: Assembler directives
priority: high
created: 2026-08-11
---

## Summary

The non-instruction statements: layout, data definition, symbol definition,
file inclusion and conditional assembly.

## Acceptance criteria

- [ ] Layout: `ORG`, `ALIGN`, `DS`/`DEFS`
- [ ] Data: `DB`/`DEFB`, `DW`/`DEFW`, `DZ`, string forms with escapes
- [ ] Symbols: `EQU`, `DEFL`/reassignable, `MODULE`/`ENDMODULE`
- [ ] Files: `INCLUDE`, `INCBIN` with offset and length
- [ ] Conditional assembly: `IF`/`IFDEF`/`IFNDEF`/`ELSE`/`ENDIF`, nested
- [ ] `INCLUDE` cycles are detected and reported with the inclusion chain
- [ ] Output: raw binary, and a `.sld`-style or custom debug info sidecar

## Notes

`INCBIN` needs to resolve paths relative to the including file, not the
process working directory — a detail that is easy to get wrong and annoying to
discover later.
