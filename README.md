# spectrum-workbench

A ZX Spectrum emulator and Z80 macro assembler in Rust, built around debugging.

The intent is that you write code in the assembler and run it in the emulator
with breakpoints, source-level stepping and a view of what the machine is
actually doing. The emulator is being grown in stages towards running the 48K
ROM with tape loading and saving, screen output and sound.

**Status: early.** The CPU core and disassembler are complete and validated.
Nothing else exists yet.

## What works

The `z80` crate is a complete Z80 CPU core and disassembler, with no knowledge
of the Spectrum. It covers the full instruction set including the undocumented
opcodes — `SLL`, the `IXH`/`IXL` register halves, the `DD CB` register-copy
forms, `IN (C)`, `OUT (C),0`, and the duplicate `IM` and `NEG` encodings — and
models the undocumented flag bits, the internal `WZ`/MEMPTR register, and the
`Q` latch that `SCF` and `CCF` consult.

Decoding follows the octal decomposition of the opcode byte rather than a flat
256-way match, so the regularity of the instruction set does the work and the
undocumented encodings fall out instead of being enumerated.

The core never adds up T-states itself. Every access is issued to the `Bus` as
a discrete machine cycle of its real duration, and purely internal cycles are
explicit. This is what makes it possible to add the Spectrum's contended memory
later without touching a single instruction implementation: contention is
defined as wait states inserted at particular cycles, so the cycles have to be
in the right places before the quirks can be.

### Validation

| Suite | Result |
| --- | --- |
| Fuse Z80 test suite | 1331 / 1335, four documented divergences |
| `zexdoc` | all 67 groups pass |
| `zexall` (undocumented flags included) | all 67 groups pass |

The Fuse suite is the more interesting of the two, because it records the time
at which every bus cycle completed. That checks *where* each machine cycle sits
inside an instruction, not merely how many there are — which is exactly the
property contention depends on.

The four divergences are all `BIT n,(HL)`. That suite predates the discovery of
MEMPTR and expects the undocumented flags to come from the byte read out of
memory; the researched behaviour takes them from `WZ`, which the instruction
does not itself set, so the correct answer depends on state the test files do
not specify. See the note in `crates/z80/tests/fuse.rs`.

## Layout

```text
crates/
  z80/            CPU core and disassembler
adr/              architecture decision records
docs/
  architecture.md performance measurements and analysis
tickets/
  open/           planned work
  closed/
scripts/
  fetch-testdata.sh
```

Planned: `rkw-asm` (macro assembler), `rkw-dbg` (debugger core),
`rkw-spectrum` (ULA, memory map, tape), `rkw-cli`, `rkw-gui`.

## Design

Decisions are recorded as ADRs in [`adr/`](adr/). The ones that shape
everything else:

- [ADR-0002](adr/0002-machine-cycle-granular-bus-interface.md) — the CPU never
  sums T-states; every access is a discrete machine cycle. This is what lets
  contended memory be added later without touching a single instruction.
- [ADR-0001](adr/0001-decode-opcodes-by-octal-decomposition.md) — decoding by
  the octal decomposition of the opcode byte, so the undocumented instructions
  fall out of the structure rather than being enumerated.
- [ADR-0003](adr/0003-model-wz-and-q-from-the-outset.md) — modelling the
  internal `WZ` and `Q` registers from the first commit, because retrofitting
  them is a search through forty instructions for two flag bits.
- [ADR-0007](adr/0007-emulation-on-its-own-thread-with-three-channels.md) —
  emulation is a hot path that pushes compact signals to another thread;
  control comes back at control rate.

Planned work is in [`tickets/open/`](tickets/open/).

## Building

```sh
cargo build
cargo test
cargo clippy --all-targets
```

The release profile uses fat LTO and a single codegen unit, because an
interpreter is one hot loop spread across several modules and cross-module
inlining is worth more here than usual. For a machine-specific build:

```sh
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## Conformance tests

The Fuse and `zexall` data is **not** vendored, because those projects are
licensed differently to this one. Fetch it first:

```sh
scripts/fetch-testdata.sh
```

Then:

```sh
cargo test --test fuse
cargo test --release --test zex -- --ignored --nocapture
```

The exercisers run for billions of T-states — around 40 seconds each in a
release build — so they are marked `#[ignore]`. The tests that need this data
skip with a message when it is absent, so `cargo test` works on a fresh clone.

## Licence

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 licence, shall
be dual licensed as above, without any additional terms or conditions.

### Third-party test data

`scripts/fetch-testdata.sh` downloads test data that is *not* covered by the
above and is not distributed with this repository:

- the Fuse Z80 test suite, from the Fuse emulator (GPL-2.0-or-later)
- `zexdoc` and `zexall`, Frank Cringle's instruction exerciser (GPL-2.0)
