//! The speaker, which is not a wire.
//!
//! What comes out of [`Windowed`](crate::Windowed) is what the pin did. What a
//! person in 1984 heard was that signal through a small paper cone glued into
//! a plastic case, and the difference between the two is most of what makes a
//! Spectrum sound like a Spectrum rather than like a signal generator.
//!
//! Three things happen to it:
//!
//! **The bottom falls off.** A cone that small moves no air below a few
//! hundred hertz, so the low end of a square wave simply is not there. This
//! also does the job a DC blocker would have to do anyway: a program that
//! leaves the speaker high does not leave a constant offset sitting in the
//! output eating headroom, because a constant is exactly what a high-pass
//! removes. That is not a trick, it is the same physics — a cone held at a
//! fixed displacement is as silent as one at rest.
//!
//! **There is a resonance.** The cone has one, hard, in the middle of the
//! band, and it is the whole character of the thing: the reason a beeper
//! "beeps" rather than "buzzes".
//!
//! **The top falls off.** Nothing above a few kilohertz gets out at all, which
//! is why beeper music sounds like beeper music and not like a harpsichord.
//!
//! # Two speakers, and no speaker
//!
//! [`Speaker::Piezo`] is the machine's own. [`Speaker::TvSpeaker`] is what most
//! people actually heard, because most people had the machine plugged into a
//! television. [`Speaker::Flat`] is neither: the unshaped signal, for tests
//! that want to measure the resampler rather than the cone, and for anyone who
//! would rather have the clean thing. It ships as a real option rather than a
//! test-only flag because a bypass that only exists under `cfg(test)` is a
//! path the tests exercise and the users never get.
//!
//! # Coefficients come from the device's rate, not from a constant
//!
//! Every filter here is built against the sample rate it will actually run at.
//! `cpal` hands out 44.1 kHz about as often as 48, and a peaking filter built
//! for the wrong rate puts its resonance in the wrong place — which is the one
//! bug in this design that would be inaudible in testing and obvious to
//! anybody who knew what the machine sounded like.
//!
//! # Why the state is `f64`
//!
//! The one-pole high-pass runs `y = a(y₁ + x − x₁)` with `a ≈ 0.9617` at
//! 300 Hz and 48 kHz. That is a pole close enough to the unit circle that
//! `f32` state loses audible precision over a long held note, and the state is
//! two numbers per filter. Samples stay `f32`; the arithmetic is `f64`.

/// Below this, a filter's state is rounded to zero.
///
/// An IIR filter fed silence does not reach zero, it approaches it, and on the
/// way it passes through the denormal range — where the arithmetic is done in
/// microcode and costs something like a hundred times as much. A beeper is
/// silent most of the time, so this is the common case rather than the corner
/// one. The usual fix is to set flush-to-zero in the floating-point control
/// register, which needs an intrinsic, which needs `unsafe`, which the
/// workspace forbids; so the filters flush their own state instead, which
/// costs a compare per sample and is exact for anything anyone can hear.
const DENORMAL: f64 = 1e-20;

#[inline]
fn flush(x: f64) -> f64 {
    if x.abs() < DENORMAL { 0.0 } else { x }
}

/// One-pole high-pass: `y[n] = a(y[n−1] + x[n] − x[n−1])`.
#[derive(Clone, Copy, Debug)]
pub struct HighPass {
    a: f64,
    x1: f64,
    y1: f64,
}

