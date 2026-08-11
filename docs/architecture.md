# Architecture: the hot path and everything else

The emulated CPU is a strictly linear state machine: no vectorisation and no
parallelism to be had inside it. Nor is there much of a memory layout to win
back, because the layout is not ours to choose — a Z80's address space is flat,
and the emulated program decides what it touches.

Our own structures are fixed and small:

|   |   |
| --- | --- |
| `Regs` | 34 bytes |
| `Cpu` | 48 bytes |
| Spectrum address space | 64 KB |

How that lands against a cache hierarchy depends on the target, and the
difference is not cosmetic:

| | Apple M3 (dev machine) | Typical x86-64 desktop |
| --- | --- | --- |
| L1d per core | 128 KB | 32-48 KB |
| Cache line | 128 B | 64 B (128 B effective — see below) |
| L2 | 16 MB | 256 KB - 2 MB |
| 64 KB address space | fits in L1d twice over | **does not fit in L1d**, fits in L2 |

On the development machine the entire emulated computer sits in L1. On an x86
desktop it does not, and anything outside the current working set of code,
stack and screen falls back to L2.

This changes none of the decisions below — there is nothing to reorganise
either way — but it does mean the rule *do not add large auxiliary tables to
the hot path* binds considerably harder on x86, where such a table competes
for a cache a quarter the size. **Design to the smaller machine.**

## Headroom

What performance work remains is therefore entirely about *not spending* the
headroom we have. From the `zexall` run: 46.7e9 T-states across 5.76e9
instructions, so 8.1 T-states per instruction, so a real 3.5 MHz Spectrum
retires about 432,000 instructions per second.

This core does 157 million on the dev machine — roughly **360× real time**.
That figure is indicative, not a specification; a given x86 part will differ,
plausibly by a factor of two either way. The margin is large enough that the
conclusion survives regardless:

**The only thing that can plausibly take that headroom away is work done per
instruction on behalf of the debugger.**

So the governing rule:

> The emulation loop is a hot path. It pushes compact signals outward through a
> ring buffer to another thread that does the slow, heavy work. Control input
> travels back the same way and is applied at control rate, not synchronously.

---

## Threads and channels

```text
                        events: lossy, high volume
        ┌──────────────────────────────────────────────────────┐
        │                                                       ▼
┌───────┴────────────┐                                ┌──────────────────┐
│ emulation thread   │   stop: lossless, rare         │  debug / UI      │
│                    ├───────────────────────────────►│  thread          │
│  CPU + ULA + tape  │                                │                  │
│  breakpoint bitmap │   commands: lossless, rare     │  disassembly     │
│  slice loop        │◄───────────────────────────────┤  source mapping  │
└────────────────────┘                                │  rendering       │
                                                      └──────────────────┘
```

Three channels, because they have genuinely different requirements. Collapsing
them into one would force the weakest guarantee onto all of them.

### 1. Events — lossy, high volume, never blocks

Instruction traces, memory writes in watched ranges, port activity, frame
boundaries. Single-producer single-consumer, fixed-size records, capacity a
power of two so the index wraps with a mask.

Overflow policy is **overwrite oldest** and bump a `dropped` counter. The
emulation thread must never stall because a UI thread is slow to drain. The
consumer sees the drop count and reports "12,431 records lost" rather than
silently showing a gappy trace as though it were complete.

Record size: 16 bytes, so eight to a cache line. A trace record holds the
program counter, the raw instruction bytes and its length — **not** formatted
text. Formatting allocates, and allocation on this path would cost more than
the instruction being traced.

Sizing: 64K records × 16 B = 1 MB, comfortably L2-resident.

Note on volume: full instruction tracing at real-time speed is 432K records/s
= 7 MB/s, which is nothing. At full uncapped speed it would be 2.5 GB/s, which
would dominate everything. Tracing is therefore opt-in, and when it is on the
emulator is generally running at something near real time anyway.

