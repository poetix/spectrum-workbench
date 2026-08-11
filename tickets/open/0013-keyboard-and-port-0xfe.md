---
id: "0013"
title: Keyboard and port 0xFE
priority: medium
created: 2026-08-11
---

## Summary

Keyboard matrix emulation and the rest of port 0xFE.

## Acceptance criteria

- [ ] 8 half-rows by 5 bits, correctly decoded from the address lines
- [ ] Multiple simultaneous key presses, including the Caps Shift and Symbol
      Shift combinations the ROM expects
- [ ] Host key mapping is a table, not hardcoded, so layouts can be swapped
- [ ] EAR bit reads (used by tape loading, 0016)
- [ ] Test: driving the matrix produces the expected `IN` results for each
      half-row

## Notes

The ROM reads the keyboard by driving one address line low at a time; getting
the decode wrong produces a keyboard that works for single keys and fails for
shifted ones.
