---
id: "0035"
title: CRT presentation: persistence, geometry and what 50 Hz means
priority: low
created: 2026-08-14
---

## Summary

0033 and 0034 make the machine emit what the ULA actually put out, T-state by
T-state. This is the other end of the same wire: what a CRT did with it. Two
things a frontend that blits a framebuffer at 50 Hz gets wrong, and one it has
to decide deliberately.

**Persistence.** The phosphor does not go out between frames. A large family of
Spectrum effects — gigascreen and every flicker-mixed shade — exists because
two frames alternating at 50 Hz read as one picture with colours the palette
does not contain. Blitting frames independently renders those as flicker, which
is what they look like on a bad emulator and not what they looked like on a
television.

**Geometry.** 352x296 is the whole non-blanking area, and a Spectrum pixel is
not square on a 4:3 display. Both are decisions the frontend currently makes by
default rather than on purpose.

**Not interlace.** The Spectrum's output is 50 Hz progressive, 312 lines, every
frame the same lines. There is no field parity to model and nothing should grow
one.

## Acceptance criteria

**Persistence**

- [ ] Frame blending as a pure function of a sequence of framebuffers and a
      decay, returning a framebuffer: no machine, no clock, no host state
- [ ] Decay expressed in milliseconds of phosphor persistence and converted
      against the 20 ms frame, so the parameter means something physical rather
      than being a blend weight someone tuned
- [ ] Off by default is a decision to make and record either way, but the switch
      is the frontend's, not the machine's (ADR-0021: host state and wall-clock
      time stay outside)
- [ ] Test: two frames differing only in one attribute, alternating, blend to a
      stable intermediate colour rather than to either input
- [ ] Test: a still picture blends to itself exactly, so persistence costs no
      fidelity where there is no motion

**Geometry**

- [ ] Pixel aspect ratio applied on presentation, with the correction named and
      derived rather than eyeballed
- [ ] Overscan crop options: the full 352x296, the conventional visible area,
      and display-only 256x192, all from the same framebuffer
- [ ] Integer scaling honoured within whichever crop is chosen (0019)

**Pacing**

- [ ] The frontend presents at the machine's 50 Hz and not at the host's
      refresh, or states what it does when those differ. A 60 Hz host showing
      50 Hz content either drops or repeats, and which one it does is visible in
      exactly the software this ticket is about
- [ ] Audio-clocked pacing (0019) remains the master clock; this ticket does not
      introduce a second one

## Notes

The order matters: persistence over a picture that is wrong per scanline just
smears the wrong picture. 0033 and 0034 first.

Everything here belongs outside `rkw-spectrum` and the crate boundary should
enforce it, the same way `rkw-audio` cannot name a `Spectrum` (ADR-0021). The
machine's output is what the ULA emitted; a phosphor is a property of the
display someone plugged it into, and a decay read from a settings file inside
the machine would break replay determinism (ADR-0017) for a cosmetic.

That parallel is exact and worth leaning on: the beeper's edges are the
machine's, the speaker cone is `rkw-audio`'s. The ULA's output is the machine's,
the phosphor is the frontend's. Both are the physical device on the far side of
a signal, and modelling it is most of what makes an emulator feel like the
hardware rather than like a signal generator.

Scanline masks, aperture grille simulation and bloom are a different thing —
they are an aesthetic over the picture rather than a property of the signal, and
they belong in a shader in 0019 if anywhere. Persistence is not in that
category: it changes which colours the software can produce.
