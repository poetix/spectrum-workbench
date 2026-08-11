---
id: "ADR-0015"
title: A typed s-expression language above the assembler
date: 2026-08-11
status: accepted
---

## Context

`rkw-asm` assembles. The next question is what, if anything, sits above it.

The premise that makes this different from every existing 8-bit compiler is who
writes the source. The intended author is a language model working in
conversation with a human coder: the model proposes and drafts, the human reads,
steers and takes over where it matters. That changes what the language is for.
Compiler size, compile speed and self-hosting — the constraints that shaped
Small-C, PL/M and their descendants — stop mattering entirely, because the
compiler runs on a modern host. What starts mattering is whether a model can
write correct code in the language without having been trained on it, and
whether a human can review what comes out.

The ambition is not modest. It is to express the overall logic of a program at a
level a person can reason about, target optimisation at the assembly level where
it pays, and land within reach of what accomplished Spectrum programmers
achieved by hand.

That ambition survives contact with the evidence only in a specific form, and
the form is what the rest of this decision is built on:

> **The compiler never has to beat hand-written assembly. It has to be good
> enough across the ~85% of the source that accounts for ~20% of the cycles,
> and get out of the way cleanly in the rest.**

Roughly 1.2–1.5× off hand assembly in control-heavy code is achievable and
nobody notices. In tight inner loops a good Z80 compiler is 2–4× off and no
realistic effort closes that, because the register file is too irregular: three
and a half usable 16-bit pairs, `A` special-cased, `HL` special-cased, `IX`/`IY`
slow enough to be a last resort. So the loops that matter get hand-written, and
the language's job is to make that crossing cheap and checked rather than a
one-way exit.

This is precisely where SDCC and z88dk lose. Their answer to "this loop is too
slow" is *rewrite it outside the language*, and outside the language the types,
the checking and the debugger are gone. People rewrite everything, and the
language dies.

### Precedent surveyed

- **Small-C** and its descendant `sccz80` — compiler emits assembler text, which
  is the architecture adopted here. The code quality is not.
- **SDCC** — peephole optimisation as rules over emitted assembly text, rather
  than in the code generator. Cheap to write, cheap to extend, adopted.
- **Millfork** — middle-level language for 8-bit targets, static allocation by
  default, no hidden runtime. The closest thing to the semantics wanted here.
- **Cowgol** — whole-program call-graph analysis placing stack frames statically,
  which is how you avoid `IX+d` at 19 T-states per access.
- **Pre-Scheme** — a restricted, typed, GC-free Scheme compiled for systems work.
  The direct precedent for the shape chosen.
- **Habit** (HASP) — Haskell-like systems language with `bitdata` for packed
  fields and `area` declarations for memory-mapped structures. Its feature set
  is close to what is specified below, arrived at independently.

None of these was designed to be written by a model, which is where the parts of
this decision without precedent are concentrated.

## Decision

Build a small typed language with s-expression syntax, hygienic macros, and
compile-time evaluation, compiling to `rkw-asm` source.

### Syntax

S-expressions. The reasoning is comparative rather than aesthetic:

- **Against a C-like surface** (the SDCC-shaped option): C's semantics fight the
  target — implicit conversions, pointer decay, undefined behaviour — and
  nothing about C's syntax is worth that.
- **Against a Haskell-like surface**: it owns the best-known notation for
  exactly the type system wanted (`newtype`, sum types, records, signatures),
  and loses anyway on three counts. `a -> b -> c` *means* curried, so using it
  without partial application reuses the most familiar notation in the language
  with different semantics. Layout-sensitive parsing is fragile against
  generated code. And the macro story is Template Haskell, whose model prior is
  thin — spending exactly the familiarity the syntax was chosen for.
- **Against a Haskell EDSL** (the Ivory/Copilot pattern — no parser, real types,
  real metaprogramming, all free): it puts GHC in a Rust toolchain, and its
  errors are Haskell type errors, which are close to useless in a model
  feedback loop.
- **For s-expressions**: nothing to learn, unambiguous parsing that survives
  reflow, trivially tree-editable, and the best macro system in existence with
  the deepest representation in training data. Working close to the AST is a
  feature when the AST is the thing being generated.

The concepts and vocabulary are borrowed from the ML family regardless —
`defnew` for newtypes, sum-typed enums, explicit signatures. Concept transfer is
stronger than syntax transfer, and there is no currying to un-teach.

**The language is not called Scheme**, in its name, its documentation or its
error messages. Say Scheme and a model assumes R7RS and reaches for all of it;
the handrails below then read as missing features rather than as design.

### Two tiers

- **Core forms** — typed, closed, small. This is the AST, and the only thing the
  compiler reasons about. Types, control flow, functions, `let`, placement,
  inline assembly, interrupt declarations, `repeat`.
