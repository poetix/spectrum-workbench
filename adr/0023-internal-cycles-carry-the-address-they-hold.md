---
id: "ADR-0023"
title: Internal cycles carry the address they hold
date: 2026-08-14
status: accepted
supersedes: partially ADR-0002
---

## Context

ADR-0002 gave the CPU a machine-cycle-granular bus so that ULA contention could
be added later as a change to one `Bus` implementation, touching no instruction.
Its consequences section says so explicitly:

> Contention becomes a change to one `Bus` implementation and touches no
> instruction. If stage 0020 turns out to need changes in `cpu.rs`, that is a
> signal something in the core is wrong.

Stage 0020 needed changes in `cpu.rs`. What was wrong was not the shape of the
bus but one of its six methods:

```rust
fn tick(&mut self, t: u32);
```

Internal cycles were issued as a bare duration. The reasoning was that during
them the CPU is not driving the bus in a way anything else can see. On a Z80 in
a Spectrum that is false twice over.

The chip leaves the last address it used on the address bus for the whole of an
internal cycle. And the ULA has nothing to arbitrate on *but* the address lines:
it cannot see `MREQ`, it cannot tell an internal cycle from a read, and it
stalls the CPU on any T-state whose address is in the contended bank. So each
T-state of an internal cycle is contended, separately, against whatever the CPU
happens to be holding.

The effect is not marginal. `INC (HL)` with `HL` in the lower 16K is stalled
four times rather than three; `LDIR` copying within it is stalled seven times
per iteration rather than five; `ADD HL,rr` is stalled seven times if `I` points
into the bank and not at all if it does not. A model that skipped internal
cycles would be wrong for most of the instruction set most of the time.

Crucially, *which* address is held is instruction-local knowledge. It is `IR`
after an opcode fetch, the operand address after a memory cycle, `PC` after a
displacement byte, `SP` in the middle of `EX (SP),HL`, `DE` in the middle of
`LDIR`, `BC` in the middle of `OTIR`. No bus implementation can recover it, and
a bus that guessed "the last address I was given" would get every post-`M1`
internal cycle wrong — the address there is `IR`, and the last address the bus
saw was `PC`.

## Decision

Internal cycles carry their address:

```rust
/// Burn `t` T-states of internal CPU activity with `addr` held on the
/// address bus.
fn tick_at(&mut self, addr: u16, t: u32) { self.tick(t) }
```

`tick(t)` survives for the one case where the claim ADR-0002 made is actually
true: the wait states of an interrupt acknowledge, which assert `M1` and `IORQ`
together — a combination no memory cycle produces and which the ULA does not
arbitrate on.

`t` is spent as `t` separately contended one-T-state cycles rather than as one
cycle of `t`, because that is what the ULA sees. It matters: a wait state moves
the clock, which moves the phase, so seven internal T-states starting on the
first contended T-state of a frame cost 2, 0, 6, 0, 6, 0, 6 and not the
2, 1, 0, 0, 6, 5, 4 that reading straight down the pattern suggests.

Thirty-one call sites across `cpu.rs`, `exec_ed.rs` and `exec_cb.rs` were
changed from `tick` to `tick_at`. Every one is a mechanical substitution of the
address the chip is holding; none changes what an instruction computes, and the
Fuse conformance suite passes unchanged before and after, which is the evidence
that the refactor is behaviour-preserving on an uncontended bus.

## Consequences

**Positive:**
- Contention is correct for internal cycles, which is most of where it is.
- The address is stated where it is known and nowhere else, so a future machine
  with different contention rules — a 128K, a Pentagon — needs no further core
  change.
- `tick` versus `tick_at` now names a real distinction: whether the CPU is
  holding an address that something else can arbitrate on.

**Negative:**
- ADR-0002's promise was wrong, and anything else resting on "the core is
  finished, hardware is all in the `Bus`" should be re-examined rather than
  trusted. The two decorators in `rkw-debug` and `rkw-spectrum` had to grow a
  `tick_at` forwarder, and a decorator that forgets one silently loses
  contention — the same hazard the wrappers already had, now with one more
  method to forget.
- An instruction implementation can now be wrong in a way that no
  timing-total test catches: passing the right *duration* with the wrong
  *address* costs nothing on an uncontended bus and is invisible to Fuse, whose
  expected timings are uncontended. `tests/contention.rs` covers the cases where
  the address is not the obvious one; the rest rests on the transcription from
  Fuse's generator being right.

**What this does not change:** the rest of ADR-0002 stands. Machine cycles still
carry their real durations, the CPU still never sums T-states, and contention is
still confined to one `Bus` implementation — it is only the *interface* that
needed one more piece of information, not the architecture.
