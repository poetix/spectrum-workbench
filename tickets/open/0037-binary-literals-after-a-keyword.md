---
id: "0037"
title: "`%` is read as modulo after a keyword or a macro name"
priority: low
created: 2026-08-15
---

## Summary

`ATTR equ %00000101` does not assemble. The lexer decides between a binary
literal and the modulo operator by asking whether the previous token *ends a
value* (`lex.rs`, `percent`), and an identifier does — but `EQU` is a keyword
and a macro name is an invocation, and neither can be the left operand of a
modulo. sjasmplus accepts both, and real Spectrum source is full of them:
attribute bytes, sprite rows and bit masks are conventionally written in binary.

Found while writing `games/scroller`, which now spells its binary literals
`0b00000101` to get around it.

## Acceptance criteria

- [ ] `label equ %1010` assembles
- [ ] `MYMACRO %1010,%0101` assembles
- [ ] `db 8 % 3` is still modulo, and so is `db (x) % 3`
- [ ] The existing `%` tests still pass

## Notes

The rule wants one more question than "did the last token end a value": a
keyword that takes an expression — `EQU`, `DEFL`, `DB`, `DW` — and a macro name
both put the lexer in operand position. The parser knows this and the lexer does
not, which is the shape of the problem; the cheapest fix is probably for the
lexer to keep a small set of names after which a value cannot have ended.
