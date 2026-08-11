---
id: "0005"
title: Assembler macros
priority: medium
created: 2026-08-11
---

## Summary

Macro expansion: `MACRO`/`ENDM` with named parameters, repetition, and label
hygiene so that a macro used twice does not produce duplicate symbols.

## Acceptance criteria

- [ ] `MACRO name params` / `ENDM`, invoked by name with positional arguments
- [ ] Repetition: `REPT`/`DUP`/`ENDR` with a loop counter symbol
- [ ] Local labels inside macros are unique per expansion
- [ ] Nested and recursive macros work, with a depth limit that reports the
      expansion stack rather than overflowing the Rust stack
- [ ] Errors inside an expansion report both the macro definition site and the
      invocation site
- [ ] Expansion is visible in the listing output (0006)

## Notes

The two-site error reporting is the debugging affordance that makes macros
tolerable to work with, and it constrains the AST design in 0001 — every node
needs to carry an expansion context, not just a span.
