---
id: "0010"
title: Debugger CLI and REPL
priority: high
created: 2026-08-11
closed: 2026-08-12
---

## Summary

The command layer and its first front end: a gdb-style REPL over the debugger
core. Deliberately UI-agnostic underneath so a GUI can drive the same commands
later (ADR-0013).

## Acceptance criteria

- [x] Commands: `break`, `delete`, `step`, `next`, `finish`, `continue`,
      `run`, `x`, `disas`, `regs`, `watch`, `trace`, `reset`
- [x] Register and flag display, including the undocumented bits, `WZ` and the
      interrupt state
- [x] Memory dump with configurable width and format
- [x] Disassembly around an address, marking the current PC
- [x] Command parsing is a library with the REPL as a thin shell over it, so
      commands are unit-testable without a terminal
- [x] Parser, executor and formatter are three separate parts: the executor
      returns structured results and never a formatted `String`, and the
      formatter is separately testable against those results (ADR-0016)
- [x] Scriptable: a file of commands can be replayed

## Notes

Scriptability is what makes the debugger usable as a test harness for the
assembler — assemble, run to a label, assert a register.

The parser/executor/formatter split is what lets a DAP adapter (0023) answer
requests from the same executor instead of parsing the REPL's output. It is
cheap to build in and expensive to retrofit, which is why it is a criterion here
rather than a concern for whenever a GUI appears.

## As built

`rkw_debug::cmd`, in three modules that are three files because they are three
jobs: `parse` (text to `Request`), `exec` (`Request` to `Outcome`, and the only
one that touches a machine), `format` (`Outcome` to text). A new `rkw-cli` crate
is the shell over them, with a `rkwdbg` binary that is argument parsing and an
exit code.

The three-way split is asserted rather than described. `tests/parse.rs` runs
without an emulator, `tests/format.rs` runs without an emulator, and
`tests/exec.rs` asserts on structured values throughout — a test that compared
formatted text would pass just as well against an executor that only had text
to give, which is the failure ADR-0016 is trying to prevent.

### The session owns the machine, and pumps it

`Session<M>` holds the `Emu` rather than talking to one across a thread
boundary. Movement still goes through the command ring, so it is stamped into
the replay log like any other input, but the session then drives the slice loop
itself and reads the machine directly when it stops.

That is right for a REPL — the person typing is not doing anything else while
the machine runs — and wrong for the DAP adapter, which has to answer `pause`
mid-run. What makes that affordable later is that the split this is arranged
around is request-to-result and not thread-to-thread: an adapter answering
requests on the emulation thread returns the same `Outcome` values, and the
parser and formatter do not care where they run.

Arming does not go through the ring. `Command` deliberately carries no ids
because ids are minted on the emulation thread and the way back is lossy; the
session is on that side of the queue, so it arms directly and returns the id it
minted. Nothing is lost from the replay log by doing so — a breakpoint decides
where a run stops, and the log records what was done at each stop.

Two commands were added to the wire set, `Reset` and `SetPc`, because both are
inputs the machine could not have worked out for itself and ticket 0026 will
want them in the log rather than applied behind its back.

### A run has a limit, because there is nothing to interrupt it with

There is no signal handling in the workspace and no thread waiting to receive
one, so `continue` on a `JR $` would take the session away for good. A movement
therefore runs at most `run_limit` T-states — a hundred million by default,
about half a minute of emulated time — and then hands back `OutOfBudget` with
the machine exactly where it got to. `--limit 0` turns it off for someone who
can interrupt the process, and a runaway script says so instead of hanging.

### What the grammar borrows and what it does not

gdb's, where gdb has one: `x/16xb $4000`, `watch`/`rwatch`/`awatch`, `delete`
with no arguments meaning everything, an empty line repeating the last command.
The differences are the machine's. `x` units are one and two bytes rather than
four and eight. A memory reference inside a condition is `[hl]`, so parentheses
can go on meaning grouping. A flag is `f.z` and never `z`, because `c` is a
register as well as a flag and a condition that quietly meant the other one
would be invisible.

`trace N` steps N instructions showing each, which is forwards; the recorded
history and its `backtrace` are ticket 0022. `disas` with no address backs up
before `PC` by a heuristic named as one — the earliest start within reach whose
decode chain lands exactly on `PC` — and gives up rather than guessing when
there is a data table in the way. Symbols are not addresses yet: `break main`
is ticket 0011, and until then it says so.

A count is where the executor has to ask a question the core will not.
`Debugger::step` will not stop a movement that has already finished, so a
breakpoint at the address stepped to reports nothing — which is right for one
step and wrong for twenty, because the remaining nineteen were asked for in
ignorance of where they were going. `step N` and `trace N` therefore check
between iterations, and the breakpoint fires properly when it fires: condition
evaluated, hit counted.

### Errors are values, and one of them is a caret

Parse failures carry a message and a column, and the formatter renders them
under the line. The executor's failures are the ones that need a machine to
notice — an id that is not armed, a count past the four-thousand-item limit, a
machine that has quit — and a `delete` with one bad id in the list changes
nothing rather than half of it.

### Scripting, and the harness argument

`source FILE` is a `Request`, but the executor does no I/O: it hands the path
back and the shell reads it, because what a file name means is a property of
where the session is running. Sourcing nests to sixteen files and says so
rather than overflowing the stack.

The shell counts errors over its life and `rkwdbg --batch` exits non-zero on
them, which is what makes a script a test. `rkw-cli` assembles a source file
through `rkw-asm` on the way in, so the loop ADR-0013 wanted — assemble, run to
somewhere, assert a register — is one command, and `tests/shell.rs` runs it.
