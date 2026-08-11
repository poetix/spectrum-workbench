---
id: "ADR-0013"
title: UI-agnostic debugger core, CLI first
date: 2026-08-11
status: accepted
---

## Context

The project exists for its debugging affordances, so the debugger is not an
afterthought to be bolted onto whatever UI appears. The question is what it
should be built against.

A GUI debugger is the best eventual experience and the most upfront work, and
building it first would mean the debugger's capabilities are defined by what is
convenient to render.

## Decision

Build the debugger as a library with a command layer that knows nothing about
presentation, and give it a gdb-style REPL as its first front end. A GUI later
drives the same command layer.

Commands are parsed by a library with the REPL as a thin shell over it, so
command behaviour is unit-testable without a terminal, and a file of commands
can be replayed.

## Consequences

**Positive:**
- The debugger's capabilities are decided by what is useful, not by what is
  easy to draw.
- Everything is testable without a UI harness.
- Scriptability makes the debugger usable as a test harness for the assembler:
  assemble, run to a label, assert a register. That is worth having well
  before any GUI exists.
- No dependency on the frontend decision (ADR-0012), so the two can proceed
  independently.

**Negative:**
- A REPL is a worse day-to-day experience than live register and memory panes,
  and that is the experience we live with for some time.
- Some state is genuinely better shown continuously than queried — the whole
  register file, a memory window, a disassembly around PC. A REPL makes those
  a repeated command rather than an ambient display.

**Mitigation:** ticket 0010 includes a disassembly-around-PC and register dump
sized to be re-issued cheaply, and the command layer is designed so a TUI or
GUI can poll the same queries on a timer.
