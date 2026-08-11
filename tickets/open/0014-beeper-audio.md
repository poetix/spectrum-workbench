---
id: "0014"
title: Beeper audio
priority: medium
created: 2026-08-11
---

## Summary

Sound output from bit 4 of port 0xFE, resampled from the Z80 clock to the host
audio rate.

## Acceptance criteria

- [ ] Speaker state changes recorded with their T-state timestamps
- [ ] Resampling from 3.5 MHz edge times to the host sample rate, band-limited
      enough not to alias audibly
- [ ] Audio buffer feeds the host device without underruns at normal speed
- [ ] Mute and volume control
- [ ] Test: a square wave of known frequency produces the expected spectrum

## Notes

Naive point sampling of the speaker bit aliases badly on beeper music, which
is precisely the software people will want to run. Band-limited synthesis of
the edges is the difference between "works" and "sounds right".
