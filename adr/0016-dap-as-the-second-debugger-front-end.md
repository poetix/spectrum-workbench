---
id: "ADR-0016"
title: DAP as the second debugger front end
date: 2026-08-11
status: accepted
---

## Context

ADR-0013 makes the debugger a library with a presentation-free command layer and
a gdb-style REPL over it, on the reasoning that a GUI later drives the same
commands. This decides what that GUI is, and it is earlier than it looks: the
choice constrains the shape of the command layer, and that shape is decided when
ticket 0010 is written rather than when a GUI is built.

The candidates are a bespoke GUI (ticket 0019's window, extended), a TUI, and
the Debug Adapter Protocol — the JSON-RPC protocol VSCode, Cursor, Zed, Neovim
and Emacs all speak to debug adapters.

DAP is a large win here for reasons specific to this project rather than general
enthusiasm for editor integration:

- The panes ADR-0013 admits a REPL is worse at — live registers, a memory
  window, disassembly around PC, a source view following execution — are exactly
  what a DAP client already renders. They are the parts that would otherwise be
  written twice, once badly.
- The `.rkwdbg` format (docs/debug-info.md) was designed around a source line
  mapping to *many* addresses, which is the same shape DAP's `setBreakpoints`
  has: one source breakpoint, N verified locations.
- The intended author of the source is a model working with a human (ADR-0015).
  The human's review loop is an editor. A debugger reachable from the same editor
  the source is read in is worth more here than in a project written entirely by
  hand.
- The macro expansion trail in `.rkwdbg` has no natural display in a REPL beyond
  printing it. It maps directly onto DAP stack frames, which every client draws.

## Decision

The second front end is a DAP adapter, `rkw-dap`, and a thin VSCode extension
that launches it and supplies language support for the assembler syntax.

### The adapter is built against the query layer, not the REPL

DAP requests are answered by the same command layer the REPL uses, consuming its
**data** rather than its formatted text. Ticket 0010's command layer is
therefore three parts and not two:

1. a parser, producing command values,
2. an executor, returning structured results,
3. a formatter, turning those results into terminal text.

The REPL is 1 → 2 → 3. The adapter is 1 → 2, then serialises to JSON. Nothing
scrapes the formatter's output, and the executor never returns a `String` where
it could return a value.

This is the whole of the cost of the decision, it is paid in ticket 0010, and it
is expensive to retrofit — a formatter-shaped executor forces the adapter to
parse its own debugger's output.

The one deliberate exception is DAP's `evaluate` with `context: "repl"`, which
goes through all three parts. The Debug Console then *is* the gdb-style REPL,
inside the editor, for the cost of one request handler. Everything ADR-0013's
scriptability argument buys stays available to someone who prefers typing.

### Addresses are opaque strings, chosen for banking now

DAP `memoryReference` values are opaque strings by specification. They are
written as `"0x8000"` while memory is flat and `"bank:0x0000"` once 48K stops
being the only target (ticket 0012 onwards), and the adapter treats them as
opaque from the first commit rather than as a `u16` it formats.

Deciding this before there is a paged machine costs nothing. Deciding it after
means every reference already handed to a client is ambiguous.

### Macro expansions are stack frames

A Z80 has no frame pointer and no reliable way to walk a call stack — `SP` walking
is a heuristic that `PUSH`/`RET` idioms defeat, and this is a machine where those
idioms are normal. So the adapter does not present a call stack it cannot
justify.

What it does present is exact: the `expansion` parent chain from `.rkwdbg` is a
real nesting, and it becomes the frame list. Stopping inside a nested macro shows
the innermost expansion, its parent, and the invocation site, each with a source
location the client can open. `SP`-derived frames may be added later as a
separate, explicitly labelled thing; they are not what the frame list is for.

### The screen is a debugger view, and DAP has no picture

Registers and memory regions are already DAP surfaces: `variables` and
`readMemory` with the client's own hex editor. The display file is not. DAP has
no image response and no pane to put one in, and the use case that motivates
this whole front end — step a rendering routine and watch pixels land in the
display file one at a time — is exactly the thing it cannot express.

So the screen pane is a **custom request answered by the adapter and drawn by
the extension**, with three constraints:

**The adapter sends pixels, not display-file bytes.** The Spectrum's screen
layout — thirds, the scrambled `y7y6 y2y1y0 y5y4y3` row order, an attribute
plane at a different address with its own 8×8 granularity — is knowledge that
belongs in `rkw-spectrum` (ticket 0012), where it is already needed and already
tested against a known image. Sending raw bytes and decoding them in TypeScript
puts a second, untested copy of that layout in the front end, where it will drift
and where the drift shows up as a picture that is subtly wrong rather than as a
failing test.

**The query takes a base address.** A rendering routine under development is
usually drawing to a back buffer, not to $4000, and on a 128K machine the shadow
screen is in bank 7. A screen view that can only show $4000 is a screen view that
cannot see the thing being debugged. Default to $4000 and accept any address, at
which point "screenshot" and "show me this buffer as pixels" are one feature.

**What changed matters more than what is there.** Between two steps of a
rendering routine, 6910 of the display file's 6912 bytes are identical. A pane
that redraws the whole screen answers "what does it look like" when the question
being asked is "what did that instruction just do". The response therefore
carries the writes made since the previous stop, and the pane marks them.

That delta cannot be recovered by diffing two snapshots, for two reasons that are
the same reason. Stopping happens *between* instructions (an instruction that
touches several watched addresses runs to completion — see `DebugBus`), so one
step of `LDIR` or a `PUSH`-based blit is many writes seen as one diff, with their
order and their authors lost. A **write journal** — address, value, PC, T-state,
recorded on write — keeps what a diff throws away, and is what makes single
stepping through a blit legible.

The journal arms over an address range through the same bitmap the watchpoints
use (ADR-0008): the hot path already tests that bit, so a journal that is not
armed costs nothing, and one that is armed records instead of stopping. It is a
fixed-size ring with overwrite-oldest and a drop count, for the same reasons as
the trace ring (ADR-0007), and it allocates nothing on the emulation thread.

The wire format is indexed pixels plus a palette rather than an encoded image,
so no image codec enters the workspace and the extension draws to a canvas. Two
front ends consuming that same response — the pane, and a `screen` command in the
REPL rendering half-block characters to a terminal — is the cheapest available
proof that the executor is returning data and not text.

Rendering is deterministic: the flash phase is fixed rather than taken from the
frame counter, so stopping twice at the same place gives the same picture.

### It is a separate crate, and it may have dependencies

`rkw-dap` is a binary crate depending on `rkw-debug`, `rkw-asm` and `serde_json`.
The protocol's types are hand-written rather than taken from a crate: the subset
needed is about twenty request types, the protocol is stable, and the workspace's
freedom from dependencies in the emulation path is worth more than the saving.

Nothing in `z80`, `rkw-debug` or `rkw-asm` gains a dependency, and the adapter is
not in the emulation thread's path.

### Ticket 0009 is a prerequisite

DAP clients expect to send `pause` while the target runs, and expect `stopped` to
arrive as an event rather than to be discovered by polling. Both need ADR-0007's
inbound command ring and stop notification, which is ticket 0009.

An adapter built before that would run the CPU on the request-handling thread and
be unable to answer anything during `continue` — which in a DAP client is not a
degraded debugger but a hung editor.

## Consequences

**Positive:**

- The continuous displays ADR-0013 gives up are recovered without writing a
  widget. Registers, memory, disassembly, breakpoint management, stepping,
  watchpoints and a source view are client-side.
- Every DAP client benefits, not just VSCode.
- The forcing function on ticket 0010 is good independent of DAP: an executor
  returning data is the testable one, and the formatter becomes separately
  testable too.
- Stale-debug-info detection (ticket 0011) has a natural presentation — `Source`
  checksums plus unverified breakpoints — rather than a warning line that scrolls
  away.
- Ticket 0019's window keeps a smaller job: it presents a running machine, and
  is not also a debugger UI.

**Negative:**

- DAP's model is a source-level debugger for a machine with threads, frames and
  variables. Some of it fits badly and is answered with a polite fiction: one
  thread, frames that are macro expansions, "variables" that are registers.
  Those fictions need to be consistent, which is a design cost paid in the
  adapter.
- Debugging the adapter means debugging a protocol conversation. A recorded
  session replayable against the adapter is worth building early, and is the
  same shape as ADR-0013's replayable command file.
- Editor integration invites feature requests shaped like an IDE. The boundary
  is that the adapter exposes the debugger's capabilities and does not grow
  capabilities of its own.
- The screen pane is the one part that is not portable across DAP clients,
  because it is a custom request. Other clients lose the picture and keep
  everything else, and the REPL's `screen` command means the capability is never
  only reachable from one editor.

## Follow-on

Language support for the assembler — syntax highlighting, and later diagnostics
from `rkw-asm`'s existing diagnostic machinery over LSP — ships in the same
extension but is a separate concern from debugging, and neither blocks the other.
