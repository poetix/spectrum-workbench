---
id: "0001"
title: Assembler lexer and parser
priority: high
created: 2026-08-11
closed: 2026-08-11
---

## Summary

Front end for `rkw-asm`: turn source text into an AST, with spans good enough to
report errors against the original file and column. Syntax follows sjasmplus so
that existing Spectrum sources and documentation apply (ADR-0011).

Covers tokenising labels, mnemonics, registers, numeric literals in every base
sjasmplus accepts (`$1234`, `0x1234`, `1234h`, `#1234`, `%1010`, `0b1010`),
strings, character literals, operators and comments; and parsing lines into
label / mnemonic / operand-list / directive forms.

## Acceptance criteria

- [x] Lexer produces tokens with byte spans; every token records its source file
- [x] All sjasmplus numeric literal forms parse, including negative and
      character literals
- [x] Labels: global, local (`.name`), anonymous forward/back references
- [x] Parser produces an AST covering instructions, directives and macro calls
      without yet knowing what any of them mean
- [x] Errors carry file, line, column and a caret span, and recovery continues
      to the next line so one typo does not cascade
- [x] Round-trip test: every mnemonic emitted by the disassembler lexes and
      parses

## Notes

Keep the AST free of encoding decisions — operand classification (is `(hl)` a
memory operand or a parenthesised expression?) is genuinely ambiguous until
the mnemonic is known, so the parser should preserve the surface form and let
0003 decide.

The disassembler already emits `$`-prefixed hex in sjasmplus-compatible form,
which gives a ready-made corpus for the round-trip test.

## As built

`crates/rkw-asm`, in five modules: `source` (files and spans), `diag`
(diagnostics and their rendering), `lex`, `ast`, `parse`, plus `keywords`.

Syntax was taken from the sjasmplus 1.23 documentation rather than from memory,
which changed several details that had been assumed:

- Temporary labels are referenced as `1_F` / `1_B`, not `1F` / `1B`. The short
  forms are accepted too, except that `1b` collides with binary `1` and the
  literal wins there — documented at the top of `lex.rs`.
- Octal (`0q14`, `14q`, `14o`) and digit separators (`12'345`, `1_3_7q`) exist
  and are implemented.
- Escape sequences are processed between double quotes only; between
  apostrophes a doubled apostrophe is one apostrophe. The escape table is
  sjasmplus's, not C's — `\d` is 127 and `\e` is 27.
- A `z` or `c` immediately after a closing quote is part of the literal.
- There is no `? :` in the expression grammar. There are word spellings for
  most operators (`mod`, `shl`, `and`, `low`, `high`, `abs`), `<?` and `>?` for
  min and max, and `>>>`.

The round-trip test is stronger than "lexes and parses": the AST keeps the
source spelling of every literal and operator, so the test disassembles every
opcode in every prefix page (twice, for positive and negative displacements),
parses the text, prints the tree back and compares it with what it started
from. A lost operand or a renormalised literal fails it.

Two things the ticket did not anticipate:

- Telling a label from a mnemonic in column 1 cannot be done without knowing
  which names are mnemonics and directives, because sjasmplus makes the colon
  optional there. `keywords.rs` holds that name set. It is a set of names and
  not a table of meanings, but it does mean **adding a directive in 0004 or
  0005 means adding its name there**.
- `size equ 40` needs a further rule, since `SIZE` is itself a directive name:
  a word followed by `EQU`, `DEFL`, `MACRO`, `FIELD` or `=` is a label whatever
  it is called.

### Deliberately deferred

Parsed as ordinary syntax or not at all; each needs a stage that knows what it
means, and none of them blocks 0002 or 0003:

- `{address}` / `{b address}` memory reads, and the `..` string concatenation
  operator, which collides with `.` being a valid identifier character.
- `<...>` quoting of macro arguments — 0005, which is the only place it is
  unambiguous against the `<` operator.
- `label+digit:` SMC offsets.
- `!` and `#` inside identifiers, which sjasmplus allows and this lexer does
  not: `!` cannot be told from the start of `!=`, and `#` cannot be told from
  the hex prefix.
- `$$`, whose meaning depends on the paging model that does not exist yet.

ADR-0011 anticipated that "sjasmplus's behaviour is defined by its
implementation rather than by a specification, so edge cases must be
discovered". The five details listed above are the first instalment of that,
and they came from the documentation rather than from the source, so more will
surface once real sources are assembled.
