---
id: "0021"
title: Remaining CPU quirks
priority: low
created: 2026-08-11
closed: 2026-08-14
---

## Summary

The known-unmodelled corners of the CPU core, deferred because they need
hardware around them to be observable or testable.

## Acceptance criteria

- [x] `ED` page NONI opcodes suppress interrupt sampling for the following
      instruction
- [x] `LD A,I` / `LD A,R` reset P/V when an interrupt arrives mid-instruction
- [x] Interrupt line sampled at the correct sub-instruction point
- [x] raxoft `z80test` suite passes: `z80full`, `z80doc`, `z80flags`,
      `z80memptr`, `z80ccf`
- [x] `z80ccf` specifically validates the `Q` latch, which no current test
      covers

## Notes

`z80test` ships as `.tap` files and as sjasmplus sources. Running it needs
0015; assembling it from source would also be a serious end-to-end exercise
of the assembler (0005).

## As built

All seven suites pass 160/160: the five named above plus `z80docflags`, which
the ticket did not list, and `z80doc` loaded off the real waveform rather than
through the `LD-BYTES` trap. Six seconds of wall clock for the lot, which is
about seven hours of emulated Spectrum.

The harness is `crates/rkw-spectrum/tests/z80test.rs`, rewritten from last
session's spike. It boots the ROM, mounts the tape, types `J""` and ENTER at
the `K` cursor, answers `scroll?` with ENTER, and asserts on the suite's own
`Result: NNN of 160 tests failed.` line. ADR-0026 records why it runs the suite
as a program rather than lifting the CRC table into Rust — briefly, because
running it as a program is what found the `EAR` bug below, and a Rust harness
built on `FlatMemory` would have had no port `0xFE` to get wrong.

`scripts/fetch-testdata.sh` grew the release-archive step, with the zip's
SHA-256 checked before unpacking for the same reason the ROM's is: a different
release has different expected CRCs baked in, and a test that loaded one and
reported against the other would be wrong twice.

### What was found

**The nine IN groups were not the floating bus.** The handover said groups
095-103 fail because `Spectrum::input` returns `0xFF` for unattached ports and
the suite CRCs what a real floating bus hands back. That is not what happens.
Every IN vector in the suite uses port `0xFE` — `IN A,(N)` has `n` fixed at
`0xFE`, and the `(C)` forms have `C` fixed at `0xFE` — so the ULA answers all of
them and the floating bus never comes into it. What fails is a *precondition*:
`main.asm` runs `OUT (0xFE),0x07` at startup and then, before each IN group,
`XOR A : IN A,(0xFE) : CP 0xBF`, and refuses to run the group at all if the
answer is not `0xBF`. It prints `FAILED / IN FE:xx / Expected:BF` and counts it.

