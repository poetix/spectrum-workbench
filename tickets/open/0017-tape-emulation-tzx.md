---
id: "0017"
title: Tape emulation: TZX
priority: low
created: 2026-08-11
---

## Summary

TZX support, which is TAP plus a block type system covering custom loaders,
turbo loading, pure tones and direct recordings.

## Acceptance criteria

- [ ] Standard speed data blocks (0x10) and turbo blocks (0x11)
- [ ] Pure tone (0x12), pulse sequence (0x13), pure data (0x14)
- [ ] Direct recording (0x15)
- [ ] Flow control blocks: pause, jump, loop, group start/end
- [ ] Text and archive info blocks parsed and displayed
- [ ] Unknown block types are skipped using their length field rather than
      aborting the load

## Notes

Depends on 0016 for the waveform replay machinery. Lower priority: TAP covers
most of what one actually wants to run early on.