- **Macro library** — everything else, including most of what looks like the
  language. Screen geometry, attributes, sprites, keyboard, tape, banking, IM2
  setup, timing loops.

The split keeps Spectrum knowledge out of the compiler, so another Z80 machine
is a library swap rather than a fork.

### Type system

Primitives (`U8` `U16` `I8` `I16`), **ranged integers** (`(U8 0 191)`),
newtypes, `U8`-backed enums without payloads, structs with layout annotations
(`@soa`), bitfields, fixed-size arrays with the size in the type, slices,
pointers, and function pointers restricted to top-level functions with no
capture.

No generics, no runtime higher-order functions, no inference beyond local `let`.
Every signature and global is annotated.

Ranged integers earn their place twice over: bounds checks vanish, 8-bit
operations become provable where a 16-bit type would have forced the wide path,
and the range documents intent for the human reviewer.

Generics are the feature that will be asked for repeatedly. The answer is a
macro emitting a monomorphic version per type, which keeps the type checker at
"compare two names for equality".

### Compile-time evaluation

Full untyped s-expression evaluation at compile time, producing **data and
syntax only**. Nothing crosses into runtime implicitly; every crossing is an
explicit `defconst` or a macro expansion with a declared type.

This is not a convenience feature. It is where the optimisation lives:

> **The techniques that let hand-written Spectrum code outrun compilers are
> mostly not code-generation techniques. They are data-generation techniques,
> and they belong in the compiler's evaluator rather than in a side-car
> script.**

Screen row-address tables, log/antilog multiply tables, reciprocal tables for
the perspective divide, pre-shifted sprite planes, mesh edge-deduplication and
face adjacency, and compiled sprites — straight-line code with pixel data baked
in as immediate operands, transparent bytes emitting no instruction at all.
Every one of those was historically a separate build-time tool emitting
assembly, which everybody wrote and nobody kept in sync with their data. Here
they are macros in the same file as the thing they generate, re-run on every
build.

Measured on a 16×16 masked sprite: the data-driven blit written in the language
costs ~3000 T-states and lands within ~10% of hand-written assembly, because the
loop is memory-bound rather than register-bound. The compiled-sprite macro over
the same bitmap costs ~1100 T-states for ~2× the memory. The 2.7× came from
compile-time evaluation over the data, not from the code generator.

### Higher-order functions are a phase restriction, not a missing feature