`0xBF` rather than `0xFF` because bit 6 of a port `0xFE` read is not idle-high:
the speaker output is wired back into the `EAR` input, so with bit 4 last
written low the bit reads low. `Ula::ear` now models that — a tape drives the
line while it is playing, and otherwise the line answers with the speaker bit of
the last write. This is the issue 3 rule (Fuse's `ula_default_value`); see below
for what was left out.

Fixing it took `z80doc` and `z80docflags` from 9 failures to none, and the other
suites from 11 to 4. It also means the old `ula.rs` test named
`what_was_written_to_port_fe_is_not_what_is_read_back` was asserting something
false, and is now
`the_border_is_write_only_but_the_speaker_comes_back_on_bit_six`.

**089/090 were not interrupts either.** The handover read `LDIR->NOP'` as an
interrupt accepted mid-repeating-instruction. The suite runs `DI` as its first
instruction and `EI` as its last, so no interrupt is ever taken. What the `NOP'`
groups actually do is point `DE` at the instruction's own second byte, so the
`LDIR` overwrites its `B0` with the byte it just copied and the repeat re-fetches
`ED 00`, a NOP. The plain `.ldir` group has `BC` set so that it does *not*
repeat; the `NOP'` group is the only one that leaves a repeating iteration's
flags where anything can see them.

So the failure was the flags of a repeating block instruction. When one goes
round again it spends an extra machine cycle putting `PC` back, and the CPU
rewrites flags 5 and 3 from bits 13 and 11 of that `PC` — David Banks, 2018,
corroborated by Peter Helcmanovsky's test. The I/O forms additionally recompute
`H` and `P/V` from the transferred byte with `B` adjusted by one in whichever
direction bit 7 of that byte says, and set `MEMPTR` to `PC + 1`. Implemented as
`Cpu::repeat_flags` and `Cpu::block_io_repeat_flags`, transcribed from
`redcode/Z80` rather than derived.

That is the whole of the remaining 4 failures in `z80full`, `z80flags` and
`z80ccf`, and the 2 in `z80memptr` — which were `MEMPTR` on the I/O forms, a
thing the core was not setting on a repeat at all.

**The `ED` NONI criterion was based on a misreading.** The term comes from
Cristian Dinu's decoding tables, where `NONI` is an invented mnemonic meaning
"no operation, and no interrupt immediately after this one", and an invalid
`ED xx` is written as "NONI followed by NOP". What is suppressed is an interrupt
between the `ED` byte and the byte after it — not one after the pair. The
criterion as written, "suppress interrupt sampling for the following
instruction", reads it as the latter, which no reference supports and
`redcode/Z80` explicitly contradicts ("functionally equivalent to two
consecutive `nop` instructions").

The behaviour the criterion was pointing at is real and the core already had it,
for free: `ED xx` is a single `Cpu::step`, so there is no point at which an
interrupt could be taken inside it. No code changed. Two tests were added to pin
it, and the stale comment in `exec_ed.rs` that the criterion came from has been
corrected rather than left to mislead the next reader.

### What else changed

`LD A,I` / `LD A,R` set `Cpu::iff2_read`, and an interrupt accepted on the
following boundary clears `P/V` — the NMOS bug from the Zilog data book and
Roshchin's 1998 note. The `Q` latch follows the flag, on the reasoning that the
instruction is still what wrote it and only the value differs; nothing tests
that and it is an assumption.

`RETN` / `RETI` set `Cpu::iff_restored` when they found `IFF1` and `IFF2`
disagreeing, and the boundary after such a return is not a sampling point
(Weissflog, 2021). Not in the ticket; it is the same family of thing and it is
five lines.

`Ula::interrupt_pending` now answers for one T-state earlier, because the Z80
reads `INT` during the last T-state of the instruction rather than at the
boundary after it. The window that takes the interrupt moves from `[0, 32)` to
`[1, 33)`. Fuse has the same off-by-one as the old code; the ROM boots
identically either way, which is why it went unnoticed.

### What was rejected

**Issue 2 board emulation.** The `EAR` feedback differs: issue 2 feeds back
`MIC | speaker`, issue 3 just the speaker. Nothing in the tree can select a
board issue, so modelling both would add a branch no test could reach, and the
two agree on everything the ROM and `z80test` do. The rule is in the doc comment
on `Ula::ear` for whoever needs it.

**Lifting the CRC table into Rust.** ADR-0026.

**Running only `z80full`.** It does not subsume the others: `z80ccf` is the only
suite that puts a `CCF` after every instruction, which is what validates the `Q`
latch of ADR-0003, and `z80memptr` is the only one that puts `BIT n,(HL)` there
for `WZ`.

### What is not there

The exact position *within* the final T-state at which `INT` is read. `INT` is a
level on this bus and is sampled at the boundary with a one-T-state offset,
which is right for a machine that holds the line for 32 T-states and would not
be for one that pulsed it for less. No machine this core drives does.

`z80ccfscr` — the seventh tape in the archive — is not run. It is a visual test
that paints a pattern revealing which SCF/CCF variant a CPU has, and asserting
on it would mean pinning a framebuffer hash of somebody else's output; `z80ccf`
already covers the same behaviour with a CRC.

The suites skip with a message when the ROM or the tapes are absent, so a broken
CPU passes on a machine that has not run `scripts/fetch-testdata.sh` and
`scripts/fetch-rom.sh`. That is the ADR-0005 bargain and it applies here more
sharply than elsewhere, because this is the only suite measured on real
hardware.
