---
id: "0015"
title: 48K ROM boot
priority: high
created: 2026-08-11
---

## Summary

Boot the real 48K ROM to the copyright message and a usable BASIC prompt. The
integration milestone for 0012 and 0013.

## Acceptance criteria

- [ ] ROM image loads and executes from reset
- [ ] Copyright screen appears and matches a reference image
- [ ] BASIC prompt accepts keyboard input
- [ ] A short BASIC program can be typed and run
- [ ] Regression test: boot for N frames headless and hash the framebuffer

## Notes

The ROM is not redistributable, so it is fetched or supplied by the user in
the same way as the conformance data (ADR-0005).

This is the point at which raxoft's `z80test` suite becomes runnable, which is
the only thing that will validate the `Q` latch behaviour (ADR-0003).
