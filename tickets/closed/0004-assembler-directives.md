---
id: "0004"
title: Assembler directives
priority: high
created: 2026-08-11
closed: 2026-08-11
---

## Summary

The non-instruction statements: layout, data definition, symbol definition,
file inclusion and conditional assembly.

## Acceptance criteria

- [x] Layout: `ORG`, `ALIGN`, `DS`/`DEFS`
- [x] Data: `DB`/`DEFB`, `DW`/`DEFW`, `DZ`, string forms with escapes
- [x] Symbols: `EQU`, `DEFL`/reassignable, `MODULE`/`ENDMODULE`
- [x] Files: `INCLUDE`, `INCBIN` with offset and length
- [x] Conditional assembly: `IF`/`IFDEF`/`IFNDEF`/`ELSE`/`ENDIF`, nested
- [x] `INCLUDE` cycles are detected and reported with the inclusion chain
- [x] Output: raw binary
- [ ] Debug info sidecar — **deferred to 0006**, which owns the format. See
      below: the records it needs are produced here, only not serialised.

## Notes

`INCBIN` needs to resolve paths relative to the including file, not the
process working directory — a detail that is easy to get wrong and annoying to
discover later.

## As built

`assemble.rs`, which is the loop ADR-0014 describes and the first thing in this
crate that assembles a whole program rather than a statement. The test-local
driver that stood in for it in 0002 and 0003 is gone.

Paths are resolved against the directory of the file that names them, per the
ticket note, and `SourceMap` gained a path per file to make that possible. The
test for it has `main.asm` include `sub/other.asm`, which includes `third.asm`
with no path at all — a resolution against the root file or the working
directory finds nothing.

### One acceptance criterion deferred, deliberately

The debug info sidecar is 0006's, whose own criteria require the format to be
"stable and documented" and queryable in both directions. Defining a format
here and redefining it there would be waste, so what this ticket produces is
the material rather than the file: `Assembled::lines` is one `LineRecord` per
statement that emitted bytes, carrying its span — and therefore its file, line
and column — with the address and length. Serialising that, and the reverse
index, is 0006. A note has been added to that ticket.

Raw binary output is done: `Image::to_binary` and `Image::write_binary`, plus
`Image::segments` for a program with more than one `ORG`, which is a list of
runs rather than a binary with a hole the size of the gap between them.

### Conditional assembly and the fixpoint

`IF` on a forward reference cannot be decided on the first pass. An unresolved
condition is taken as false, and defining the symbol it names moves addresses,
which is exactly what asks the driver for another pass — so the branch is
assembled on the pass after the one that could not see it. Conditions inside a
skipped block are not evaluated at all, since complaining about symbols on a
branch that is not being assembled would be complaining about code that does
not exist.

Nested conditionals track "is this branch selected" separately from "is any
enclosing branch selected", so that an `ENDIF` inside a skipped block closes
its own `IF` rather than the outer one. There is a test for exactly that, since
it is the failure that turns a conditional bug into a file that assembles to
nonsense rather than to an error.

### Found while testing

- `DEFL` had been implemented as `EQU` with permission to redefine, which made
  `count DEFL count+1` a circular definition. It is not: `EQU` is held as an
  expression and evaluated when asked for, while `DEFL` is evaluated **at the
  point it is written**, which is the whole point of the form. Variables also
  had to be excluded from convergence tracking — a counter's value depends on
  how far the pass has got, so comparing one against the previous pass reports
  a change forever.
- `ORG` was recording a line record covering the distance it jumped, because
  the driver measured "did this statement emit anything" by how far the address
  moved. It now counts bytes written. The listing in 0006 would have shown a
  32 KB instruction.

### Deferred

`STRUCT`, `DUP`/`EDUP`, `REPT`, `PHASE`/`DEPHASE`, the `SAVE*`/`DEVICE` output
directives, and `LUA`. The first three belong with macros in 0005; the output
directives need the Spectrum crate to exist before "save a .tap" means
anything.
