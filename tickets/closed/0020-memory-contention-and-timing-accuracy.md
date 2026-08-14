---
id: "0020"
title: Memory contention and timing accuracy
priority: medium
created: 2026-08-11
closed: 2026-08-14
---

## Summary

The ULA's contention of CPU access to the lower 16K of RAM, computed rather
than tabulated (ADR-0009), plus the floating bus and I/O contention.

## Acceptance criteria

- [x] Contention delay computed arithmetically from an 8-byte pattern table
- [x] Applied only to addresses 0x4000-0x7FFF and only during the display
      period
- [x] I/O contention for ports, which follows different rules to memory
- [x] Floating bus reads return the byte the ULA is fetching
- [x] Frame constants verified against a published reference before landing
- [x] Test: a timing-sensitive demo effect renders correctly

## Notes

The machine-cycle-granular bus (ADR-0002) means this should touch no
instruction implementation — only the `Bus` implementation. If it turns out
otherwise, something in the core is wrong.

The constants in `docs/architecture.md` are from memory and must be checked.

## As built

### ADR-0002's promise did not hold, and that is the main finding

The note above says that if this ticket needs changes in `cpu.rs`, something in
the core is wrong. It did, and something was: `tick(n)` carried a duration and
no address.

A Z80 leaves the last address it used on the address bus for the whole of an
internal cycle, and the ULA arbitrates on the address lines alone — it cannot
see `MREQ` and cannot tell an internal cycle from a read, so it stalls each of
those T-states individually if the address is in the contended bank. `INC (HL)`
in the lower 16K is held four times rather than three, `LDIR` copying within it
seven times per iteration rather than five, and `ADD HL,rr` seven times or not
at all depending on where `I` points.

Which address is held is instruction-local — `IR` after an opcode fetch, the
operand address after a memory cycle, `PC` after a displacement byte, `SP` mid
`EX (SP),HL`, `DE` mid `LDIR`, `BC` mid `OTIR`. A bus cannot recover it, and a
bus that assumed "the last address I was given" would get every post-`M1`
internal cycle wrong, because the answer there is `IR` and the last address the
bus saw was `PC`. So the core had to state it. `tick_at(addr, t)` was added to
the trait and thirty-one call sites across `cpu.rs`, `exec_ed.rs` and
`exec_cb.rs` were changed to use it. ADR-0023 records this; ADR-0002 has been
amended to point at it rather than left claiming otherwise.

`tick(t)` survives for the one case where ADR-0002's reasoning was right: the
wait states of an interrupt acknowledge, which assert `M1` and `IORQ` together,
a combination the ULA does not arbitrate on.

The refactor is behaviour-preserving on an uncontended bus, and the Fuse suite
passing unchanged at 1331/1335 before and after is the evidence for that.

### A wait state moves the phase

The rule for internal cycles is `t` separately contended one-T-state cycles, not
one cycle of `t`. This is not pedantry: each wait moves the clock, which moves
where the next lookup lands. Seven internal T-states starting on the first
contended T-state of a frame cost 2, 0, 6, 0, 6, 0, 6 — not the 2, 1, 0, 0, 6,
5, 4 that reading down the pattern suggests. Three of the tests in
`tests/contention.rs` were written with the wrong arithmetic first and corrected
against the machine; the working is in the comments so the next person does not
have to make the same mistake.

### The constants were remembered nearly right

Checked against Fuse (`libspectrum/timings.c`, `fuse/spectrum.c`,
`fuse/machines/spec48.c`), which is the same project ADR-0005 already takes the
ROM and the conformance data from. `T_STATES_PER_LINE`, `LINES_PER_FRAME`,
`T_STATES_PER_FRAME`, `INTERRUPT_LENGTH`, `CLOCK_HZ` and `FIRST_DISPLAY_T` all
agree with the 48K row. The pattern `6,5,4,3,2,1,0,0` is right.

One number was wrong and is the sort that would have been hard to find later:
contention starts at **14,335**, not at 14,336 where the display does. An access
beginning on that T-state is still holding the bus when the ULA wants it.

