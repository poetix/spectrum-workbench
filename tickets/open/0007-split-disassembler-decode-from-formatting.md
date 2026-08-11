---
id: "0007"
title: Split disassembler decode from formatting
priority: high
created: 2026-08-11
---

## Summary

`disasm::Instruction` allocates a `Vec<u8>` and a `String` per decode. That is
fine for rendering a debugger pane and unusable at emulation rate, which the
trace ring (0022) needs.

## Acceptance criteria

- [ ] A non-allocating decode returning `{ addr, len, flow, undocumented }`
- [ ] Text formatting as a separate call, taking the decoded form
- [ ] `Instruction` retained as the convenience composition of the two
- [ ] Existing disassembler tests pass unchanged
- [ ] Benchmark or assertion demonstrating the decode path does not allocate

## Notes

Prerequisite for 0008 (step-over needs instruction length) and 0022. Doing it
before the debugger is built avoids a retrofit through call sites.

See ADR-0007 for the no-allocation-on-the-emulation-thread rule.