Lambdas exist, and are fully higher-order — at compile time. The runtime
language has none. This makes `map` and `lambda` legal in the place where they
cost nothing, and turns the restriction into a phase error ("`map` is
compile-time only; the runtime form is `for`") rather than an unknown
identifier. Much easier to teach in a single diagnostic.

Runtime indirection is served by `fnptr`: a jump table with a type, `JP (HL)`,
no capture. Refusing it just pushes dispatch into hand-written assembly that
cannot be checked.

### Handrails

The model will reach for idioms the target cannot support. Every one gets a
diagnostic naming the legal replacement, because the context window holds the
last error and not the manual. `cons`/`car`/`list` → no heap, use an array or a
struct. Runtime `map`/`lambda` → compile-time only. `apply` and varargs → fixed
arity. `call/cc` → absent. Strings and `display` → no runtime string type.
Floats → fixed-point library. Self-recursion → needs a declared stack budget.

Beyond diagnostics:

- **Budgets that fail the build.** `@cycles`, `@stack`, `@size` per bank —
  stated as an intent (`(fps >= 12)`) and reduced to a T-state ceiling by the
  compiler. Static where the code is analysable; worst-case-observed from an
  instrumented emulator run where it is not, which is most hot loops, because
  their cost is data-dependent.
- **Capability declarations** — `clobbers`, `uses shadow`, `no-interrupts`,
  `@bank`. Hidden machine state made explicit and checkable, and somewhere for
  the model to put intent it would otherwise leave in a comment.
- **Model-specific lints** for the legal-but-usually-wrong: 16-bit arithmetic
  where 8 bits suffice, structs copied by value, recursion, indirect calls in
  hot loops.
- **`unimplemented`** stubs that typecheck, so a whole program's shape and
  memory layout can be reviewed before any body exists.
- **Machine-readable output** — diagnostics, layout, cycle reports and the
  compiler's own feature list as JSON, so the model never guesses what the
  toolchain supports.
- **`--explain`** — register allocation, frame layout, and why each helper call
  was emitted, so both the model and the human can review code generation rather
  than only source.

`*` and `/` are deliberately **not** operators. The Z80 has neither instruction,
and an infix `*` that silently emits a 300 T-state helper hides exactly the cost
this language exists to expose. Multiplication is a named call with a
project-wide policy (`shift-add`, `log-table`, `unrolled-const`); division does
not exist, and you use a reciprocal table or a shift.

### The seam

What makes the ceiling hand-written assembly rather than compiler output is
being able to reach assembly *without leaving the toolchain*:

- Inline `asm` with declared clobbers, **verified against the allocator** rather
  than trusted.
- `@naked` — body is yours, signature and cycle budget still checked.
- **`patchable` operands** — self-modifying code as a checked language feature.
  The step, mask and direction patched into an unrolled Bresenham, or the screen
  address patched into a compiled sprite's `LD HL,nn`. The compiler knows which
  operand bytes are writable and refuses writes anywhere else. Without this,
  every hot loop becomes `@naked` and all checking is lost.
- **Emitted assembly is legal `rkw-asm` source**, commented with originating
  source lines, hand-editable and re-assemblable.
- **Source locations survive macro expansion**, with an expansion trail the
  debugger can follow back through each macro to the original line.

Optimisation is at the assembly layer, as peephole rules in a data file rather
than in the code generator. Rules are testable in isolation against the existing
round-trip corpus, and the model can propose new ones.

### Load-bearing, and not deferrable

**Macros operate on syntax objects carrying source locations, from the first
commit.** Retrofitting source locations onto naive s-expressions after a macro
library exists is miserable, and without them source-level stepping dies the
moment the library becomes useful — which is the capability this whole project
exists to provide. Every other decision here can be revisited. This one cannot.

### Non-goals

No garbage collector, no heap, no runtime closures, no dynamic dispatch beyond
`fnptr`, no separate compilation, no standard library beyond what the Spectrum
needs, no self-hosting.

## Consequences

**Positive:**

- Nothing to learn syntactically, and no semantic prior to un-teach beyond a
  bounded, enumerable list of rejections — each of which is a diagnostic.
- The macro layer moves domain abstraction above the assembler, where it can be
  typed and hygienic, while emitted assembly stays flat and steppable. The macro
  assembler is the target; generated code does not lean on its macros.
- Compile-time evaluation collapses the build-time-tool problem. Artwork, mesh
  data, tables and the code that consumes them live in one file and one diff.
- The seam gives an escalation ladder — language, then language plus inline
  assembly, then hand assembly with a checked signature — with no rung requiring
  an exit from the toolchain.
- Cycle-accurate measurement already exists in this repo, and it is what turns
  "this must fit in a scanline" from a comment into a build failure. That
  measurement loop is arguably a larger contribution than the language.

**Negative:**

- Per-type arithmetic without type classes is the visible ugly spot: `q+`/`q*`
  for fixed-point against `+` for `U8`. Honest about cost, worse to read than a
  language with overloading, and macros cannot fix it without inference. Accepted
  deliberately rather than discovered later.
- **Flags are a register the language cannot see**, and this is the single
  largest source of the gap against hand-written code. The Spectrum's
  down-a-line sequence is fast because it branches on flags a previous
  instruction already set; a naive `if` re-tests what the flags already know and
  leaks 10–20% on every pointer-walking loop. Mitigated by intrinsics for the
  known sequences and by letting functions declare a flag result
  (`@flags (nz . crossed-cell)`), but not eliminated.
- A compile-time evaluator in Rust is where scope creep will come from. It
  starts as "enough to build a table" and the model will keep asking for more
  library. The data-and-syntax-only boundary is the containment.
- Two evaluators — untyped compile-time and typed runtime — can drift in
  behaviour. Shared numeric semantics need testing as such.
- The model is a mediocre Z80 programmer, so assembly written at the seam is
  itself a risk. The checked contract plus emulator verification means bad
  assembly fails rather than ships, which is a safety net and not a fix.
- Compiled sprites cannot clip, and clipped variants are meaningfully slower.
  Strategy selection is a real space/time trade the model must make with numbers
  in front of it, which is why `@size` budgets must land alongside `@cycles`.

## Follow-on

A **benchmark corpus** is how this stays honest, and it fits the testing culture
already in the repo. Five routines with known-good hand-written versions —
Bresenham, an 8×8 sprite blit, a 3×3 transform loop, a memory fill, a collision
sweep — compiled and measured both ways, with the ratio tracked as a regression
test. Expect ~3–4× at first pass and 1.5–2.5× once ranged types, declared
calling conventions and the peephole rules are in. A ratio moving the wrong way
should be a failing test, not a demo that flickers.

Build order: core forms, type checking and flat code generation first, with no
macros, to prove the pipeline against the emulator. Then compile-time
evaluation, which is the earliest large payoff. Then macros on syntax objects.
Then budgets and timing contracts.

Measurement infrastructure comes before code-generation quality. It is what
identifies which weaknesses actually cost cycles, and it prevents the compiler
being optimised on intuition.

Three sub-decisions are likely to want ADRs of their own once there is code:
the calling convention and frame-layout scheme, the flag-result mechanism, and
the debug-information format that carries the macro expansion trail.

The language does not have a name yet. It must not be "Scheme".
