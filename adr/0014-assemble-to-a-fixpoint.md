---
id: "ADR-0014"
title: Assemble to a fixpoint, because instruction size does not depend on value
date: 2026-08-11
status: accepted
---

## Context

A label can be referred to before it is defined, so its address is not known
the first time the assembler walks the source. Every assembler resolves this
either by making a fixed number of passes or by iterating until nothing moves.

The classic hazard is a value that changes the *size* of what it appears in: a
forward branch assembled short on one pass needs a long encoding on the next,
which moves everything after it, which changes the branch again. Assemblers for
architectures with branch relaxation either iterate carefully or make the
programmer write the width explicitly.

That hazard is much smaller here than it looks, and the reason is worth
recording because it is what makes the rest of the choice easy:

> **On the Z80, instruction size is a function of the parse tree alone.**

`JR` and `JP` are different mnemonics, chosen by the programmer; the assembler
never picks between them. `LD A,(HL)` and `LD A,(nn)` differ in the surface form
of the operand, not in the magnitude of any value. `(IX+3)` and `(IX+100)` are
both three bytes. So no expression value anywhere changes how many bytes an
instruction occupies — and an out-of-range `JR` is an error rather than a silent
promotion, which is also what sjasmplus does.

What can still move an address is a directive that reserves space or aligns:
`DS n` with a forward-referenced `n`, `ALIGN`, and conditional assembly that
takes a different branch once a symbol is known.

## Decision

Iterate to a fixpoint. Passes repeat until the symbol table reports that nothing
changed, with a minimum of two passes and a cap after which the assembly is
reported as not converging, naming the symbols that were still moving.

A minimum of two: the first pass is what makes forward references knowable, so
no result can be trusted until a second pass agrees with it.

Errors from a non-final pass are provisional. A reference to a symbol the pass
has not reached yet is `NotYetDefined`, which the driver discards; from the
second pass onwards the same reference is `Undefined`, which is real. Only the
final pass's diagnostics are reported.

Constants (`EQU`) are not part of that iteration. They are expressions,
evaluated on demand with a visiting set, so a cycle is detected as a cycle and
reported — `a EQU b` where `b EQU a` names itself rather than quietly failing to
settle for eight passes.

## Consequences

**Positive:**
- No explicit sizing syntax, so sources written for sjasmplus work unchanged.
- Convergence is normally reached in two passes and provably so for any source
  that does not use `DS`/`ALIGN`/`IF` on a forward reference, because nothing
  else can move an address.
- Ticket 0003 may compute an instruction's size from the parse tree without
  evaluating anything, which means the location counter can advance past an
  instruction whose operand is not yet resolvable. That is what keeps the first
  pass useful rather than a guess.
- Non-convergence is reported rather than being an infinite loop or a silently
  wrong binary.

**Negative:**
- A pathological source can be made not to converge (`IF` on a symbol that the
  branch itself moves). The cap turns that into an error message, but it is an
  error message about the assembler's model rather than about the mistake.
- Every pass re-evaluates every constant, since the labels underneath them may
  have moved. Constants are memoised within a pass but not across passes.
