---
id: "0014"
title: Beeper audio
priority: medium
created: 2026-08-11
closed: 2026-08-13
---

## Summary

Sound output from bit 4 of port 0xFE, resampled from the Z80 clock to the host
audio rate.

## Acceptance criteria

- [x] Speaker state changes recorded with their T-state timestamps
- [x] Resampling from 3.5 MHz edge times to the host sample rate, band-limited
      enough not to alias audibly
- [x] Audio buffer feeds the host device without underruns at normal speed
- [x] Mute and volume control
- [x] Test: a square wave of known frequency produces the expected spectrum

## Notes

Naive point sampling of the speaker bit aliases badly on beeper music, which
is precisely the software people will want to run. Band-limited synthesis of
the edges is the difference between "works" and "sounds right".

## As built

`crates/rkw-audio`, plus one field in `Ula` and a wrapping machine in
`rkw-spectrum`. See
[ADR-0021](../../adr/0021-edges-in-the-machine-and-sound-outside-it.md) for why
the split falls where it does.

### The machine records edges; everything else is a crate that cannot name it

`Ula` gains a 2048-entry `EdgeLog`: one packed `u32` per move of the audio
bits, holding a T-state offset into the frame and the level it moved to. A
write to `0xFE` that leaves both bits alone records nothing, so a
border-striping program — thousands of writes a frame — costs nothing.

It saturates rather than wrapping, because everything downstream is a forward
walk over offsets that only increase and a wrapped buffer has no such order;
and it tracks where the speaker is independently of what it had room to write
down, so a truncated frame still hands the next one the right level. Offsets
past the end of the frame are carried over rather than clamped, which matters
more than it looks: `Command::Step` never services the frame interrupt, so
single-stepping can carry the clock several whole frames past the last roll.

It is single-buffered, unlike the border, because it is drained inside the same
`service_event` that ends the frame and there is no second reader to race.

### Bit 3 is wired up too

The `MIC` output drives the same amplifier at a lower weight, so port `0xFE`
has four output levels rather than two and the beeper engines that care about
amplitude use all of them. The weighting is a parameter defaulting to
`speaker × 0.8 + mic × 0.2`; setting it to zero gives a machine on which only
bit 4 is connected. It also means 0016's tape-save path is already recorded.

### A sample is an average, not a reading

`Windowed` computes the exact mean level over each output window from the edges
inside it, which is what preserves the sub-sample edge timing that a beeper
engine is entirely made of. Then four windows per output sample, decimated
through a 133-tap Blackman-windowed sinc.

### Exact boundaries turned out to be the whole game

The first working version computed window boundaries as `i * clock / rate` in
whole T-states, and measured an alias floor 61 dB down — *worse* than the
images the oversampling existed to remove, and getting worse as oversampling
increased, which is the signature of jitter rather than of folding. Half a
T-state of boundary error is 2.7% of a window at 4× and 11% at 16×, and
reconstructing samples off a uniform grid as though they were on one is exactly
the same thing as adding noise.

Counting time in T-states divided by the window rate makes every boundary and
every edge a whole number, every window exactly the same width, and nothing
rounded anywhere. The floor went to 84 dB and started improving with
oversampling as the arithmetic says it should. The counter is `u128` because
the two rates multiplied overrun a `u64` after about a year.

### The speaker, from RKW-2

The filter profiles are ports of the `FilterProfile` chain from the RKW-2
Voltage Modular module: `Piezo` is a 300 Hz high-pass, a 2.5 kHz bell at Q 1.5
and +6 dB, and a 5 kHz low-pass; `TvSpeaker` is 200 / 800 Q 1.0 +3 dB / 8000.
The resonance is what makes a beeper beep rather than buzz.

The high-pass does the DC blocker's job as a side effect, and that is not a
trick but the same physics: a cone held at a fixed displacement is as silent as
one at rest, which is also why a small cone has no bass. One filter, both
reasons.