impl HighPass {
    pub fn new(cutoff_hz: f64, sample_rate: f64) -> HighPass {
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate;
        HighPass {
            a: rc / (rc + dt),
            x1: 0.0,
            y1: 0.0,
        }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.a * (self.y1 + x - self.x1);
        self.x1 = x;
        self.y1 = flush(y);
        self.y1
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// One-pole low-pass: `y[n] = a·x[n] + (1−a)·y[n−1]`.
#[derive(Clone, Copy, Debug)]
pub struct LowPass {
    a: f64,
    y1: f64,
}

impl LowPass {
    pub fn new(cutoff_hz: f64, sample_rate: f64) -> LowPass {
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate;
        LowPass {
            a: dt / (rc + dt),
            y1: 0.0,
        }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        self.y1 = flush(self.a * x + (1.0 - self.a) * self.y1);
        self.y1
    }

    pub fn reset(&mut self) {
        self.y1 = 0.0;
    }
}

/// A peaking biquad — a bell, which is the cone's resonance.
#[derive(Clone, Copy, Debug)]
pub struct Peak {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Peak {
    /// The standard peaking-EQ biquad: `q` sets how narrow the bell is and
    /// `gain_db` how tall.
    pub fn new(centre_hz: f64, q: f64, gain_db: f64, sample_rate: f64) -> Peak {
        let amp = 10.0f64.powf(gain_db / 40.0);
        let omega = 2.0 * std::f64::consts::PI * centre_hz / sample_rate;
        let alpha = omega.sin() / (2.0 * q);
        let a0 = 1.0 + alpha / amp;

        Peak {
            b0: (1.0 + alpha * amp) / a0,
            b1: (-2.0 * omega.cos()) / a0,
            b2: (1.0 - alpha * amp) / a0,
            a1: (-2.0 * omega.cos()) / a0,
            a2: (1.0 - alpha / amp) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = flush(y);
        self.y1
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Which speaker the sound is coming out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Speaker {
    /// The machine's own beeper: no bass at all, a hard resonance in the
    /// middle, and nothing above five kilohertz.
    #[default]
    Piezo,
    /// A 1980s television, which is what most people actually listened
    /// through: a lower resonance, gentler, and more top.
    TvSpeaker,
    /// No speaker. The unshaped signal, for measuring the resampler and for
    /// anyone who prefers it clean.
    Flat,
}

impl Speaker {
    /// High-pass cutoff, resonance (centre, Q, gain in dB) and low-pass
    /// cutoff, in hertz.
    fn shape(self) -> Option<(f64, (f64, f64, f64), f64)> {
        match self {
            Speaker::Piezo => Some((300.0, (2_500.0, 1.5, 6.0), 5_000.0)),
            Speaker::TvSpeaker => Some((200.0, (800.0, 1.0, 3.0), 8_000.0)),
            Speaker::Flat => None,
        }
    }

    /// Build the filter chain for this speaker at a given sample rate.
    pub fn chain(self, sample_rate: f64) -> Chain {
        match self.shape() {
            None => Chain { stages: None },
            Some((high, (centre, q, gain), low)) => Chain {
                stages: Some((
                    HighPass::new(high, sample_rate),
                    Peak::new(centre, q, gain, sample_rate),
                    LowPass::new(low, sample_rate),
                )),
            },
        }
    }
}

/// A speaker's filters, in the order the sound goes through them.
#[derive(Clone, Copy, Debug)]
pub struct Chain {
    stages: Option<(HighPass, Peak, LowPass)>,
}

impl Chain {
    /// One sample in, one sample out. [`Speaker::Flat`] returns it untouched,
    /// bit for bit.
    pub fn process(&mut self, x: f32) -> f32 {
        match &mut self.stages {
            None => x,
            Some((high, peak, low)) => {
                low.process(peak.process(high.process(f64::from(x)))) as f32
            }
        }
    }

    /// Forget everything, as at the start of a stream.
    pub fn reset(&mut self) {
        if let Some((high, peak, low)) = &mut self.stages {
            high.reset();
            peak.reset();
            low.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// The chain's gain at one frequency, in dB, measured by driving it with a
    /// sine and taking the peak once it has settled.
    fn gain_db(speaker: Speaker, freq: f64, sample_rate: f64) -> f64 {
        let mut chain = speaker.chain(sample_rate);
        let n = (sample_rate * 0.5) as usize;
        let settle = n / 2;
        let mut peak: f64 = 0.0;
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f64 / sample_rate).sin() as f32;
            let y = chain.process(x);
            if i > settle {
                peak = peak.max(f64::from(y).abs());
            }
        }
        20.0 * peak.max(1e-30).log10()
    }

    #[test]
    fn a_flat_speaker_is_not_a_filter_at_all() {
        let mut chain = Speaker::Flat.chain(48_000.0);
        for i in 0..1_000 {
            let x = (i as f32 * 0.37).sin();
            assert_eq!(chain.process(x), x, "flat should be bit-for-bit");
        }
    }

    #[test]
    fn a_piezo_has_its_resonance_where_the_cone_has_one() {
        // The whole point of the peaking stage: 2.5 kHz is louder than either
        // side of it, by about the 6 dB the bell is set to.
        let at_peak = gain_db(Speaker::Piezo, 2_500.0, 48_000.0);
        let below = gain_db(Speaker::Piezo, 1_000.0, 48_000.0);
        let above = gain_db(Speaker::Piezo, 4_500.0, 48_000.0);

        assert!(at_peak > below + 3.0, "{at_peak} dB vs {below} dB at 1 kHz");
        assert!(at_peak > above + 3.0, "{at_peak} dB vs {above} dB at 4.5 kHz");
        assert!(at_peak > 3.0, "the bell should lift, not just shape: {at_peak}");
    }

    #[test]
    fn a_piezo_puts_its_resonance_in_the_same_place_at_every_sample_rate() {
        // Coefficients built against a hardcoded 48 kHz would move the
        // resonance by two and a half semitones on a 44.1 kHz device, which is
        // audible to anyone who knows the machine and invisible to a test that
        // only ever runs at one rate.
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let at_peak = gain_db(Speaker::Piezo, 2_500.0, rate);
            let below = gain_db(Speaker::Piezo, 1_000.0, rate);
            let above = gain_db(Speaker::Piezo, 4_500.0, rate);
            assert!(at_peak > below + 3.0, "{rate} Hz: {at_peak} vs {below}");
            assert!(at_peak > above + 3.0, "{rate} Hz: {at_peak} vs {above}");
        }
    }

    #[test]
    fn a_speaker_moves_no_air_at_dc() {
        // Which is why nothing else needs a DC blocker: the reason a held
        // level is silent and the reason a small cone has no bass are the same
        // reason, and one filter does both.
        for speaker in [Speaker::Piezo, Speaker::TvSpeaker] {
            let mut chain = speaker.chain(48_000.0);
            let mut last = 0.0;
            for _ in 0..48_000 {
                last = chain.process(1.0);
            }
            assert!(last.abs() < 1e-3, "{speaker:?} passed DC: {last}");
        }
    }

    #[test]
    fn a_speaker_falls_away_on_both_sides_of_its_resonance() {
        // Both ends are single poles, which is six decibels an octave and
        // deliberately gentle — a cone rolls off, it does not have a brick
        // wall in it. So the claim worth asserting is the shape rather than a
        // depth: the resonance is the loudest place, and an octave into either
        // skirt costs about six more decibels.
        for (speaker, resonance) in [(Speaker::Piezo, 2_500.0), (Speaker::TvSpeaker, 800.0)] {
            let peak = gain_db(speaker, resonance, 48_000.0);
            let bottom = gain_db(speaker, 40.0, 48_000.0);
            let top = gain_db(speaker, 20_000.0, 48_000.0);

            assert!(bottom < peak - 12.0, "{speaker:?} at 40 Hz: {bottom} dB");
            assert!(top < peak - 10.0, "{speaker:?} at 20 kHz: {top} dB");

            // Six decibels an octave, at the bottom where only the high-pass
            // is doing anything.
            let octave_down = gain_db(speaker, 20.0, 48_000.0);
            let slope = bottom - octave_down;
            assert!(
                (slope - 6.0).abs() < 1.0,
                "{speaker:?} should roll off six dB an octave, not {slope}"
            );
        }
    }

    #[test]
    fn a_television_is_a_different_speaker_and_not_the_same_one_relabelled() {
        // The control for the pair: if the two profiles ever collapse into
        // each other, this is what notices.
        let piezo = gain_db(Speaker::Piezo, 800.0, 48_000.0);
        let telly = gain_db(Speaker::TvSpeaker, 800.0, 48_000.0);
        assert!(
            telly > piezo + 3.0,
            "the telly's resonance is at 800 Hz and the piezo's is not: {telly} vs {piezo}"
        );
    }

    #[test]
    fn silence_settles_to_exactly_zero_rather_than_creeping_through_denormals() {
        let mut chain = Speaker::Piezo.chain(48_000.0);
        // Kick it, then leave it alone for a quarter of a second.
        chain.process(1.0);
        for _ in 0..12_000 {
            chain.process(0.0);
        }
        assert_eq!(chain.process(0.0), 0.0);
    }
}
