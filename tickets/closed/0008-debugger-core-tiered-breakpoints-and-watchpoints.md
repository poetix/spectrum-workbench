---
id: "0008"
title: "Debugger core: tiered breakpoints and watchpoints"
priority: high
created: 2026-08-11
closed: 2026-08-11
---

## Summary

Breakpoint and watchpoint storage and lookup, structured so the per-instruction
cost is a predictable branch and a bit test rather than a hash probe
(ADR-0008).

## Acceptance criteria

- [x] Three tiers: armed flag, 8 KB address bitmap, detail map
- [x] Execution breakpoints, with optional condition and hit count
- [x] Memory watchpoints, separate read and write bitmaps, checked in the `Bus`
      path
- [x] Port I/O watchpoints
- [x] Benchmark: with nothing armed, throughput is within a few percent of the
      bare core; with breakpoints armed but not hit, still no hash lookups
- [x] Step, step-over (uses 0007 for instruction length), step-out, run-to-
      cursor

## Notes

Step-over sets a temporary breakpoint at the address after a `Call` or `Rst`
and resumes; step-out needs the return address, which means tracking stack
depth or reading it from `SP` at the point of entry. Both use `disasm::Flow`.

## As built

A new crate, `rkw-debug`, which knows about a CPU and a bus and nothing about a
user interface (ADR-0013). `Breakpoints` holds what is armed, `Debugger` moves
the machine, and neither owns the CPU or the memory — ticket 0009 will own
those on the emulation thread and drive `resume` with a deadline where it
currently takes an instruction budget.

### What it asks of a machine

`Bus + Peek`. The bus is how the CPU runs; `Peek` (ticket 0007) is how the
debugger looks without the machine noticing — the byte a write is about to
replace, the return address on the stack, the instruction under `PC`. Keeping
them apart makes it structurally impossible for a debugger read to be mistaken
for a machine read, which matters on a bus where reading has effects.

### The tiers, and keeping three copies of one fact in step

The cost of the design is exactly what ADR-0008 said it would be: the same fact
lives in an armed flag, a bitmap and a map, and they can disagree. So nothing
sets a bit directly. Every mutation ends in a `rearm_*` function that recomputes
one address's bit, and the flag, from the things that can arm it. The tests
check the bitmap's population count against the map rather than trusting either.

Two decisions worth naming:

- **One breakpoint per address.** Two breakpoints on one address that differ
  only in condition are a confusion to display and to delete, and `Condition`
  can say `Any` for the case that wanted them.
- **The debugger's own temporaries are not in the map.** A step-over's landing
  site is a `Temporary` in a short list, so stepping over an address that
  already has a user breakpoint cannot disturb it, and "delete all breakpoints"
  has nothing to say about a step in flight.

Conditions are data rather than closures: comparisons between operands that
read registers, flags, memory, or the byte at a register pair. That way they
print back in `info breakpoints`, the command parser of 0010 can build them
without constructing code, and tests can assert on what was set. Comparisons
are unsigned, because that is what someone reading a hex dump means.

### Watchpoints, and the decorator that carries them

Memory and port watchpoints are checked by wrapping the bus for the duration of
a run. The wrapper forwards *every* method, including the machine-cycle
wrappers that have default bodies — that is load-bearing rather than
fastidious: a contended Spectrum bus implements its timing by overriding
`read_cycle` and friends, and a decorator that only overrode the raw `read`
would silently route around it, taking the contention with it.

- An instruction fetch is not a watched read. Execution is what breakpoints are
  for, and a read watchpoint that fired on the bytes of the instruction sitting
  at the watched address would be useless.
- The byte a write is replacing is read through `Peek`, never through the bus.
- An instruction that touches a watched address twice — `LDIR` does — reports
  the first hit and still runs to completion. Stopping half way through would
  leave the machine in a state the Z80 has no way to represent.
- Port watches match `port & mask == value`, because Spectrum ports are
  partially decoded and "port $FE" means 32768 addresses. Arming walks all
  65536 ports once to fill the bitmap, which is the same trade the address
  bitmaps make: work moved off the access path onto the rare mutation. The
  details are a short list rather than a map, since a masked watch would
  otherwise need tens of thousands of identical entries.

### Moving

Step-over uses a temporary breakpoint at the address after the call, as the
ticket suggested, plus a stack-pointer guard: a recursive call reaching the
same return address arrives with a lower `SP`, and stepping over one call must
not stop inside the next one down. That is `O(1)` per instruction, where
tracking call depth would have cost a decode per instruction. There is a test
for the case that distinguishes them — stepping over the recursive call two
levels down.

Two cases the ticket did not mention:

- **A repeating block instruction is its own loop.** `LDIR` does not advance
  `PC` between iterations, so stepping it appears to do nothing. Step-over runs
  it to completion, which is what "over" means for an instruction like that.
- **A `HALT` nothing can end is a stop.** `HALT` with interrupts disabled will
  never resume, so running on would only burn the budget; with interrupts
  enabled it is the machine waiting, which is not the debugger's business.

Step-out reads the return address from the top of the stack. That is a guess —
the top of the stack is whatever the routine last pushed — and it is the guess
every debugger of this kind makes; when it is wrong the run stops somewhere
unexpected or hits the budget rather than misreporting anything.

### Found by the benchmark

The tiers cost nothing measurable, as designed: attached-and-idle and
armed-but-not-firing both measure at 0.99 to 1.01 of the bare core, and the
`detail_probes` counter proves deterministically that nothing reaches the map.

The wrapper, though, cost about 40% — and not because of the check it carries.
Wrapping monomorphises the CPU against a bus that reaches the real one through
a pointer, and that indirection is the whole of it. So the run loop now decides
once, at entry, whether anything is watching the bus, and hands the CPU the
machine's own bus when nothing is. Deciding once is sound because commands are
applied between slices rather than mid-slice (ADR-0007). Watching memory now
costs what it costs; watching execution costs nothing.

Chasing that number also turned up an inlining effect that makes any quoted
throughput figure meaningless without context: a binary with exactly one call
site for `Cpu::step` at a given bus type gets the entire interpreter inlined
into that loop and runs 60% faster than the identical source with a second call
site anywhere. Both findings are written up in
[docs/architecture.md](../../docs/architecture.md).

Running does not allocate, asserted with `alloc-check` from ticket 0007 —
including the paths that stop, which are the ones that consult the map. Arming
and disarming may allocate; they happen when a person types something.
