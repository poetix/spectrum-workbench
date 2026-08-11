---
id: "0003"
title: Assembler instruction encoding
priority: high
created: 2026-08-11
closed: 2026-08-11
---

## Summary

Turn parsed instructions into bytes. The inverse of the disassembler, and it
should share its structure: the same octal decomposition (ADR-0001) read in the
opposite direction.

## Acceptance criteria

- [x] Full documented instruction set encodes correctly
- [x] Undocumented forms accepted: `SLL`, `IXH`/`IXL`, the `DD CB` register-copy
      forms, `IN (C)`, `OUT (C),0`
- [x] Operand classification resolves `(nn)` versus `nn` and register-versus-
      symbol ambiguity using the mnemonic
- [x] Out-of-range `JR`/`DJNZ` displacements are a clear error naming the
      distance and the limit
- [x] Property test: assemble then disassemble every encoding, and assert the
      result re-assembles to identical bytes
- [x] End-to-end test: assemble a short source, load the bytes into a
      `FlatMemory`, run them on the CPU, and assert the resulting registers and
      memory. The property test above never leaves the assembler; this is the
      only check that what it emits is what the hardware does.

## Notes

The round-trip property test is the high-value item here and should be written
first. It subsumes a very large number of hand-written cases and is what will
catch operand-order mistakes in the four-byte `DD CB d op` forms.

## As built

`operand.rs` and `encode.rs`.

### Planning and emitting are separate

`plan` works out the shape of an instruction — prefix, opcode, where the operand
bytes go — from the parse tree alone. `emit` fills the operand bytes in once the
values are known. This is ADR-0014 made concrete: an instruction's length never
depends on a value, so the first pass advances the location counter correctly
past instructions whose operands refer to labels it has not yet reached, and the
addresses it produces are real rather than provisional.

`tests/encode.rs::length_is_known_without_evaluating_anything` is the guard on
that: it plans `ld (ix+unknown),unknown` and friends with no symbol table at all
and checks the lengths.

### Operands ask rather than being classified

Each mnemonic's encoder asks its operands what they could be and takes the first
answer that fits a form it has. Nothing classifies an operand up front, because
nothing about an operand decides what it is: `C` is a condition in `JP C,nn`, a
register in `LD A,C` and a port in `IN A,(C)`; `(HL)` is memory in `LD A,(HL)`
and a jump target in `JP (HL)`.

The one ordering subtlety is that `LD`'s special forms have to be tried before
its immediate form, or `LD A,I` reads as `LD A,n` with a symbol called `I`. The
round-trip test caught that immediately.

`(IX+d)` displacements are handled by rebuilding the expression with the index
register replaced by zero, so `(ix+lo-2)` works: `+` and `-` associate to the
left, so the index register is the leftmost leaf and everything else is the
displacement.

### The round-trip is text-stable, not byte-stable

The ticket asked for assemble-disassemble-reassemble to identical bytes. It is
implemented as text to bytes to text, because byte equality is not the right
invariant: some encodings are not canonical. `DD 40` is `LD B,B` with a prefix
that changes nothing, and the `ED` page has two-byte no-ops; both disassemble to
an instruction that assembles to the shorter form. Requiring the *text* to
survive is the strongest property that holds for every opcode in every page,
and it is checked for all of them, twice over (positive and negative
displacements).

Hand-checked encodings sit alongside it, so that an error shared by the
assembler and the disassembler cannot pass unnoticed — the round trip alone
would agree with itself.

### Found while testing

`Plan::len` counted entries rather than bytes, so a 16-bit immediate counted as
one. Every label after a `LD HL,nn` would have been placed a byte early on the
first pass — and, because the second pass would then move them, the fixpoint
would have hidden it as "one more pass" rather than as a wrong answer. Caught by
the length test, not by the round trip, which only ever encodes one instruction
at a time.

### Deferred

- sjasmplus's "fake instructions" (`LD HL,DE` expanding to two real
  instructions, `LD BC,DE`, and so on). They are a source-compatibility feature
  rather than an encoding one, and they need a decision about whether an
  assembler this project owns should silently emit instructions nobody wrote.
- `JP HL` without parentheses. Only `JP (HL)` is accepted, which is what the
  disassembler emits.
- `OUT (C),0` requires the literal `0`, not an expression that evaluates to
  zero, since the CPU supplies the value rather than the instruction carrying
  it.
