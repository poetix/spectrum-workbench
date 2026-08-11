---
id: "0018"
title: Snapshots: SNA and Z80 formats
priority: medium
created: 2026-08-11
---

## Summary

Save and restore full machine state, which is both a user feature and the
fastest way to get to a specific point when debugging.

## Acceptance criteria

- [ ] SNA 48K read and write
- [ ] Z80 format versions 1, 2 and 3, including the compressed variants
- [ ] Round-trip: save, restore, and continue with identical subsequent
      behaviour
- [ ] Internal state not in the file formats (`WZ`, `Q`, pending `EI`) is
      handled explicitly, with the choice documented
- [ ] Snapshot on breakpoint hit, for post-mortem inspection

## Notes

The SNA format loses information — notably it restores `PC` from the stack —
so a save/restore cycle is not perfectly transparent. Document what is lost
rather than pretending otherwise.