### 2. Stop notifications — lossless, rare

"You hit a breakpoint at 0x8034." This cannot be dropped, so it does not go
through the lossy ring at all. It is a state transition: the emulation thread
writes a `StopReason`, publishes `RunState::Paused` with a release store, and
parks. The debug thread observes it with an acquire load.

Rare enough that the mechanism can be as heavy as it likes.

### 3. Commands — lossless, rare, applied at control rate

Set/clear a breakpoint, poke memory, change run state, request a state
snapshot. SPSC ring in the other direction, drained by the emulation thread at
a control tick.

These do **not** need to be applied synchronously. A breakpoint set while the
machine is running may take effect a few thousand instructions later; that is
invisible to a person. The one case that must be exact is a breakpoint set
*while stopped*, and that falls out for free: the queue is always drained
before resuming.

---

## The slice loop

The emulation thread runs in slices bounded by a T-state deadline:

```rust
pub fn run_slice(&mut self, deadline: u64) -> SliceEnd {
    while self.bus.t < deadline {
        if self.breakpoints.armed && self.breakpoints.hit(self.cpu.regs.pc) {
            return SliceEnd::Breakpoint(self.cpu.regs.pc);
        }
        self.cpu.step(&mut self.bus);
    }
    SliceEnd::Deadline
}
```

The deadline is the earliest of: the next scheduled hardware event (interrupt,
scanline, tape edge), and the next control tick.

This is the same event-scheduler shape the ULA needs anyway, so the control
polling costs nothing structurally — it is one more entry in a schedule that
has to exist regardless.

**Control tick cadence: one scanline, 224 T-states.** That is about 27
instructions, or 64 µs of emulated time. Draining an empty command ring is a
single acquire load of the producer index, a couple of nanoseconds against
roughly 160 ns of emulated work — under 1% overhead, and it bounds command
latency at 64 µs, far below human perception.

---

## Breakpoints: three tiers

A `HashMap<u16, Breakpoint>` probe per instruction costs 20-50 ns against a
6 ns instruction. That is a 5-10× slowdown, paid whether or not any
breakpoints are set. Instead:

| Tier | Structure | Cost |
| --- | --- | --- |
| Is anything armed? | `bool` (or a count) | Predictable branch, free when debugging is off |
| Is *this* address armed? | 8 KB bitmap, 65536 bits | One bit test; touched lines follow code locality |
| What kind of breakpoint? | `HashMap<u16, Breakpoint>` | Only on a bitmap hit — conditions, hit counts, source location |

The bitmap is owned by the emulation thread and mutated only when applying
commands, so it needs no atomics of its own.

Its 8 KB is a quarter of an x86 L1d, which sounds alarming and is not: the
bitmap is touched *sparsely*, one bit per executed address, so only the lines
covering currently-executing code become resident — typically one or two. The
8 KB is address space, not working set.

Memory watchpoints get the same treatment with separate read and write bitmaps
consulted in the `Bus` path. That is roughly three bit tests per instruction
rather than one, so the `armed` guard matters more there.

---

## Contention: computed, never tabled

The obvious implementation of ULA contention is a delay-per-T-state table for
a whole frame. For the 48K machine that is 69,888 entries — 68 KB, more than
half of L1d, streamed continuously alongside the 64 KB of emulated RAM. It
would evict the machine out from under itself.

The pattern is periodic, so compute it:

```rust
const PATTERN: [u8; 8] = [6, 5, 4, 3, 2, 1, 0, 0];

fn contention(t: u64) -> u32 {
    let t = t % FRAME_TSTATES;
    if t < FIRST_CONTENDED || t >= FIRST_CONTENDED + 192 * LINE_TSTATES {
        return 0;
    }
    let into_line = (t - FIRST_CONTENDED) % LINE_TSTATES;
    if into_line >= 128 { return 0 }      // border and flyback are uncontended
    PATTERN[(into_line % 8) as usize] as u32
}
```

