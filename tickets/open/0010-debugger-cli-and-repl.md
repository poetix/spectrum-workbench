---
id: "0010"
title: Debugger CLI and REPL
priority: high
created: 2026-08-11
---

## Summary

The command layer and its first front end: a gdb-style REPL over the debugger
core. Deliberately UI-agnostic underneath so a GUI can drive the same commands
later (ADR-0013).

## Acceptance criteria

- [ ] Commands: `break`, `delete`, `step`, `next`, `finish`, `continue`,
      `run`, `x`, `disas`, `regs`, `watch`, `trace`, `reset`
- [ ] Register and flag display, including the undocumented bits, `WZ` and the
      interrupt state
- [ ] Memory dump with configurable width and format
- [ ] Disassembly around an address, marking the current PC
- [ ] Command parsing is a library with the REPL as a thin shell over it, so
      commands are unit-testable without a terminal
- [ ] Parser, executor and formatter are three separate parts: the executor
      returns structured results and never a formatted `String`, and the
      formatter is separately testable against those results (ADR-0016)
- [ ] Scriptable: a file of commands can be replayed

## Notes

Scriptability is what makes the debugger usable as a test harness for the
assembler — assemble, run to a label, assert a register.

The parser/executor/formatter split is what lets a DAP adapter (0023) answer
requests from the same executor instead of parsing the REPL's output. It is
cheap to build in and expensive to retrofit, which is why it is a criterion here
rather than a concern for whenever a GUI appears.
