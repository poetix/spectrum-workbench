---
id: "0024"
title: VSCode extension
priority: low
created: 2026-08-11
---

## Summary

The editor-side half of ADR-0016: language support for the assembler syntax, and
the debugger contribution that launches `rkw-dap` (0023). Deliberately thin —
configuration and a grammar, with no debugger logic on this side of the protocol.

## Acceptance criteria

- [ ] `languages` contribution for `rkw-asm` source, with a TextMate grammar
      covering mnemonics, registers, conditions, directives, macro definition and
      invocation, labels, numeric bases and comments
- [ ] `debuggers` contribution launching `rkw-dap` in `stdio` mode
- [ ] `launch.json` schema: source or binary to run, `.rkwdbg` path, load
      address, `stopOnEntry`, with defaults that work for a single-file program
- [ ] Initial configurations and a configuration snippet, so a new project gets
      a working `launch.json` from the UI
- [ ] Disassembly view, memory view and data breakpoints enabled, matching what
      the adapter reports it supports
- [ ] Build task assembling the current file, with `rkw-asm` diagnostics matched
      into the Problems pane by a problem matcher
- [ ] Packaged as a `.vsix` by a scripted build, with the adapter binary located
      by setting rather than assumed on `PATH`
- [ ] Works in at least one other DAP client with no extension involved, as
      evidence the adapter is not VSCode-shaped — the screen pane (0025) being
      the one documented exception, as it is a custom request

## Notes

Depends on 0023. Language support and debugging are independent halves; the
grammar is useful before the adapter exists.

Diagnostics currently arrive through a problem matcher over build output. An LSP
server over `rkw-asm`'s existing diagnostics is the better answer later and is
not part of this ticket.