An 8-byte table: one cache line, permanently resident. A handful of arithmetic
ops beats a 68 KB streaming read comfortably, and only applies to addresses in
0x4000-0x7FFF.

> The constants (69888 T-states per frame, 224 per line, first contended
> T-state at 14335, the `6,5,4,3,2,1,0,0` pattern) are from memory and must be
> checked against a reference when stage 8 is implemented. The *shape* of the
> solution is the point here; the numbers are not yet load-bearing.

---

## False sharing: do not hardcode a cache line size

The producer index, consumer index and run-state word are written by different
threads. If two of them share a cache line, every write by one invalidates the
other's copy, and a ring that should be nearly free becomes a bus-traffic
generator. They must be padded apart.

The tempting number is 64, and on this development machine that is wrong —
`hw.cachelinesize` reports 128 on Apple silicon. But 64 is also wrong on
x86-64, for a different reason: although the line size *is* 64 bytes, Intel's
spatial prefetcher pulls lines in adjacent pairs, so two variables 64 bytes
apart still interfere.

So the padding is a per-target constant, and the right answer is to take it
from `crossbeam_utils::CachePadded` rather than write our own:

| Target | Padding |
| --- | --- |
| x86-64, aarch64, powerpc64 | 128 |
| s390x | 256 |
| arm, mips, riscv64, sparc | 32 |
| everything else | 64 |

If we would rather not take the dependency, the same table has to be
replicated behind `cfg(target_arch)`. What must not happen is a bare
`#[repr(align(128))]` justified by a measurement taken on one laptop.

---

## Performance builds

`[profile.release]` in the workspace manifest enables fat LTO and a single
codegen unit. This is an interpreter — one hot loop with heavy inlining across
module boundaries between `cpu`, `alu` and `bus` — so cross-crate inlining is
worth considerably more here than in typical code, at the cost of slower
builds.

For a machine-specific build:

```sh
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

That is a local optimisation only; it produces binaries that will not run on
older parts of the same architecture, so it belongs in a developer's shell and
not in the committed configuration.

---

## A property this buys us: deterministic replay

Commands are applied at deterministic points — the control tick is a T-state
deadline, not a wall-clock moment. If each applied command is stamped with the
T-state at which it took effect, then a recorded command log replays a session
exactly.

"It crashed after I poked that byte" stops being an anecdote and becomes a
reproducible test case. This is worth preserving as the design grows; it is
easy to lose by applying a command at an arbitrary moment "because it was
convenient".

---

## Consequences for code already written

`disasm::Instruction` is 64 bytes containing a `Vec<u8>` and a `String` — two
heap allocations per decode. That is fine for rendering twenty lines in a
debugger pane, and unusable on any hot path.

Before the trace ring lands, split it:

- a non-allocating core decode returning `{ addr, len, flow, undocumented }`,
  usable for stepping over calls and for walking the trace ring
- text formatting as a separate step, called only when something is about to
  be displayed

The event ring stores raw bytes and formats on the debug thread, so the
expensive half never runs at emulation rate.

---

## Summary of decisions

1. Emulation runs on its own thread in T-state-bounded slices.
2. Observation flows outward through a lossy SPSC ring of 16-byte records that
   never blocks the producer; drops are counted and reported.
3. Stops flow outward as a lossless state transition, not through that ring.
4. Control flows inward through a lossless SPSC ring, drained once per
   scanline. Latency of 64 µs is acceptable; exactness while stopped is
   guaranteed by draining before resume.
5. Breakpoints are tiered: armed flag, then bitmap, then map.
6. Contention is computed from an 8-byte table, never tabulated per frame.
7. Ring indices and run state are padded apart using a **per-target** constant,
   not a number measured on one laptop.
8. Nothing on the emulation thread allocates.
9. Sizing arguments assume the *smaller* cache hierarchy of the two targets we
   care about, not the development machine's.
