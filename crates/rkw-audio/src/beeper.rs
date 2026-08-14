//! The whole signal path, from a frame's edges to samples in the ring.
//!
//! ```text
//!   edges ──► windows at 4× the device rate ──► decimate ──► speaker ──► ring
//! ```
//!
//! All of it runs on the emulation thread, once a frame, inside the same
//! `service_event` that ends the frame. That is deliberate: it keeps the edge
//! log single-buffered, because there is no other reader; it costs about a
//! quarter of a millisecond against a frame that takes twenty; and it means
//! the only thing crossing a thread boundary is finished audio, which is the
//! one thing with no timing left in it to get wrong.
//!
//! # Nothing here is machine state
//!
//! A `Beeper` holds a sample rate, a filter's coefficients and a scratch
//! buffer, none of which the emulated program can observe and none of which
//! belongs in a checkpoint (ADR-0017, ADR-0021). It is constructed once,
//! against the rate the device actually reported, and its `Vec` is allocated
//! there — arming is at person-rate and may allocate; running may not, and
//! does not.
//!
//! # Headroom
//!
//! A square wave at full scale through a resonance that lifts 6 dB clips, and
//! clipping a square wave is inaudible right up until it is horrible. So the
//! signal is scaled down before the speaker rather than after: the default
//! leaves 12 dB of room, which is enough for the bell plus the overshoot at
//! every edge, and the output is clamped anyway because a default is not a
//! proof.

use crate::edges::Levels;
use crate::filter::{Chain, Speaker};
use crate::resample::{Decimator, Rates, Windowed};
use crate::ring::SampleTx;

/// How loud a fully-on speaker is, before the speaker model.
///
/// −12 dBFS. See the module note on headroom.
pub const DEFAULT_AMPLITUDE: f32 = 0.25;

/// `MIC`'s share of the output level.
///
/// See [`Levels::amplitude`].
pub const DEFAULT_MIC_LEVEL: f32 = 0.2;

/// How many windows there are per output sample.
///
/// Four is where the returns stop being dramatic: it buys about 34 dB over not
/// oversampling at all, and eight buys under two more.
pub const DEFAULT_OVERSAMPLE: u32 = 4;

/// Everything a [`Beeper`] needs to know that the machine does not.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The machine's clock, in Hz. 3,500,000 for a 48K Spectrum.
    pub clock_hz: u64,
    /// T-states in a frame. 69,888 for a 48K Spectrum.
    pub frame_ticks: u32,
    /// What the audio device actually reported, not what it ought to be.
    pub sample_rate: u32,
    /// Which speaker the sound is coming out of.
    pub speaker: Speaker,
    /// How loud a fully-on speaker is, before the speaker model.
    pub amplitude: f32,
    /// `MIC`'s share of that.
    pub mic_level: f32,
    /// Windows per output sample.
    pub oversample: u32,
}

impl Config {
    /// A configuration for a machine of the given clock and frame length,
    /// against a device of the given rate, with everything else defaulted.
    pub fn new(clock_hz: u64, frame_ticks: u32, sample_rate: u32) -> Config {
        Config {
            clock_hz,
            frame_ticks,
            sample_rate,
            speaker: Speaker::default(),
            amplitude: DEFAULT_AMPLITUDE,
            mic_level: DEFAULT_MIC_LEVEL,
            oversample: DEFAULT_OVERSAMPLE,
        }
    }

    /// With a different speaker.
    pub fn speaker(mut self, speaker: Speaker) -> Config {
        self.speaker = speaker;
        self
    }

    /// With a different oversampling factor.
    pub fn oversample(mut self, oversample: u32) -> Config {
        self.oversample = oversample;
        self
    }

    /// With a different pre-speaker amplitude.
    pub fn amplitude(mut self, amplitude: f32) -> Config {
        self.amplitude = amplitude;
        self
    }

    /// With a different `MIC` share. Zero is a machine on which only bit 4 is
    /// wired to the amplifier.
    pub fn mic_level(mut self, mic_level: f32) -> Config {
        self.mic_level = mic_level;
        self
    }
}

/// The signal path.
pub struct Beeper {
    config: Config,
    windowed: Windowed,
    decimator: Decimator,
    chain: Chain,
    /// Windows for one frame. Sized once, at construction, to the most a frame
    /// can close; nothing in [`Beeper::render`] grows it.
    inner: Vec<f32>,
    /// Output samples for one frame, likewise. The decimator carries its phase
    /// across frames, so a frame can close one more output sample than its
    /// window count divided by the factor; the spare few are margin on an
    /// index that would otherwise panic on the emulation thread.
    samples: Vec<f32>,
}

