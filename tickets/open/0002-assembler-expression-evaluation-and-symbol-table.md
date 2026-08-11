---
id: "0002"
title: Assembler expression evaluation and symbol table
priority: high
created: 2026-08-11
---

## Summary

Constant-expression evaluation with the operator set and precedence sjasmplus
uses, plus a symbol table that supports forward references and the two-pass (or
fixpoint) resolution assembly requires.

## Acceptance criteria

- [ ] Integer expressions: `+ - * / % << >> & | ^ ~`, comparison and logical
      operators, unary minus, parentheses
- [ ] `$` (current address) and `$$` (current section start) resolve correctly
- [ ] Forward references resolve; genuinely circular definitions are reported
      rather than looping
- [ ] Symbol scoping: global, file-local, and `MODULE`-qualified names
- [ ] Errors distinguish "undefined symbol" from "not yet defined on this pass"
- [ ] Overflow and truncation are diagnosed where a value cannot fit the field
      it is destined for

## Notes

The classic failure is a value that changes size between passes (a forward
reference assembled as `JR` on pass one and needing `JP` on pass two).
Decide early whether to iterate to a fixpoint or require explicit sizing, and
record the choice — it affects 0003's interface.
