---
id: "0011"
title: Source-level debugging
priority: medium
created: 2026-08-11
closed: 2026-08-13
---

## Summary

Join the assembler's debug info (0006) to the debugger (0008, 0010) so
breakpoints can be set on source lines and the current position shown as source
rather than an address.

## Acceptance criteria

- [x] `break file.asm:42` resolves through the line table, setting a breakpoint
      at every address that line generated
- [x] Stopping displays the source line, with macro expansion context when
      inside an expansion
- [x] `list` shows source around the current position
- [x] Symbol names resolve in expressions: `break main`, `x/16 screen_buffer`
- [x] Stale debug info is detected (source newer than the binary) and warned
      about rather than silently misleading

## Notes

Depends on the debug info format from 0006 being stable.

## As built

`rkw-dbginfo`, a new crate holding the format and the resolution over it, and
depended on by both the assembler and the debugger
([ADR-0019](../../adr/0019-the-debug-info-format-is-its-own-crate.md)). The
alternative was a dependency from `rkw-debug` to `rkw-asm`, which would have
had the debugger link a macro assembler in order to read a text file — and said
the wrong thing about the snapshots, ROMs and third-party binaries it is meant
to debug next.

What stayed in `rkw-asm` is the part that is genuinely the assembler's: turning
an `Assembled` and its `SourceMap` into records. What moved is the format, its
two indexes, and everything asked of the pair of a sidecar and the text it
names.

### A name is resolved where the program is, not where the line is

The parser gained `Base::Symbol` and `Place::Source`, and resolves neither. A
symbol is a name in a `Request` exactly as `$8002` is a number in one, and what
the program calls `main` is the executor's business — which is what keeps the
grammar a pure function of the line, and what lets a DAP adapter build a
`Request` for a source line without going through text at all.

Case is kept where it matters. Command and register names are matched without
regard to it; a symbol is matched with, because the assembler's symbols are
case-sensitive and folding `Draw` into `draw` would be two labels becoming one.

A `FILE:LINE` spec is taken from the raw line rather than reassembled from
tokens, for the same reason `source` is: a path is made of characters the
command lexer takes apart, and `break ../src/my-file.asm:7` would otherwise be
a guess about where the spaces were not.

### One line is one breakpoint per address it produced

`break plot.asm:12` on a line inside a macro used five times arms five
breakpoints and reports five, because arming one of them would be a debugger
that stopped on a fifth of the executions and said nothing about the rest.

A line that produced nothing moves on to the next line that did, and says which
line it settled on. Answering "no code there" would be correct and useless: the
line the cursor is on is as likely as not to be a comment, a blank or an `EQU`.

### A stop knows where it is

`Stop` carries a resolved `Located` — file, line, the text of that line, and
the macro invocations that led there, innermost first. Resolved by the executor
rather than by the formatter, so a front end that never formats anything still
knows which line to highlight (ADR-0016).

The expansion chain is the part that earns its space. Inside a macro the line
shown is in the body, which is written once and may have been reached from
twenty places; without the invocation the reader is looking at the right text
with no idea which call produced it.

### File specs match on path components

`main.asm` matches `src/main.asm` and not `domain.asm`, and matching two files
is refused with both named rather than resolved to whichever came first. A
debugger that picked one of two `util.asm`s would be wrong half the time and
silent about it.

### Staleness is timestamps, and it is said where it misleads

A program assembled by `rkwdbg` takes its text from the same `SourceMap` it
assembled, so it cannot be stale. A sidecar read from disk is different: each
source file is timestamped against the sidecar, anything newer is named in a
warning at startup, and the flag rides along on every `Located` and `Listing`
so it is repeated at the point where the text is actually shown. The addresses
are still the binary's; it is the text beside them that has moved on.

### What is not here

Symbols in breakpoint *conditions*. A condition is evaluated on the emulation
thread against registers and memory, and its operands are a small `Copy` enum
with no allocation and no lookup in it; carrying a name through to that would
either duplicate the condition tree or put a `String` on the hot path. `break
main if a == lives` therefore parses as far as `lives` and says that is not a
register, flag or number. Every *address* position takes a symbol, which is
what the criterion asked for; conditions can be revisited if the want is real.