Coefficients are built against the rate the device actually reported. The
reference hard-codes 48 kHz, which on the 44.1 kHz device `cpal` hands out
about half the time would put the resonance two and a half semitones out — a
bug that is inaudible in a test that only runs at one rate and obvious to
anybody who knows what the machine sounded like. There is a test at three
rates.

`Flat` ships as a real profile rather than a `cfg(test)` bypass, because a
bypass that only exists under test is a path the tests exercise and the users
never get.

### Volume is applied by the consumer

Not baked into the samples. ADR-0017 is the binding reason — a volume knob is
host state and has no business crossing the command ring or being read by the
emulation thread — but it also saves a buffer's depth of latency and keeps the
ring's contents a pure function of the machine, which is what lets the spectrum
tests measure the resampler without knowing the gain.

Mute ramps over 5 ms rather than multiplying by zero, and an underrun holds the
last sample and fades it over 2 ms rather than filling with zeros. Both are
clicks otherwise, and the underrun is the common case rather than the rare one:
a debugger sitting at a breakpoint underruns continuously.

### The tests

`crates/rkw-audio/tests/spectrum.rs` is the acceptance criterion. Frequencies
that divide the clock exactly, measured over a whole number of their own
cycles, so every component sits on a bin centre and a rectangular window leaks
nothing — 1 kHz has a half-period of exactly 1750 T-states and 7 kHz a period
of exactly 500. A 7 kHz square wave's seventh harmonic is at 49 kHz and folds
to 1 kHz, where nothing legitimate lives, so that bin is a clean alias meter.
A Goertzel over six named bins rather than a transform, for the same reason the
screen tests hash rather than compare.

Two controls, because a test that only ever asserts a good number cannot tell a
working resampler from a broken analyser:

- Ten lines of naive point sampling in the same file, asserted to be *worse*
  than 25 dB. It measures 16.9 dB, which is the seventh harmonic at its full
  1/7, and it turns this ticket's own premise into a measurement.
- The 1× path asserted to *fail* the 70 dB threshold the 4× path passes.

`crates/rkw-spectrum/tests/audio.rs` is the same criterion from real Z80 code:
a `DJNZ` delay loop flipping bit 4, run through the actual CPU, ULA and slice
loop, measured out of the ring. 1001.14 Hz predicted from the loop's T-states,
1001.14 Hz measured — and a second note asserted to be in the ratio the two
loops put them in, since one number agreeing could be a constant.

`tests/no_alloc.rs` gains a third case over `Emu<AudioMachine>`. Required
rather than optional: the existing case runs `Emu<Spectrum>`, which has no
beeper in it, so every line of the new per-frame work would otherwise have sat
outside the assertion that exists to protect it.

### Found while testing

- A coarse frequency scan over a long record steps straight over bins under a
  hertz wide, and whichever component lands nearest a grid point wins. A square
  wave's third harmonic beat its own fundamental. The scan now runs coarse over
  a short prefix, where the bins are wide enough not to be missed, and fine
  over the whole record.
- `Bus`'s machine-cycle wrappers have default bodies, so `AudioMachine`
  delegates them as well as the raw accessors. `Spectrum` uses the defaults
  today, but ticket 0020 will override them for contention, and a wrapper left
  on the default would quietly run an uncontended machine with nothing pointing
  at the file responsible.

### What is not here

- No device is opened. `cpal` is ticket 0019, per ADR-0012; this ends at a
  sample ring and an `Output::fill` that never blocks and never allocates.
  `cargo run --example beep -p rkw-spectrum` writes a `.wav` in the meantime,
  so the sound can be listened to rather than only measured.
- No `AY` — that is a 128K machine and a different chip.
- The device's sample rate is fixed when the `AudioMachine` is built. Changing
  devices mid-session has nowhere to go, because a reconfiguration command must
  not enter the replay log; ADR-0021 records what to do if it ever needs to.
- 0027 will checkpoint the `Spectrum` and not the beeper, so restoring will
  click at the seam. That is the right trade and is written down there.
