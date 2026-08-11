---
id: "0005"
title: Assembler macros
priority: medium
created: 2026-08-11
closed: 2026-08-11
---

## Summary

Macro expansion: `MACRO`/`ENDM` with named parameters, repetition, and label
hygiene so that a macro used twice does not produce duplicate symbols.

## Acceptance criteria

- [x] `MACRO name params` / `ENDM`, invoked by name with positional arguments
- [x] Repetition: `REPT`/`DUP`/`ENDR` with a loop counter symbol
- [x] Local labels inside macros are unique per expansion
- [x] Nested and recursive macros work, with a depth limit that reports the
      expansion stack rather than overflowing the Rust stack
- [x] Errors inside an expansion report both the macro definition site and the
      invocation site
- [x] Expansion is visible in the listing output (0006) — the records are
      produced here; 0006 draws the listing from them

## Notes

The two-site error reporting is the debugging affordance that makes macros
tolerable to work with, and it constrains the AST design in 0001 — every node
needs to carry an expansion context, not just a span.

## As built

### The note above is wrong, and usefully so

Expansion context cannot live on an AST node, because a macro body is **one**
set of nodes however many times it is used. `.loop` written once in a macro
called five times is one `Statement` and five expansions; a field on the node
could only hold one of them.

Expansion context is dynamic state, so it lives where the other dynamic state
already does — a stack in the assembler, exactly like the `INCLUDE` stack from
0004. A diagnostic raised anywhere is decorated on its way out with the
expansions in force: the error itself points into the macro body, which is
where the mistake is *written*, and a note per level points at the invocation,
which is what tells the reader why the values were what they were.

This is why 0001's decision to keep spans plain turned out to cost nothing.

### Substitution is by expression

Arguments are bound as parsed expressions and substituted wherever a body
identifier matches a parameter name, keeping the argument's own span so that a
complaint about a value points at the call that supplied it.

That covers more than it sounds like: since operands are expressions and
register names are identifiers, `load b,(hl)` passes a register and an
addressing mode to a macro that writes `ld register,source`.

What it does not cover is textual substitution — passing a mnemonic as an
argument, or passing something containing a comma as one argument (sjasmplus
uses `<...>` for that). Both are deferred, and neither is reachable from a
tree that is substituted rather than re-lexed.

### Hygiene reuses local labels

Entering an expansion sets the name that `.local` labels hang off to one unique
to that expansion, so `.loop` becomes `wait#0.loop` and `wait#1.loop` without
any new mechanism. A label written *without* a dot is still global, and using a
macro that defines one twice is still a duplicate — which is the behaviour the
name asks for, and it has a test.

### Parser change

`MACRO plot x,y` is the one statement in the language whose first operand is
separated from the second by a space rather than a comma. The parser now allows
that after `MACRO` specifically, which is the second place it knows a name
(after `keywords.rs`). Both spellings work: `plot MACRO x,y` and
`MACRO plot x,y`.

A macro invoked in **column 1** still needs a colon on the label before it, for
the same reason 0001 documented: a word in column 1 that is not a known
mnemonic or directive is a label. Indented invocations — which is how anyone
writes them — are unaffected.

### Found while testing

A macro call recorded a line record covering the whole expansion, on top of the
records the expanded statements make for themselves. The listing would have
shown overlapping entries and the debug info would have had two answers for one
address. `INCLUDE` had exactly the same bug, unnoticed since 0004, because both
emit bytes through statements that record themselves.

### Deferred

`STRUCT`, `PHASE`/`DEPHASE`, `<...>`-quoted arguments, and macros that take a
mnemonic as a parameter.
