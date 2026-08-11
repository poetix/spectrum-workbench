---
id: "ADR-0011"
title: sjasmplus-compatible assembler syntax
date: 2026-08-11
status: accepted
---

## Context

The macro assembler needs a syntax. The options were to follow sjasmplus, which
is the de-facto modern standard for Spectrum development; to follow the simpler
classic assemblers such as pasmo; or to design something new with debugging
metadata as a first-class concern.

## Decision

Follow sjasmplus: rich macros, `MODULE` scoping, local and anonymous labels,
`REPT`/`DUP`, structs, and its full range of numeric literal forms (`$1234`,
`0x1234`, `1234h`, `#1234`, `%1010`).

## Consequences

**Positive:**
- Existing Spectrum sources assemble without translation, and existing
  documentation and tutorials apply directly.
- A large corpus of real code is available to test against — raxoft's `z80test`
  suite in particular ships as sjasmplus sources, and is both a serious
  exercise of the macro system and a suite we want to run anyway.
- No syntax bikeshedding.

**Negative:**
- More to implement than a classic assembler, and sjasmplus's behaviour is
  defined by its implementation rather than by a specification, so edge cases
  must be discovered.
- Some sjasmplus features are historical accretions we would not choose.
  Compatibility means implementing them anyway, or documenting the gaps.

**Note:** the debugging metadata ambition that motivated the "design our own"
option is not actually in tension with this. Debug info is an output format
(ticket 0006), not a syntax feature.
