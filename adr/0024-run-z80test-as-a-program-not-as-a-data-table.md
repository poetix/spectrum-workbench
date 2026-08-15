---
id: "ADR-0024"
title: Run z80test as a program, not as a data table
date: 2026-08-14
status: accepted
---

## Context

raxoft's `z80test` is the only conformance data in this project whose expected
values were measured on real hardware rather than produced by another emulator.
It is 160 instruction groups, each of which runs an exhaustive permutation of
one instruction and CRCs the registers and flags it leaves behind, against a
table Patrik Rak captured from a 48K Spectrum with a Zilog Z80 in it.

It ships two ways: as `.tap` tapes, and as sjasmplus sources with the CRC table
inline. So there are two ways to run it.

**As a data table.** Parse `tests.asm`, lift the 160 vectors and their expected
CRCs into Rust, and drive the permutations from a harness of our own against
`Cpu` and `FlatMemory`. Fast, granular — a failure names its group without
anything having to read a screen — and it needs no ROM, no tape and no ULA.

**As a program.** Boot the 48K ROM, mount the tape, type `LOAD ""`, and read
the answer off the emulated screen. Which is what a person with the tape would
do.

## Decision

Run it as a program.

The harness types `J""` and ENTER at the `K` cursor, answers the ROM's
`scroll?` prompt with ENTER every screenful, and asserts on the suite's own
summary line — `Result: 000 of 160 tests failed.` — scraped from the display
file through the ROM's own font.

## Consequences

**Positive:**
- The suite's own harness is part of what gets tested. `idea.asm` is several
  hundred instructions of vector combination, CRC accumulation and `EX AF,AF'`
  that runs 160 times over; reimplementing the driver in Rust would replace
  that with our own code and test the emulator against fewer instructions, not
  more.
- The CRC table stays where its author put it. Lifting 160 four-way CRC rows
  out of assembler source into Rust is a transcription with 640 chances to be
  wrong, and a wrong expected value fails as "the CPU is broken".
- It found things a CPU-only harness could not have. The nine `IN` groups fail
  on a *precondition* — the suite executes `OUT (0xFE),0x07` then `IN A,(0xFE)`
  and refuses to run if the answer is not `0xBF` — which is a fact about the
  ULA's `EAR` feedback and not about the Z80 at all. A harness built on
  `FlatMemory` would have had no port `0xFE` to get wrong, would have passed,
  and would have left a real bug in the machine.
- One test covers the CPU, the ULA, the keyboard, the screen decode, the tape
  and the ROM at once, which nothing else in the tree does.

**Negative:**
- A failure reports a count, and the names have to be collected as they scroll
  past. A line caught mid-print comes back with `?` cells in it and is dropped,
  so the names are best-effort where the count is exact.
- It cannot run without a ROM and a tape, neither of which is in the tree
  (ADR-0005). The tests skip with a message when either is missing, which means
  a broken CPU passes CI on a machine that has not fetched them.
- It is slower per group than a direct harness would be, and the whole set is
  `#[ignore]`d. Six seconds of wall clock for all seven suites, against roughly
  seven hours of emulated time.
- Typing at the ROM is timing-dependent in a way a data table would not be: the
  keystroke helper exists because the ROM debounces over frames, and a change
  to the frame clock could break the test in a way that has nothing to do with
  the CPU.

**Rejected:** running only `z80full` on the grounds that it subsumes the others.
It does not — `z80ccf` is the only one that exercises the `Q` latch of ADR-0003
against every instruction, and `z80memptr` is the only one that puts `BIT n,(HL)`
after each group to expose `WZ`.
