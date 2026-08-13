# spectrum-workbench

A ZX Spectrum emulator and Z80 macro assembler in Rust, built around debugging.

The intent is that you write code in the assembler and run it in the emulator
with breakpoints, source-level stepping and a view of what the machine is
actually doing. The emulator is being grown in stages towards running the 48K
ROM with tape loading and saving, screen output and sound.

**Status: early.** The CPU core and disassembler are complete and validated, and
the assembler is finished: it turns source into Z80 machine code that runs on
it, with macros, conditional assembly, a listing and debug information. The
debugger works — breakpoints, watchpoints, stepping, source-level breakpoints
and listings, and a gdb-style REPL that assembles a file and runs it — but
nothing runs a Spectrum yet, so there is no screen, no keyboard and no tape.

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

### The assembler

`rkw-asm` assembles sjasmplus-compatible source (ADR-0011) into Z80 machine
code. The front end lexes and parses it into a syntax tree; diagnostics carry a
file, line, column and caret span, and recovery continues at the next line, so
one typo does not cascade and one run reports every independent mistake.

The tree records what was written rather than what it means: `(HL)` is a
parenthesised identifier until instruction selection decides otherwise, and the
spelling of every literal and operator survives — `10`, `$0A` and `%1010` are
distinct nodes with the same value. Nothing later has to unpick a decision the
parser had no business making.

That is what makes the main test possible. It disassembles every opcode in every
prefix page, parses the text, prints the tree back, and requires it to be
identical to what it started from — so a merged operand or a renormalised
literal fails rather than passing as "it parsed". The encoder then closes the
loop: the same corpus is assembled and disassembled again, and has to come back
the same text. Every opcode in every page, including the undocumented ones.

Beyond that the assembler is checked against the CPU rather than against itself:
short programs are assembled, loaded where they were assembled for, and run, and
the tests assert what the machine did with them. Those two checks answer
different questions — the round trip catches a wrong encoding, and running it
catches an encoding that the disassembler agrees with and the hardware does
not.

The assembler emits a listing — address, bytes, source line, and macro
expansions marked by depth — and a debug information sidecar in a
[documented, versioned format](docs/debug-info.md). The debugger reads that to
answer both of the questions a raw binary cannot: which source produced the
instruction at an address, and which addresses a line of source produced. The
second is one-to-many, because a line inside a macro used five times produced
five of them.

Macros expand with positional arguments bound as expressions, so an argument can
be a register or an addressing mode as easily as a number. Local labels inside a
macro hang off a name unique to each expansion, and an error inside one points
at the macro body, where the mistake is written, with a note per level naming
the invocation that led there. `REPT`/`DUP` repeat a block, optionally naming
their counter.

The directives are there too: `ORG`, `ALIGN` and `DS` for layout, `DB`/`DW`/`DZ`
for data, `EQU` and `DEFL`, `MODULE` scoping, `INCLUDE` and `INCBIN` — resolving
paths against the including file's directory, and reporting an include cycle
with the chain that produced it — and nested conditional assembly. A conditional
on a forward reference is decided on the pass after the one that could not see
it, which is the fixpoint earning its keep.

Behind the front end are expression evaluation and the symbol table: 32-bit
arithmetic with sjasmplus's operator set, labels both global and local,
`MODULE` scoping, numeric temporary labels, and constants resolved on demand so
that a circular `EQU` is reported as a cycle rather than as a value that never
settles. Assembly runs to a fixpoint ([ADR-0014](adr/0014-assemble-to-a-fixpoint.md)),
which on the Z80 is cheaper than it sounds: instruction size is a function of
the parse tree alone, so only `DS`, `ALIGN` and conditional assembly can move an
address once a forward reference resolves.

### The debugger

`rkw-debug` is the debugger as a library, with no user interface in it at all
(ADR-0013). It owns breakpoints, watchpoints and port watches, the four ways of
moving that are not "run" — step, step over, step out, run to cursor — and the
emulation thread they run on.

What is armed is asked about in three tiers (ADR-0008), each reached only if the
last said maybe: a `bool`, then a bit in an 8 KB bitmap, then the map that says
what is actually there. Conditions, hit counts and ignore counts live in the
third tier, so they cost nothing until something fires. Measured, attaching the
debugger and arming breakpoints that never fire are both free.

The machine runs on its own thread in slices bounded by a T-state deadline, with
the three channels of ADR-0007: a lossy event ring that never blocks the
emulator, a stop published as a state transition because a missed breakpoint is
a broken debugger, and a lossless command queue drained once per scanline. That
last one is what makes a session reproducible — a command is applied at a
T-state rather than at a wall-clock moment, so a recorded command log replays to
the same machine, byte for byte, which is the difference between "it crashed
after I poked that byte" and a test.

