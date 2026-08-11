---
id: "0006"
title: Assembler listing and debug info
priority: medium
created: 2026-08-11
closed: 2026-08-11
---

## Summary

Human-readable listing output, and the machine-readable debug information the
debugger needs to map addresses back to source.

## Acceptance criteria

- [x] Listing shows address, encoded bytes, source line, and macro expansion
      nesting
- [x] Symbol table dump, sorted by name and by address
- [x] Debug info sidecar: address to (file, line, column) line table, symbol
      table with types where known, and macro expansion records
- [x] Line table is queryable in both directions — address to source, and
      source to the addresses a line generated
- [x] Format is stable and documented, since the debugger depends on it

## Notes

Ticket 0004 deferred its sidecar criterion here, and left the raw material:
`Assembled::lines` is one `LineRecord` per statement that emitted bytes, with
the span (so file, line and column) plus address and length. What is missing is
the format, the reverse index, and the macro expansion records.

Source-to-address must be one-to-many: a line inside a macro used five times
generates five addresses, and "set a breakpoint on this line" should set five.

## As built

`debug.rs` (the sidecar) and `listing.rs` (the human-facing output), with the
format written down in [docs/debug-info.md](../../docs/debug-info.md).

### The format, and why it is text

Tab-separated records, one per line, with a version on the first. It is read
once when the debugger loads a program, so a binary layout would buy nothing
measurable; being greppable and diffable buys a great deal the first time
something disagrees about which line an address belongs to.

Two decisions worth their own note:

- **All integers are decimal, including addresses.** One rule for the whole
  format is worth more than the convenience of reading one field in hex, and
  the listing is where a person looks anyway.
- **Unknown record types are ignored rather than refused, but an unknown
  version is refused.** That makes adding a record type — a data-type table, a
  source-level line kind — a compatible change, while a change to what the
  existing records *mean* is not. Both halves have a test.

"Types where known" is the kind of symbol — label, constant, or variable — since
there is no type system here to know anything more than that.

### Both directions, and only one copy of the information

Address to source is a binary search over the line table sorted by address.
Source to addresses is a map built from that same table, rebuilt when the file
is parsed rather than written into it: two copies of one relation are two things
that can disagree, and the derived one is cheap.

The one-to-many direction has the test the ticket asked for — a `DB` inside a
macro used three times is one line of source and three addresses.

### The listing

Files appear in the order they were registered — the root first, then each
`INCLUDE`d file whole — rather than being spliced in at the point of inclusion.
That keeps a file's lines next to each other, which is what someone reading a
listing is usually following. It is a choice rather than a limitation, and the
alternative is a small change if it turns out to read worse.

Statements produced by a macro are printed under the line that invoked them,
with one `>` per level of nesting, showing the macro body's own source text —
which is where the reader has to look to understand the bytes, and is otherwise
nowhere near them.

### Found while testing

The writer emitted ten fields for an `expansion` record and the reader demanded
eleven, so nothing with a macro in it could be read back. Caught immediately by
the round-trip test, which is the entire reason to write one rather than to
eyeball the output: a format with a writer and no reader is a format nobody has
checked.
