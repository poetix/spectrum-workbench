---
id: "0007"
title: Split disassembler decode from formatting
priority: high
created: 2026-08-11
closed: 2026-08-11
---

## Summary

`disasm::Instruction` allocates a `Vec<u8>` and a `String` per decode. That is
fine for rendering a debugger pane and unusable at emulation rate, which the
trace ring (0022) needs.

## Acceptance criteria

- [x] A non-allocating decode returning `{ addr, len, flow, undocumented }`
- [x] Text formatting as a separate call, taking the decoded form
- [x] `Instruction` retained as the convenience composition of the two
- [x] Existing disassembler tests pass unchanged
- [x] Benchmark or assertion demonstrating the decode path does not allocate

## Notes

Prerequisite for 0008 (step-over needs instruction length) and 0022. Doing it
before the debugger is built avoids a retrofit through call sites.

See ADR-0007 for the no-allocation-on-the-emulation-thread rule.

## As built

`decode` returns `Decoded { addr, len, flow, undocumented }` and allocates
nothing. `text` / `write_text` produce the human-readable form from a
`Decoded`. `Instruction` is `Instruction::render` of the two, and
`disassemble` is unchanged in behaviour and signature, so the assembler's
round-trip test and every existing disassembler test carried over untouched.

### One table, two readings

The obvious split — a decode function and a format function — would be two
copies of the opcode table, and this module's whole premise is that a second
copy of a table is a thing that drifts. So there is still one walk, generic
over a private `Sink` trait: `str`, `hex8`, `hex16`, `dec`, `index`. Decoding
instantiates it with `Discard`, whose methods are empty and vanish at
monomorphisation; formatting instantiates it with a `fmt::Write` wrapper. The
arms read the same as before, with `format!` replaced by emission.

Two things fell out of that:

- **Reads and writes had to be separated in order.** Bytes are consumed in
  encoding order, which is not always print order — `DD CB d op` reads its
  displacement before the opcode that names it. Previously `format!` argument
  order happened to coincide; now each arm reads what it needs first and then
  emits. `decode_load` had to be split into its two memory-operand cases for
  the same reason.
- **The cursor no longer collects bytes.** `Instruction::render` re-reads `len`
  bytes from memory instead, which removes the `Vec` from the walk entirely and
  costs a handful of loads on the path that was going to allocate a `String`
  anyway. It also sidesteps a fixed-size buffer being wrong: a chain of
  `DD`/`FD` prefixes is longer than four bytes, because the CPU treats each
  prefix as an instruction in its own right and the disassembler follows it.

`Decoded` is `Copy` and holds no heap, so it is usable as a trace record.
`next_addr()` is on it because step-over is the reason 0008 wanted this.

### Proving the negative

The workspace forbids `unsafe_code`, and `forbid` cannot be lifted by an inner
`allow`, so a counting global allocator cannot live in a test of the `z80`
crate. It lives in `crates/alloc-check`, a dev-only crate that opts out of the
workspace lints for that one `unsafe impl GlobalAlloc` — described in
[docs/architecture.md](../../docs/architecture.md), because the emulation
thread and the trace ring will want it next.

The count is per thread; a process-wide counter would be measuring the test
harness's other threads. `tests/no_alloc.rs` decodes all 65,536 addresses of a
filled address space and asserts zero allocations — and first asserts that
`disassemble` *does* allocate, so that an allocator which failed to install
fails the test rather than passing everything vacuously. Checked by mutation:
adding a `vec![]` to `decode` fails it.