impl Beeper {
    /// Build the path. Allocates its two scratch buffers here and nowhere
    /// else.
    pub fn new(config: Config) -> Beeper {
        let rates = Rates::new(config.clock_hz, config.sample_rate, config.oversample);
        let windows = rates.max_windows(config.frame_ticks);
        Beeper {
            config,
            windowed: Windowed::new(rates),
            decimator: Decimator::new(config.oversample),
            chain: config.speaker.chain(f64::from(config.sample_rate)),
            inner: vec![0.0; windows],
            samples: vec![0.0; windows / config.oversample as usize + 4],
        }
    }

    /// What this was built for.
    pub fn config(&self) -> Config {
        self.config
    }

    /// Turn one frame's edges into samples, and hand back what it made.
    ///
    /// `edges` and `start` are what [`EdgeLog::frame`](crate::EdgeLog::frame)
    /// returns. The slice is borrowed from the beeper's own scratch and is
    /// valid until the next call.
    pub fn render(&mut self, edges: &[u32], start: Levels) -> &[f32] {
        let windows = self.windowed.render(
            edges,
            start,
            self.config.frame_ticks,
            self.config.mic_level,
            &mut self.inner,
        );

        let gain = self.config.amplitude;
        let mut written = 0;
        for &window in &self.inner[..windows] {
            if let Some(sample) = self.decimator.push(window) {
                self.samples[written] = self.chain.process(sample * gain).clamp(-1.0, 1.0);
                written += 1;
            }
        }
        &self.samples[..written]
    }

    /// Render one frame straight into the ring, returning how many samples the
    /// ring took.
    ///
    /// A short return means the consumer is behind, which at 360× real time is
    /// the ordinary state of affairs and is how the front end knows to wait —
    /// see the note on pacing in [`ring`](crate::ring).
    pub fn render_into(&mut self, edges: &[u32], start: Levels, out: &mut SampleTx) -> usize {
        let samples = self.render(edges, start);
        out.push(samples)
    }
}

