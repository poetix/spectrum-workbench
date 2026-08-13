---
id: "0030"
title: Assembler syntax bugs found against real source
priority: high
created: 2026-08-13
closed: 2026-08-13
---

## Summary

Four bugs where `rkw-asm` rejects source sjasmplus accepts, found by pointing
the assembler at raxoft's `z80test` (see 0031 for what that exercise turned up
more generally). All four are wrong behaviour rather than missing features, and
all four would bite any real-world source file.

## Acceptance criteria

- [x] `0x…d` is hexadecimal, not decimal. `split_radix` in `lex.rs` tests the
      `d` decimal suffix before the `0x` prefix, so `0xbd` lexes as decimal
      `0x` + `bd` and reports "`x` is not a decimal digit". `0xbe` is fine;
      only the ones ending in `d` or `D` fail
- [x] A dot-prefixed word in column 1 whose remainder is a *mnemonic* is a
      local label. `is_op_name` strips a leading `.` before consulting a table
      holding both mnemonics and directives, so `.scf`, `.ret`, `.exx`, `.ldi`
      and the rest parse as instructions and the label is lost. The dot form
      belongs to directives (`.db`, `.org`), which have no `.ld` equivalent —
      so strip the dot for the directive table only
- [x] `INCLUDE` accepts an unquoted filename: `INCLUDE MORE.I` as well as
      `INCLUDE "MORE.I"`, both documented
- [x] `EX AF,AF`, `EX AF` and `EXA` all assemble as `EX AF,AF'`, which the
      sjasmplus documentation lists explicitly as accepted spellings
- [x] Test: each of the four has a regression case in the assembler tests

## Notes

The dot-label one is the interesting one. It is not a lexing slip: the column-1
label rule genuinely cannot be decided from the characters alone, and the fix
is to narrow which table the dot-stripping consults rather than to add a
special case. A local label called `.db` stops working, which is the same
restriction sjasmplus has.

Angle-bracket includes (`INCLUDE <VDP.I>`) are deliberately not in scope: they
select include-path search order, and there are no include paths yet. That
belongs with the rest of 0031.

## As built

All four are one-place fixes, and each regression test went into the file that
owns the behaviour rather than into a ticket-shaped test file of its own.

### A prefix beats a suffix

`split_radix` examined suffixes before prefixes, which is right for `0FFh` — it
is why the `h` check comes first — but wrong for `0xbd`, where the trailing `d`
claimed the literal for decimal and left `0x` as its digits. Moving the
`0x`/`0q`/`0b` prefix block above the `d` check is the whole fix. `0d` is still
decimal zero, because a `0` prefix followed by no radix letter falls through to
the suffixes as before.

The failure had a distinctive shape worth remembering: it hit exactly the bytes
whose low nibble is `d`, so a hex table looked half-corrupt rather than
uniformly broken, and the error it produced named `x` rather than `d`.

### Which table the dot is stripped for

`is_op_name` stripped a leading `.` and looked the remainder up in a table
holding mnemonics and directives together, so `.scf` was `SCF`. Splitting the
lookup — dot-stripped names consult the directives only — is enough, because
the dot form exists for directives (`.db`) and there is no `.ld`. Sources name
their local labels after the instruction under test, so this was the single
biggest source of misparses in a real file, and the errors it produced pointed
at the *next* token rather than at the label.

The cost is that a local label called `.db` no longer works, which is the same
restriction sjasmplus has.

### A file name is taken from the source, not the parse tree

The unquoted form is handed to the expression parser, which makes what it can
of `main.asm`, so `path_argument` now takes the argument's span and reads the
snippet back out of the `SourceMap` instead of interpreting the tree. That is
the only honest reading: what the characters mean as an expression is beside
the point when they are a file name.

### Four spellings of one opcode

`EX AF,AF'`, `EX AF,AF`, `EX AF` and `EXA` all produce `0x08`. `EXA` needed
adding to both mnemonic tables — `encode`'s, which decides what is an
instruction rather than a macro call, and `keywords`', which decides what is a
label in column 1. It is an input alias only: the disassembler still emits
`EX AF,AF'`, so the round-trip corpus is unaffected.

### What it was measured against

Pointing the assembler at pristine `z80test` v1.2a sources took `z80full` from
1230 errors to 967. Everything remaining is one of the sjasm-only constructs
listed in 0031 — `.@name`, `@#`, macro parameter defaults, and `IFIDN` for
`z80ccf` — and none of it is parity work.
