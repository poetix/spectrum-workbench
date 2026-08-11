---
id: "ADR-0001"
title: Decode opcodes by octal decomposition
date: 2026-08-11
status: accepted
---

## Context

The Z80 has 256 unprefixed opcodes plus four prefix pages (`CB`, `ED`, `DD`,
`FD`) and two doubly-prefixed pages (`DD CB`, `FD CB`). Written as flat match
statements that is well over a thousand arms, most of them near-duplicates.

It is also incomplete in practice. A large part of the instruction set is
undocumented — `SLL`, the `IXH`/`IXL` register halves, the `DD CB` forms that
write to both memory and a register, duplicate `IM` and `NEG` encodings — and
enumerating those by hand means finding out about each one individually.

The instruction set is not arbitrary. Every opcode byte decomposes as

```text
  7 6   5 4 3   2 1 0
 +-----+-------+-------+
 |  x  |   y   |   z   |     p = y >> 1,  q = y & 1
 +-----+-------+-------+
```

and the encoding is regular in those fields.

## Decision

Decode on `x`, `y`, `z`, `p`, `q` rather than on the opcode byte, with small
lookup tables for the register and register-pair slots. The `DD`/`FD` prefixes
are modelled as an `Index` parameter threaded through decoding, which rewrites
the `HL` slot to `IX` or `IY` and the `(HL)` slot to `(IX+d)`.

The disassembler mirrors the same decomposition arm for arm, so the two are
the same table read in opposite directions.

## Consequences

**Positive:**
- The whole unprefixed set is about a hundred lines rather than a thousand.
- Undocumented encodings fall out of the structure. `SLL` is simply rotation
  index 6; `IXH` is what the `H` slot means under a `DD` prefix. They were
  implemented without being individually enumerated, and `zexall` passed them
  first time.
- The disassembler and the CPU cannot drift far apart, because a change to the
  shape of one is visibly a change to the shape of the other.

**Negative:**
- Less immediately greppable. "Where is `LD A,(HL)` implemented?" has no direct
  answer; you have to know it is `x=1, y=7, z=6`.
- The irregular instructions (`x=0, z=2` in particular, which is eight
  unrelated load forms) fit the scheme badly and need their own sub-match.

**Mitigations:**
- The decomposition is documented at the top of `cpu.rs` with the bit diagram.
- A cross-check test walks all 1536 encodings and asserts the disassembler's
  byte count matches what the CPU consumed, which catches any divergence.