impl std::fmt::Debug for Beeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Beeper").field("config", &self.config).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::pack;
    use crate::ring;

    const CLOCK: u64 = 3_500_000;
    const FRAME: u32 = 69_888;

    const LOW: Levels = Levels {
        speaker: false,
        mic: false,
    };
    const HIGH: Levels = Levels {
        speaker: true,
        mic: false,
    };
    /// Both audio bits, which is what full scale means on this machine.
    const BOTH: Levels = Levels {
        speaker: true,
        mic: true,
    };

    fn flat(sample_rate: u32) -> Beeper {
        Beeper::new(Config::new(CLOCK, FRAME, sample_rate).speaker(Speaker::Flat))
    }

    /// A square wave of the given half-period, between `high` and silence,
    /// filling one frame.
    fn square_at(half_period: u32, high: Levels) -> Vec<u32> {
        (0..)
            .map(|half| half * half_period)
            .take_while(|&t| t < FRAME)
            .enumerate()
            .map(|(i, t)| pack(t, if i % 2 == 0 { high } else { LOW }))
            .collect()
    }

    /// A square wave on the speaker bit alone, which is what beeper music is.
    fn square(half_period: u32) -> Vec<u32> {
        square_at(half_period, HIGH)
    }

    #[test]
    fn a_frame_makes_about_a_frame_s_worth_of_samples() {
        let mut beeper = flat(48_000);
        // 69,888 T-states at 48 kHz is 958.464 samples, so frames alternate
        // between 958 and 959 and average out.
        let mut total = 0;
        for _ in 0..1_000 {
            total += beeper.render(&[], LOW).len();
        }
        // A thousand frames is 1000 × 69,888 T-states, which is exactly this
        // many samples. Not "about": the boundaries are exact.
        assert_eq!(total, 958_464);
    }

    #[test]
    fn silence_in_makes_silence_out() {
        let mut beeper = Beeper::new(Config::new(CLOCK, FRAME, 48_000));
        for _ in 0..50 {
            for &sample in beeper.render(&[], LOW) {
                assert_eq!(sample, 0.0);
            }
        }
    }

    #[test]
    fn a_held_level_is_as_silent_as_no_level_at_all() {
        // The thing that makes a beeper a beeper: leaving the speaker high is
        // not a loud sound, it is no sound. A second of it, through the real
        // speaker model, has to settle to nothing.
        let mut beeper = Beeper::new(Config::new(CLOCK, FRAME, 48_000));
        let mut last = 1.0f32;
        for frame in 0..50 {
            let edges = if frame == 0 { vec![pack(0, HIGH)] } else { vec![] };
            let start = if frame == 0 { LOW } else { HIGH };
            for &sample in beeper.render(&edges, start) {
                last = sample;
            }
        }
        assert!(last.abs() < 1e-4, "a held speaker should fall silent: {last}");
    }

    #[test]
    fn a_full_scale_square_wave_comes_out_at_the_amplitude_it_was_told_to() {
        let mut beeper = flat(48_000);
        let edges = square_at(1_750, BOTH);
        // Past the decimator's startup.
        beeper.render(&edges, LOW);
        let out = beeper.render(&edges, LOW);

        // A band-limited square overshoots at every edge — Gibbs, about nine
        // percent — so the peak sits just above the level rather than on it.
        let peak = out.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(
            (DEFAULT_AMPLITUDE..DEFAULT_AMPLITUDE * 1.15).contains(&peak),
            "peak {peak} against an amplitude of {DEFAULT_AMPLITUDE}"
        );
    }

    #[test]
    fn a_speaker_only_square_wave_leaves_the_mic_bit_s_share_behind() {
        // Beeper music drives bit 4 and leaves bit 3 alone, so it reaches four
        // fifths of what the port can do. The missing fifth is not lost
        // headroom, it is what the four-level engines have to play with.
        let mut beeper = flat(48_000);
        let edges = square(1_750);
        beeper.render(&edges, LOW);
        let peak = beeper
            .render(&edges, LOW)
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));

        let expected = DEFAULT_AMPLITUDE * (1.0 - DEFAULT_MIC_LEVEL);
        assert!(
            (expected..expected * 1.15).contains(&peak),
            "peak {peak} against an expected {expected}"
        );
    }

    #[test]
    fn nothing_clips_however_hard_the_speaker_is_driven() {
        // Both bits, square, through the piezo's resonance is the loudest
        // thing this machine can be made to do, and the default headroom
        // exists so that it does not reach the rails. Every profile, every
        // plausible rate.
        for speaker in [Speaker::Piezo, Speaker::TvSpeaker, Speaker::Flat] {
            for rate in [44_100, 48_000] {
                let mut beeper = Beeper::new(Config::new(CLOCK, FRAME, rate).speaker(speaker));
                // Sweep the half-period through the speaker's passband, so the
                // resonance is hit rather than hopefully missed.
                for half_period in [3_500, 1_750, 700, 350, 250, 100] {
                    let edges = square_at(half_period, BOTH);
                    for _ in 0..8 {
                        let peak = beeper
                            .render(&edges, LOW)
                            .iter()
                            .fold(0.0f32, |a, &b| a.max(b.abs()));
                        assert!(
                            peak < 1.0,
                            "{speaker:?} at {rate} Hz, half-period {half_period}: {peak}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_mic_bit_reaches_the_output() {
        let mut beeper = flat(48_000);
        // Bit 3 alone should be a fifth of the level bit 4 alone gives.
        let mic_only = Levels {
            speaker: false,
            mic: true,
        };
        beeper.render(&[], mic_only);
        let with_mic = beeper.render(&[], mic_only).iter().fold(0.0f32, |a, &b| a.max(b));

        let mut beeper = flat(48_000);
        beeper.render(&[], HIGH);
        let with_speaker = beeper.render(&[], HIGH).iter().fold(0.0f32, |a, &b| a.max(b));

        assert!(with_mic > 0.0, "the MIC bit should make a sound at all");
        let ratio = with_mic / with_speaker;
        assert!(
            (ratio - 0.25).abs() < 0.01,
            "MIC is a fifth and the speaker four fifths, so a quarter of it: {ratio}"
        );
    }

    #[test]
    fn rendering_into_the_ring_delivers_what_rendering_into_a_slice_would() {
        let mut into_slice = flat(48_000);
        let mut into_ring = flat(48_000);
        let (mut tx, mut rx) = ring::channel(4_096);
        let edges = square(1_750);

        for _ in 0..3 {
            let expected: Vec<f32> = into_slice.render(&edges, LOW).to_vec();
            let pushed = into_ring.render_into(&edges, LOW, &mut tx);
            assert_eq!(pushed, expected.len());

            let mut got = vec![0.0f32; expected.len()];
            assert_eq!(rx.pop(&mut got), expected.len());
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn a_full_ring_takes_what_it_can_and_says_so() {
        let mut beeper = flat(48_000);
        let (mut tx, _rx) = ring::channel(1_024);

        // A frame is about 958 samples, so the second one does not fit.
        let first = beeper.render_into(&[], LOW, &mut tx);
        let second = beeper.render_into(&[], LOW, &mut tx);
        assert!((900..1_024).contains(&first), "{first}");
        assert_eq!(first + second, 1_024, "the ring should be exactly full");
        assert!(tx.dropped() > 0);
    }
}
