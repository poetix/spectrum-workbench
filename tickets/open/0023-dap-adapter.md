---
id: "0023"
title: DAP adapter
priority: medium
created: 2026-08-11
---

## Summary

`rkw-dap`: a Debug Adapter Protocol server over the debugger's command layer, so
any DAP client — VSCode, Zed, Neovim — drives the debugger with its own
register, memory, disassembly and source panes (ADR-0016).

## Acceptance criteria

- [ ] JSON-RPC framing over stdio: `Content-Length` headers, request/response
      correlation, events
- [ ] Lifecycle: `initialize` (with an honest capabilities reply),
      `launch`, `configurationDone`, `disconnect`, `terminate`
- [ ] Execution: `continue`, `next`, `stepIn`, `stepOut`, `pause`, and `stopped`
      events carrying a reason — breakpoint, step, watchpoint, entry, pause
- [ ] `pause` answered while the target is running, via the command ring (0009);
      the request thread never runs the CPU
- [ ] `setBreakpoints` resolves a source line through the line table to every
      address it generated, reporting each as a verified location, and reports
      lines that generated nothing as unverified with a reason
- [ ] `setInstructionBreakpoints` and `setDataBreakpoints` over the core's exec
      breakpoints and memory watchpoints
- [ ] `supportsStepBack` reported honestly — only when the checkpoint ring is
      configured — with `stepBack` and `reverseContinue` over 0028, and history
      exhaustion surfaced as a `stopped` reason rather than a silent no-op
- [ ] Breakpoint `condition` parsed by the same parser the REPL uses, into
      `rkw_debug::Condition`; `hitCondition` onto the ignore count
- [ ] `stackTrace` built from the `expansion` parent chain — innermost expansion
      outwards to the invocation site — with `instructionPointerReference` on
      the top frame
- [ ] `scopes`/`variables`: registers including the undocumented bits, `WZ`,
      shadow set, `SP`/`PC`, and interrupt state; flags as children of `AF`
- [ ] `setVariable` writes registers back
- [ ] `readMemory`/`writeMemory` and `disassemble`, the latter over
      `z80::disasm`'s decoded form rather than its rendered text
- [ ] `evaluate`: `context: "repl"` goes through the formatter and behaves as
      the gdb REPL; other contexts return structured values
- [ ] `memoryReference` treated as an opaque string throughout, so paging can
      change its form without changing the surface (ADR-0016)
- [ ] Stale debug info surfaced as `Source` checksum mismatch plus unverified
      breakpoints, not a silent wrong answer
- [ ] Custom screen request, and the writes-since-last-stop it carries, per 0025
- [ ] A recorded protocol session replays against the adapter as a test, with no
      terminal and no client

## Notes

Depends on 0009 (command ring and stop notification), 0010 (command layer) and
0011 (source-level resolution). The adapter should add no debugger capability of
its own: anything it can do, the REPL can do.

Reverse execution (0028) is not a prerequisite — the capability is advertised or
not, and every DAP client handles its absence. It is listed here because
`initialize` must report it correctly from the start: a client shown a step-back
button that does nothing is worse than one that never had it.

The fictions DAP requires — one thread, expansion frames, registers as variables
— are listed in ADR-0016 and should be applied consistently rather than
case-by-case.
