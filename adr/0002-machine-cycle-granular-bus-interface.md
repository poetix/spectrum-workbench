---
id: "ADR-0002"
title: Machine-cycle-granular bus interface
date: 2026-08-11
status: accepted
amended-by: ADR-0023
---

## Context

The Spectrum's ULA contends for memory access to the lower 16K of RAM, stalling
the CPU for a number of T-states that depends on exactly when within the frame
the access falls. Getting that right is what makes timing-sensitive software
work.

Contention is therefore a property of *when each individual bus cycle happens*,
not of how long an instruction takes in total. An emulator that computes
"`LD A,(HL)` costs 7 T-states" and adds 7 to a counter has thrown away the
information contention needs, and retrofitting it means rewriting every
instruction.

We are building the idealised CPU first and the hardware later, so the risk is
of baking in a shape that cannot accept the hardware.

## Decision

The CPU never sums T-states. Every access is issued to the `Bus` trait as a
discrete machine cycle carrying its real duration, and purely internal cycles
are issued as explicit `tick(n)` calls:

```rust
fn fetch_opcode(&mut self, addr: u16) -> u8;   // M1: 4 T-states
fn read_cycle(&mut self, addr: u16) -> u8;     // 3
fn write_cycle(&mut self, addr: u16, v: u8);   // 3
fn input_cycle(&mut self, port: u16) -> u8;    // 1 + 3
fn output_cycle(&mut self, port: u16, v: u8);  // 1 + 3
fn tick(&mut self, t: u32);
```

The wrappers have default bodies spending the standard durations; a contended
implementation overrides them, keeping every timing decision in one place.

I/O cycles are split 1 + 3 rather than issued as a single 4, because `IORQ`
does not assert until one T-state in and the ULA samples the bus at that point.

## Consequences

**Positive:**
- Contention becomes a change to one `Bus` implementation and touches no
  instruction. If stage 0020 turns out to need changes in `cpu.rs`, that is a
  signal something in the core is wrong.

  **It did.** `tick(n)` carries no address, and the ULA contends internal
  cycles against the address the CPU is holding through them. See ADR-0023,
  which replaces `tick` with `tick_at(addr, t)` at every mid-instruction call
  site. The rest of this decision stands.
- The Fuse conformance suite records the time at which every bus cycle
  completed, so this structure is directly testable — and 1331 of 1335 cases
  pass, which validates cycle *placement* and not merely instruction totals.
- The same hooks serve memory watchpoints and the event ring.

**Negative:**
- More verbose than returning a cycle count. Each instruction must place its
  internal `tick` calls correctly, and getting them wrong is silent until a
  timing test catches it.
- Slight cost from the extra calls, though monomorphisation and inlining make
  it unmeasurable in practice — the core runs at roughly 360× real time.
