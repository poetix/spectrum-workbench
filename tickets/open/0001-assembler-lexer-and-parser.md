---
id: "0001"
title: Assembler lexer and parser
priority: high
created: 2026-08-11
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

- [ ] Lexer produces tokens with byte spans; every token records its source file
- [ ] All sjasmplus numeric literal forms parse, including negative and
      character literals
- [ ] Labels: global, local (`.name`), anonymous forward/back references
- [ ] Parser produces an AST covering instructions, directives and macro calls
      without yet knowing what any of them mean
- [ ] Errors carry file, line, column and a caret span, and recovery continues
      to the next line so one typo does not cascade
- [ ] Round-trip test: every mnemonic emitted by the disassembler lexes and
      parses

## Notes

Keep the AST free of encoding decisions — operand classification (is `(hl)` a
memory operand or a parenthesised expression?) is genuinely ambiguous until
the mnemonic is known, so the parser should preserve the surface form and let
0003 decide.

The disassembler already emits `$`-prefixed hex in sjasmplus-compatible form,
which gives a ready-made corpus for the round-trip test.
