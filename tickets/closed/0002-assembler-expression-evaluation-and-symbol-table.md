---
id: "0002"
title: Assembler expression evaluation and symbol table
priority: high
created: 2026-08-11
closed: 2026-08-11
---

## Summary

Constant-expression evaluation with the operator set and precedence sjasmplus
uses, plus a symbol table that supports forward references and the two-pass (or
fixpoint) resolution assembly requires.

## Acceptance criteria

- [x] Integer expressions: `+ - * / % << >> & | ^ ~`, comparison and logical
      operators, unary minus, parentheses
- [x] `$` (current address) and `$$` (current section start) resolve correctly
- [x] Forward references resolve; genuinely circular definitions are reported
      rather than looping
- [x] Symbol scoping: global, file-local, and `MODULE`-qualified names
- [x] Errors distinguish "undefined symbol" from "not yet defined on this pass"
- [x] Overflow and truncation are diagnosed where a value cannot fit the field
      it is destined for

## Notes

The classic failure is a value that changes size between passes (a forward
reference assembled as `JR` on pass one and needing `JP` on pass two).
Decide early whether to iterate to a fixpoint or require explicit sizing, and
record the choice — it affects 0003's interface.

## As built

`eval.rs` and `symbols.rs`. The pass model is [ADR-0014](../../adr/0014-assemble-to-a-fixpoint.md).

### The pass model, and why the classic failure does not arise here

The note above assumes the x86 problem. It does not apply: **on the Z80,
instruction size is a function of the parse tree alone**. `JR` and `JP` are
different mnemonics that the programmer chooses, not encodings the assembler
picks between; `(IX+3)` and `(IX+100)` are both three bytes; an out-of-range
`JR` is an error, not a promotion. So no value anywhere changes how many bytes
an instruction occupies.

What can still move an address is `DS`, `ALIGN`, or an `IF` on a forward
reference. That is a much smaller problem, and it is what the fixpoint is for:
passes repeat until the symbol table stops changing, minimum two, capped, with
non-convergence reported rather than looped on.

The consequence for 0003 is the useful part: **it may compute an instruction's
size without evaluating any operand**, so the location counter advances past
instructions whose operands are not yet resolvable, and the first pass produces
real addresses rather than a guess.

### Two kinds of symbol, resolved two different ways

A label's value comes from the location counter, so labels are redefined on
every pass and a reference to one the pass has not reached is a forward
reference, not an error.

A constant (`EQU`) is an expression, evaluated on demand with a visiting set.
That is what makes `a EQU b` / `b EQU a` reportable *as a cycle* — it is a node
visited while already being visited — rather than merely a value that never
settles, which is all a pass counter could have told us.

### Scoping

Global, local (`.name`, belonging to the preceding non-local label, so `.loop`
under `main:` is `main.loop`), `MODULE`-qualified with outward resolution, and
`@name` used verbatim to escape both.

The ticket asked for "file-local", which sjasmplus does not have: there is no
per-file private scope to implement, and adding one would mean sources that
assemble under sjasmplus failing here. The sub-global scope is the local-label
one described above.

### Deferred

`sizeof` parses and evaluates to a clear "not supported yet"; it needs `STRUCT`,
which arrives with the directives in 0004.

### Found while testing

The symbol table did not converge for a source whose constant is defined after
it is used. Every pass re-executes the whole source, so reaching `size EQU 4`
resets that constant to unevaluated — discarding the value the *same* pass had
already computed from it when `DS size` asked. The end-of-pass value therefore
looked like `None` every time, and every pass reported a change. Fixed by
recording the value a symbol took during the pass separately from the state of
its binding. The test that caught it (`a_program_whose_addresses_move_takes_
another_pass`) asserts the pass count, not just the answer, which is the only
reason it failed rather than quietly costing eight passes.

`$$` was in the acceptance criteria but not in the lexer, which 0001 had left
out deliberately for want of a meaning to give it; it is now a token.

The tests drive a stand-in assembly pass (`tests/symbols.rs`) that walks
statements assigning addresses, since forward references and convergence are
not observable without one. It fakes instruction sizes at one byte and skips
operands spelled like registers — both are 0003's work.
