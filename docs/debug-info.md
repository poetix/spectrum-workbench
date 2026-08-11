# The `.rkwdbg` debug information format

Version 1.

The assembler writes this alongside the binary; the debugger reads it to answer
two questions that a raw binary cannot: *what source produced the instruction at
this address*, and *what addresses did this line of source produce*. The second
is one-to-many — a line inside a macro used five times produced five addresses,
and "set a breakpoint on this line" means all five.

It is a text format on purpose. It is read once at load, so parsing speed is not
worth a binary layout; and being greppable and diffable is worth a great deal
when the question is "why does the debugger think line 40 is at $8003".

## Grammar

One record per line. Fields are separated by a single **tab**. Blank lines and
lines beginning with `#` are ignored.

The first line must be:

```
rkw-debug	1
```

Every integer is **decimal**. Addresses are decimal too, despite being addresses:
one rule for the whole format is worth more than the convenience of reading a
field in hex, and the listing is where a person looks anyway.

Text fields — file names and symbol names — escape a backslash as `\\`, a tab as
`\t` and a newline as `\n`. No other escapes are defined.

### Records

| Record | Fields |
| --- | --- |
| `file` | index, name |
| `expansion` | index, macro name, invoked file, line, column, defined file, line, column, parent index or `-` |
| `line` | address, length, file, line, column, expansion index or `-` |
| `symbol` | name, value, kind, file, line, column |

`file` records come first and define the indices the others use. `expansion`
records come before the `line` records that refer to them. A parent index always
refers to an expansion defined earlier in the file, so the nesting can be
rebuilt in one pass.

`kind` is one of `label`, `constant`, `variable`: an address, an `EQU`, or a
`DEFL`/`=` that may be reassigned.

Lines and columns are 1-based; columns count characters, not bytes.

### Example

```
rkw-debug	1
file	0	main.asm
file	1	lib.asm
expansion	0	plot	0	14	9	0	5	1	-
line	32768	3	0	12	9	-
line	32771	2	0	5	9	0
symbol	main	32768	label	0	11	1
symbol	width	256	constant	1	3	1
```

`plot` was invoked at main.asm:14:9 and defined at main.asm:5:1. The instruction
at address 32771 is two bytes long and came from main.asm:5:9 — inside that
expansion, which is what the trailing `0` says.

## Compatibility

The version number on the first line changes when a record's field list changes
or a field's meaning does. A reader for version 1 should reject a file whose
version it does not recognise rather than guess.

Adding a **new record type** is not a version change: readers ignore records
whose first field they do not know. That is the intended way to add optional
information — a type table, a source-level line kind — without breaking a
debugger built against version 1.
