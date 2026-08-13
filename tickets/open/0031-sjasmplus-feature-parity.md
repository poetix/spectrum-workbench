---
id: "0031"
title: sjasmplus feature parity
priority: medium
created: 2026-08-13
---

## Summary

Close the gap between what ADR-0011 promised — "existing Spectrum sources
assemble without translation" — and what `rkw-asm` currently implements, so
that source we want to build in future builds without being rewritten first.

Implemented today: `ORG`, `ALIGN`, `DS`/`DEFS`/`BLOCK`, `DB`/`DEFB`/`DM`/
`DEFM`/`BYTE`, `DZ`/`DEFZ`, `DW`/`DEFW`/`WORD`, `MODULE`/`ENDMODULE`,
`INCLUDE`, `INCBIN`, `END`, `EQU`/`DEFL`/`=`/`:=`, `IF`/`IFN`/`IFDEF`/
`IFNDEF`/`ELSE`/`ELSEIF`/`ENDIF`, `MACRO`/`ENDM`, `REPT`/`DUP`.

## Acceptance criteria

Data and expression directives:

- [ ] `ABYTE`, `ABYTEC`, `ABYTEZ`
- [ ] `D24`, `DP`, `DEFH`/`DH`, `DC`, `DEFG`/`DG`, `HEX`
- [ ] `DEFARRAY` and `DEFARRAY+`
- [ ] `DEFINE`, `DEFINE+`, `UNDEFINE`
- [ ] `DISPLAY`, `ASSERT`, `FPOS`, `SIZE`, `TEXTAREA`, `ENCODING`, `OPT`

Structural:

- [ ] `WHILE`
- [ ] `STRUCT`/`ENDS`, including `FIELD` and the module-local scoping
- [ ] `PHASE`/`DEPHASE`/`UNPHASE`
- [ ] `@.name` inside a macro. A leading `@` already escapes the enclosing
      module, but `@.kip:` in a macro body silently defines nothing at all
      rather than the local label it should
- [ ] `::` forces a global label. Currently parsed and then ignored: `lab::`
      inside `MODULE v` still defines `v.lab`
- [ ] Named macro arguments at the call site: `RECT x=1, y=2` in any order,
      spaces around `=` allowed
- [ ] Include-path search, and the angle-bracket `INCLUDE <file>` form that
      selects search order

- [ ] Test: a corpus of real sjasmplus source assembles, checked against
      sjasmplus's own output where a binary can be diffed

## Notes

Deliberately out of scope, in descending order of how likely we are to want
them later:

- The output-format family — `DEVICE`, `MMU`, `SLOT`, `PAGE`, `BANK`,
  `SAVEBIN`, `SAVESNA`, `SAVETAP`, `SAVENEX`, `SAVEHOB`, `EMPTYTAP`, `OUTPUT`,
  and the CPC and +3DOS variants. These are not assembler work; they are an
  emulator-adjacent output layer and should wait until there is a machine to
  target (0012 onwards).
- `LUA`/`ENDLUA`/`INCLUDELUA`, `SHELLEXEC`, `RELOCATE_*`, `CSPECTMAP`,
  `SLDOPT`, `BPLIST`. Embedding a scripting language is a different project,
  and the rest serve tooling we do not have.

Explicitly **not** parity work, despite being what prompted this ticket:
`z80test` is built by `sjasm`, not `sjasmplus`, and needs `IFIDN`, the `@#`
macro invocation counter, macro parameter defaults (`param:0`) and `.@name`
scope escape. None of the four appear anywhere in the sjasmplus documentation.
Supporting them is a separate decision about sjasm compatibility, not a step
towards this ticket. See the note added to ADR-0011.
