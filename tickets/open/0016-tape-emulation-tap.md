---
id: "0016"
title: Tape emulation: TAP
priority: medium
created: 2026-08-11
---

## Summary

Load and save in TAP format, both through ROM routine emulation (fast) and by
replaying the audio waveform (accurate).

## Acceptance criteria

- [ ] TAP parsing: block structure, flag and checksum bytes
- [ ] Waveform replay driving the EAR bit with correct pulse timings
- [ ] The ROM loader loads a real tape image successfully
- [ ] Saving produces a TAP a real Spectrum would load
- [ ] Optional accelerated loading by trapping the ROM load routine
- [ ] Test: round-trip save then load recovers identical data

## Notes

Waveform replay must come first even though trapping is faster, because a lot
of software uses custom loaders that never call the ROM routine.