Above that is the command layer, which is three parts and not two: a parser
producing command values, an executor returning structured results, and a
formatter that turns those into text. A front end can take the first two and
none of the third, which is what a DAP adapter will do (ADR-0016); the executor
never returns a `String` where it could return a value, and the parser and the
formatter are both tested without an emulator anywhere near them.

Where a program brought debug information, the debugger talks about source
rather than about addresses: `break plot.asm:12` arms one breakpoint per address
that line produced — five, for a line inside a macro used five times — a stop
shows the line and the macro invocation that reached it, `list` shows what is
around the current position, and a label is an address anywhere an address can
be written. The format and the resolution over it are their own crate
([ADR-0019](adr/0019-the-debug-info-format-is-its-own-crate.md)), because the
sidecar is a contract between two programs and a debugger that had to link the
assembler to read one could not debug a program the assembler did not build.
Source that has been edited since it was assembled is reported rather than shown
as if it ran.

### Driving it

`rkwdbg` is the terminal front end: it assembles a source file, loads it, and
gives you a gdb-style prompt.

```sh
cargo run -p rkw-cli -- program.asm
```

```text
(rkw) break $8007 if a == $43
Breakpoint 1 at $8007 if a == $43
(rkw) continue
Breakpoint 1 at $8007
=> 8007  77           LD (HL),A
   T=84 after 11 instructions
(rkw) regs
AF=4301 [-------C]  BC=02FF  DE=FFFF  HL=800F
AF'=FFFF [SZYHXPNC] BC'=FFFF  DE'=FFFF  HL'=FFFF
IX=FFFF  IY=FFFF  SP=FF00  PC=8007  WZ=8007
I=00  R=0B  IM0  IFF1=0  IFF2=0  Q=00
T=84 after 11 instructions
(rkw) x/8 hl
$800F  00 00 00 00 00 00 00 00                          |........|
```

```text
(rkw) break plot.asm:5
Breakpoint 1 at $8005 (plot.asm:5)
Breakpoint 2 at $800A (plot.asm:5)
(rkw) continue
Breakpoint 1 at $8005
plot.asm:5          ld ($4000),a
   in macro `plot`, invoked at demo.asm:9
=> 8005  32 00 40     LD ($4000),A
   T=17 after 2 instructions
(rkw) list
demo.asm
      7
      8  start:  ld hl,$4000
=>    9          plot 1
     10          plot 2
```

The commands are gdb's where gdb has them — `break`, `delete`, `step`, `next`,
`finish`, `continue`, `until`, `list`, `x`, `disas`, `info breakpoints`,
`watch`, `rwatch`, `awatch` — with `regs`, `trace`, `reset`, `poke` and `pwatch`
for the things a Z80 has that gdb's targets do not. `help` lists them all. Breakpoint
conditions are written `break $8000 if a > 1 && [hl] == 0`, memory in a
condition is `[hl]` so that parentheses can go on meaning grouping, and a flag
is `f.z` rather than `z` because `c` is already a register.

A file of commands can be replayed, which is what makes the debugger a test
harness for the assembler: assemble, run to somewhere, look at a register.

```sh
rkwdbg program.asm --batch -x check.rkw
```

`--batch` exits non-zero if any command failed, so a script is a test — and
since a label is an address, the script can say `until draw_sprite` rather than
`until $80F3` and go on meaning it after the code moves.

A source file assembled by `rkwdbg` brings its own debug information, from the
same text it assembled. A binary brings whatever `FILE.rkwdbg` sits beside it,
or whatever `--debug` names.

## Layout

```text
crates/
  rkw-asm/        macro assembler: complete
  rkw-cli/        rkwdbg: the terminal front end
  rkw-dbginfo/    the debug info format, and source resolution over it
  rkw-debug/      debugger core, emulation thread, command layer
  z80/            CPU core and disassembler
adr/              architecture decision records
docs/
  architecture.md performance measurements and analysis
  debug-info.md   the debug information format the debugger reads
tickets/
  open/           planned work
  closed/
scripts/
  fetch-testdata.sh
```

Planned: `rkw-spectrum` (ULA, memory map, tape), `rkw-dap` (Debug Adapter
Protocol front end), `rkw-gui`.

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
- [ADR-0013](adr/0013-ui-agnostic-debugger-core-cli-first.md) and
  [ADR-0016](adr/0016-dap-as-the-second-debugger-front-end.md) — the debugger is
  a library with a presentation-free command layer; a gdb-style REPL and a Debug
  Adapter Protocol server are two front ends over the same executor.

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
