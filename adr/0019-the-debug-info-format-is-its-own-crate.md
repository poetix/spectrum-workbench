---
id: "ADR-0019"
title: The debug info format is its own crate
date: 2026-08-13
status: accepted
---

## Context

Ticket 0011 joins the assembler's debug information to the debugger: `break
file.asm:42`, a stop shown as a source line, symbols as addresses. The
assembler writes the `.rkwdbg` sidecar; the debugger has to read it. The type
that describes it, and the parser for it, were in `rkw-asm`.

Three ways to give the debugger access to it:

1. `rkw-debug` depends on `rkw-asm`. Nothing moves, and the debugger core links
   an entire macro assembler in order to read a text file it did not write.
2. The resolution lives in `rkw-cli`, and `rkw-debug` stays source-blind. The
   DAP adapter (ticket 0023) then reimplements the same resolution, and a stop
   cannot carry the line it stopped on because the executor does not know it.
3. The format moves to a crate both depend on and neither owns.

The debugger will not always be looking at something this assembler produced. A
`.sna` snapshot (ticket 0018), the 48K ROM (0015) and a binary built by
sjasmplus are all programs it is meant to debug, and none of them arrives with
this assembler anywhere in the picture. A dependency edge from the debugger to
the assembler says the opposite.

## Decision

`rkw-dbginfo` holds the format: the records, the two indexes over them, the
text reader and writer, and the resolution asked of the pair of debug info and
source text — which file a spec names, which addresses a line produced, what a
symbol is worth, what source produced an address, and whether the text on disk
has moved on since the sidecar was written.

`rkw-asm` depends on it and keeps only the part that is the assembler's: turning
an `Assembled` and its `SourceMap` into records. `rkw-debug` depends on it and
re-exports what a front end needs, so a front end still depends on one crate.

## Consequences

**Positive:**
- The debugger does not depend on the assembler, and a program from anywhere
  else can bring debug information the same way.
- Resolution is shared by the REPL and the DAP adapter rather than written
  twice, and a `Stop` can carry the source line because the executor can
  resolve one.
- The format's tests are written the way a reader meets it — against text from
  a program it did not run — instead of against something just assembled.

**Negative:**
- A fifth crate, and a change to a record's fields now touches two crates
  rather than one.
- `rkw_asm::DebugInfo` is a re-export, so its documentation lives elsewhere
  than the crate it is usually reached through.