Fuse writes the same rule with the pattern rotated one place and the window
shifted one place to match. Rather than transcribe results, the unit test in
`src/contention.rs` transcribes Fuse's *arithmetic* — `contend_delay_common`
with the 48K timings substituted in — and asserts the two agree for all 69,888
T-states of the frame. That is the check that would catch a rotation error; a
handful of spot values would not.

The one place this crate and Fuse differ is how the 120 non-display lines are
split between border and retrace (16/56 here, 24/48 there). That decides how
much border a frontend shows and nothing else; the T-state geometry contention
depends on is identical. Left alone.

### The I/O sample point moved

Reads now sample the port three T-states into the cycle rather than one. ADR-0002
put it at one on the reasoning that `IORQ` asserts there — which is true, and is
where a *write* is issued, but a read is latched at the end. It matters here
because the floating bus is entirely a question of when the byte is taken:
sampling at T+1 gives the byte from before the ULA's fetch. Writes still go out
at T+1, which is what keeps a border stripe on the scanline its `OUT` was on.

### ADR-0009's performance argument did not survive being measured

The ticket asked for contention to be measured because it runs per machine
cycle. It was, in `tests/throughput.rs`, against the `Spectrum`'s own bus with
the contention removed and nothing else changed — built by wrapping a real
`Spectrum` so that the trait's default wrappers are what runs, which is exactly
the pre-0020 arrangement and cannot drift from it.

| | M inst/s | ratio |
| --- | --- | --- |
| no contention (baseline) | 183 | 1.00 |
| free addresses: the range test alone | 144 | 0.79 |
| contended addresses, border: the arithmetic too | 117 | 0.64 |
| contended addresses, display: and the stalls | 119 | 0.65 |

Getting that to mean anything took two corrections worth recording. `FlatMemory`
is the wrong baseline — it is an array index where a `Spectrum` is a memory map,
a ULA, a tape deck and a frame clock, so most of the difference is not
contention. And the two loops were in different inlining regimes: with one
`Cpu::step` call site per bus type LLVM inlined the whole interpreter into the
simpler bus's loop and not into the `Spectrum`'s, which on its own was larger
than the effect being measured. A second call site apiece fixes it. Stubbing
`is_contended` to `false` then measures 0.97, which is what says the two are
comparable and that the 0.79 above is the check and not the layout.

Then the ADR itself. It chose eight bytes of arithmetic over a 68 KB table on a
cache argument that had never been run. Building the table:

| | computed | tabulated |
| --- | --- | --- |
| the lookup alone | 892 M/s | 2266 M/s |
| in the machine | 114 M inst/s | 150 M inst/s |

The table wins both, which is the opposite of what ADR-0009 argues. That is not
enough to overturn it: the argument rests on a 32-48 KB L1d where 68 KB does not
fit, against the 128 KB of the machine it was measured on, and on an emulated
program with a working set of kilobytes, against a benchmark loop touching 256
bytes. Both conditions would move the result back. Neither can be tested on this
hardware.

So the decision stands and its justification does not. ADR-0009 has been amended
to say exactly that, and ticket 0032 is to settle it on an x86 part with a real
workload. Tuning the benchmark until it agreed with the ADR was the other option
and would have been dishonest.

### What is not there

- **128K and Pentagon timings.** The shape is right for them — different
  constants, same arithmetic — but `contention` hard-codes the 48K numbers and
  `is_contended` is a range rather than a per-page lookup. A second machine
  makes that a parameter; one machine does not.
- **`I` in the contended bank is modelled, snow is not.** Setting `I` to
  `0x40-0x7F` correctly slows the machine down, because `tick_at` is given `IR`.
  What it does not do is corrupt the display, which is what the real thing does
  and what makes the trick unusable rather than merely slow.
- **The interrupt acknowledge is uncontended**, following Fuse. The cycle
  asserts `M1` and `IORQ` together and the ULA does not arbitrate on it. This is
  Fuse's model rather than something checked against hardware.
- **Sub-scanline border effects** are still one colour per line, as ticket 0013
  left them. Contention was the prerequisite named there, so this is now
  buildable; it is not built.
- **The floating bus is 48K only.** A 128K machine reads it from whichever
  screen is being displayed and returns different values in the idle slots.
- **No `.tzx`-era loader has been run against it.** The ROM loader still loads a
  block off a waveform with contention on, which is the evidence there is that
  the timing did not break anything real.
