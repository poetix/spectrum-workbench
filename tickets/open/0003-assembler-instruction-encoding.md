---
id: "0003"
title: Assembler instruction encoding
priority: high
created: 2026-08-11
---

## Summary

Turn parsed instructions into bytes. The inverse of the disassembler, and it
should share its structure: the same octal decomposition (ADR-0001) read in the
opposite direction.

## Acceptance criteria

- [ ] Full documented instruction set encodes correctly
- [ ] Undocumented forms accepted: `SLL`, `IXH`/`IXL`, the `DD CB` register-copy
      forms, `IN (C)`, `OUT (C),0`
- [ ] Operand classification resolves `(nn)` versus `nn` and register-versus-
      symbol ambiguity using the mnemonic
- [ ] Out-of-range `JR`/`DJNZ` displacements are a clear error naming the
      distance and the limit
- [ ] Property test: assemble then disassemble every encoding, and assert the
      result re-assembles to identical bytes

## Notes

The round-trip property test is the high-value item here and should be written
first. It subsumes a very large number of hand-written cases and is what will
catch operand-order mistakes in the four-byte `DD CB d op` forms.
