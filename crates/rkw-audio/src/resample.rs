//! From edges on a 3.5 MHz clock to samples at whatever rate the device runs.
//!
//! # An output sample is a window, not an instant
//!
//! The obvious way to do this is to ask, at each sample instant, what the
//! speaker is doing — and it is the wrong way, because the speaker moves
//! seventy times between one sample and the next and the answer throws away
//! sixty-nine of those. What comes out is the beeper's harmonics folded back
//! into the audible band at full strength, which is the grinding, whistling
//! quality that made emulated beeper music sound wrong for a decade.
//!
//! So a sample here is not a reading but an average: the window it covers is
//! `[boundary(i), boundary(i+1))`, and its value is the mean level over that
//! window, computed exactly from the edges that fall inside it. An edge two
//! thirds of the way through a window moves the sample by exactly a third of
//! the step, which is how sub-sample edge timing — the whole of what a beeper
//! engine is doing — survives the trip.
//!
//! This is a box filter, which is a first-order thing and not the last word:
//! it puts the worst surviving image around 50 dB down. [`Decimator`] takes it
//! the rest of the way by running the windows four times as fast and filtering
//! properly on the way down.
//!
//! # Boundaries are exact, and the sample index never resets
//!
//! A frame is 69,888 T-states, which at 48 kHz is 958.464 samples. Rounding
//! that per frame drifts by about 23 samples a second — half a millisecond
//! every second, which is a device that runs dry once a minute. Multiplying a
//! `f64` phase by 50 a second for an hour is not much better. So the window
//! index is absolute and counts from the start of the stream, and there is no
//! accumulated quantity for an error to collect in.
//!
//! Its position, though, has to be exact and not merely unbiased. Rounding
//! each boundary to the nearest whole T-state moves each sample off the
//! uniform grid by up to half a T-state, and reconstructing jittered samples
//! as though they were uniform is the same thing as adding noise: measured
//! against a 7 kHz square wave it puts a floor about 61 dB down, which is
//! *worse* than the images the oversampling is there to remove, and it gets
//! worse the faster the windows run — at 16× a window is only 4.6 T-states
//! long and a half-T-state of jitter is 11% of it.
//!
//! So time is counted here in units of one T-state divided by the window rate.
//! In those units a window is exactly `clock_hz` long, every T-state is
//! exactly `inner_rate` of them, and both the boundaries and the edges land on
//! whole numbers. Nothing is rounded, every window is exactly as wide as every
//! other, and the floor drops to where the arithmetic says it should be. The
//! counter is `u128` because the product of the two rates overruns a `u64`
//! after about a year of continuous emulation, which is not a limit worth
//! having.
//!
//! A window that straddles a frame boundary is ordinary, and is why
//! [`Windowed`] carries a part-accumulated sample between calls rather than
//! rounding the frame's edges to whole samples.

use std::f64::consts::PI;

use crate::edges::{Levels, levels_of, tick_of};

/// The three rates a beeper works between: the machine's clock, the device's
/// sample rate, and how much faster than the device the windows run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    clock_hz: u64,
    sample_rate: u32,
    oversample: u32,
}

impl Rates {
    /// # Panics
    ///
    /// If any rate is zero, or if the windows would run at or above the
    /// machine's clock — at which point a window is less than one T-state and
    /// there is nothing left to average.
    pub fn new(clock_hz: u64, sample_rate: u32, oversample: u32) -> Rates {
        assert!(clock_hz > 0, "clock rate must not be zero");
        assert!(sample_rate > 0, "sample rate must not be zero");
        assert!(oversample > 0, "oversampling factor must not be zero");
        let inner = u64::from(sample_rate) * u64::from(oversample);
        assert!(
            inner < clock_hz,
            "windows at {inner} Hz against a {clock_hz} Hz clock are shorter than a T-state"
        );
        Rates {
            clock_hz,
            sample_rate,
            oversample,
        }
    }

    /// The machine's clock, in Hz.
    pub fn clock_hz(&self) -> u64 {
        self.clock_hz
    }

    /// The device's sample rate, in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// How many windows there are per output sample.
    pub fn oversample(&self) -> u32 {
        self.oversample
    }

    /// The rate the windows run at, which is what [`Windowed`] produces.
    pub fn inner_rate(&self) -> u64 {
        u64::from(self.sample_rate) * u64::from(self.oversample)
    }

    /// How many of [`Windowed`]'s time units there are in one T-state.
    ///
    /// See the module note: dividing the T-state this finely is what makes
    /// every window boundary a whole number and the window grid exactly
    /// uniform.
    fn tick(&self) -> u128 {
        u128::from(self.inner_rate())
    }

    /// The width of one window in those units, which is the same for every
    /// window and is the whole point of them.
    fn window_width(&self) -> u128 {
        u128::from(self.clock_hz)
    }

    /// The most windows a frame of `frame_ticks` T-states can close.
    ///
    /// A span of `L` T-states contains at most `L * rate / clock + 1`
    /// boundaries, whatever it is aligned against.
    pub fn max_windows(&self, frame_ticks: u32) -> usize {
        (u64::from(frame_ticks) * self.inner_rate() / self.clock_hz) as usize + 1
    }
}

/// The windowed average: edges in, one sample per closed window out.
#[derive(Clone, Debug)]
pub struct Windowed {
    rates: Rates,
    /// The window being accumulated into, counted from the start of the
    /// stream. Never reset — see the module note on drift.
    window: u64,
    /// Level times time accumulated into that window so far.
    acc: f64,
    /// The instant accumulation has reached, absolute, in the exact units of
    /// [`Rates::tick`].
    cursor: u128,
    /// The T-state the frame about to be rendered begins at, absolute.
    origin: u64,
}

impl Windowed {
    pub fn new(rates: Rates) -> Windowed {
        Windowed {
            rates,
            window: 0,
            acc: 0.0,
            cursor: 0,
            origin: 0,
        }
    }

    /// The rates this was built for.
    pub fn rates(&self) -> Rates {
        self.rates
    }

    /// Windows closed since the stream began.
    pub fn windows(&self) -> u64 {
        self.window
    }

    /// Render one frame's edges, writing one sample per window the frame
    /// closes and returning how many that was.
    ///
    /// `edges` and `start` are what [`EdgeLog::frame`](crate::EdgeLog::frame)
    /// returns; edges at or past `frame_ticks` belong to the next frame and
    /// are left for it. A window straddling the boundary is carried over
    /// part-accumulated, so calling this once per frame produces exactly the
    /// stream that one call over the whole run would have.
    ///
    /// # Panics
    ///
    /// If `out` is shorter than [`Rates::max_windows`] for this frame length.
    /// The buffer is the caller's to size once, so a short one is a mistake
    /// rather than a condition.
    pub fn render(
        &mut self,
        edges: &[u32],
        start: Levels,
        frame_ticks: u32,
        mic_level: f32,
        out: &mut [f32],
    ) -> usize {
        assert!(
            out.len() >= self.rates.max_windows(frame_ticks),
            "output buffer of {} is too short for a {frame_ticks} T-state frame",
            out.len()
        );

        let tick = self.rates.tick();
        let width = self.rates.window_width();
        let origin = u128::from(self.origin) * tick;
        let frame_end = origin + u128::from(frame_ticks) * tick;

        let mut level = f64::from(start.amplitude(mic_level));
        let mut next_edge = 0;
        let mut written = 0;

        loop {
            // Everything that happened at or before the cursor has happened.
            // Edges are in T-state order, so the first one past the end of the
            // frame ends the walk: the rest are the next frame's.
            while let Some(&edge) = edges.get(next_edge) {
                let t = origin + u128::from(tick_of(edge)) * tick;
                if t >= frame_end {
                    next_edge = edges.len();
                    break;
                }
                if t > self.cursor {
                    break;
                }
                level = f64::from(levels_of(edge).amplitude(mic_level));
                next_edge += 1;
            }

            if self.cursor >= frame_end {
                break;
            }

            // Run to whichever comes first: the next edge, the end of the
            // window, or the end of the frame. All three are strictly ahead of
            // the cursor, so this always advances.
            let until_edge = edges
                .get(next_edge)
                .map_or(u128::MAX, |&edge| origin + u128::from(tick_of(edge)) * tick);
            let until_window = (u128::from(self.window) + 1) * width;
            let step_to = until_edge.min(until_window).min(frame_end);

            self.acc += level * (step_to - self.cursor) as f64;
            self.cursor = step_to;

            if self.cursor == until_window {
                // Every window is exactly `width` wide, so this is the mean
                // level over it and not an approximation to one.
                out[written] = (self.acc / width as f64) as f32;
                written += 1;
                self.acc = 0.0;
                self.window += 1;
            }
        }

        self.origin += u64::from(frame_ticks);
        written
    }
}

/// Taps in the decimator's filter.
///
/// Chosen from the transition band the job actually needs rather than from a
/// round number. Coming down from 192 kHz to 48 kHz, everything above 24 kHz
/// folds back into the band, so the filter has to be flat to the top of what
/// anyone can hear — 20 kHz — and deep by 28 kHz, above which the images land
/// between 20 and 24 kHz where nothing can hear them either. That is a
/// transition of 8 kHz in 192, and a Blackman window needs about `5.5 / width`
/// taps to manage it, which is 132; one more makes it odd, so the filter is
/// symmetric about a real tap and its delay is a whole number of samples.
///
/// Blackman gives 74 dB of stopband for that, and 133 multiply-adds per output
/// sample is 6.4 Mflop/s — against a CPU core that retires 157 million
/// instructions a second, which is what makes this the cheap way to buy 20 dB.
pub const DECIMATOR_TAPS: usize = 133;

/// The anti-image filter on the way down from the oversampled rate.
///
/// [`Windowed`] running four times as fast puts the beeper's surviving images
/// four times further from the band, but they are still there, and taking
/// every fourth sample would fold them straight back in. This is what removes
/// them first: a symmetric linear-phase FIR, windowed with a Blackman, cut off
/// at the output's Nyquist.
///
/// Nothing here allocates. The taps are computed into a fixed array at
/// construction and the history is another one, because this runs on the
/// emulation thread and ADR-0007 says nothing there allocates.
#[derive(Clone)]
pub struct Decimator {
    factor: u32,
    taps: [f32; DECIMATOR_TAPS],
    history: [f32; DECIMATOR_TAPS],
    /// Index of the most recently written sample.
    pos: usize,
    /// Inner samples taken since the last output.
    phase: u32,
}

impl Decimator {
    /// A decimator taking one output sample per `factor` inner samples.
    ///
    /// A factor of one is a pass-through: with no oversampling there is no
    /// image to remove that the window average did not already deal with.
    pub fn new(factor: u32) -> Decimator {
        assert!(factor > 0, "decimation factor must not be zero");
        Decimator {
            factor,
            taps: Decimator::design(factor),
            history: [0.0; DECIMATOR_TAPS],
            pos: 0,
            phase: 0,
        }
    }

    /// Windowed sinc, normalised to unity gain at DC.
    fn design(factor: u32) -> [f32; DECIMATOR_TAPS] {
        let mut taps = [0.0f32; DECIMATOR_TAPS];
        if factor == 1 {
            return taps;
        }

        let last = (DECIMATOR_TAPS - 1) as f64;
        let centre = last / 2.0;
        // Half the output sample rate, in cycles per inner sample.
        let cutoff = 1.0 / (2.0 * f64::from(factor));

        let mut sum = 0.0f64;
        for (i, tap) in taps.iter_mut().enumerate() {
            let x = i as f64 - centre;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let phase = 2.0 * PI * i as f64 / last;
            let blackman = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();

            let value = sinc * blackman;
            sum += value;
            *tap = value as f32;
        }
        // Unity at DC, so that a held level comes through at its own height.
        for tap in taps.iter_mut() {
            *tap = (f64::from(*tap) / sum) as f32;
        }
        taps
    }

    /// The decimation factor.
    pub fn factor(&self) -> u32 {
        self.factor
    }

    /// Group delay, in inner samples. Half the filter's length, because it is
    /// symmetric.
    pub const fn group_delay(&self) -> usize {
        (DECIMATOR_TAPS - 1) / 2
    }

    /// Take one inner sample; get an output sample every `factor` of them.
    pub fn push(&mut self, x: f32) -> Option<f32> {
        if self.factor == 1 {
            return Some(x);
        }

        self.pos = if self.pos == 0 {
            DECIMATOR_TAPS - 1
        } else {
            self.pos - 1
        };
        self.history[self.pos] = x;

        self.phase += 1;
        if self.phase < self.factor {
            return None;
        }
        self.phase = 0;

        // history[pos] is the newest sample and the ring runs forwards from
        // it, so tap k multiplies the sample k inner ticks old.
        let mut acc = 0.0f32;
        let mut i = self.pos;
        for &tap in &self.taps {
            acc += tap * self.history[i];
            i += 1;
            if i == DECIMATOR_TAPS {
                i = 0;
            }
        }
        Some(acc)
    }

    /// Forget the history, as at the start of a stream.
    pub fn reset(&mut self) {
        self.history = [0.0; DECIMATOR_TAPS];
        self.pos = 0;
        self.phase = 0;
    }
}

impl std::fmt::Debug for Decimator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decimator")
            .field("factor", &self.factor)
            .field("taps", &DECIMATOR_TAPS)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::pack;

    const LOW: Levels = Levels {
        speaker: false,
        mic: false,
    };
    const HIGH: Levels = Levels {
        speaker: true,
        mic: false,
    };

    /// A clock and a sample rate small enough to check on paper: one window is
    /// exactly 100 T-states, and a frame is exactly ten of them.
    fn toy() -> Windowed {
        Windowed::new(Rates::new(1_000, 10, 1))
    }

    /// Spectrum rates, no oversampling.
    fn spectrum() -> Windowed {
        Windowed::new(Rates::new(3_500_000, 48_000, 1))
    }

    const FRAME: u32 = 69_888;

    #[test]
    fn a_window_with_no_edges_in_it_is_the_level_it_started_at() {
        let mut w = toy();
        let mut out = [0.0f32; 16];
        let n = w.render(&[], HIGH, 1_000, 0.0, &mut out);
        assert_eq!(n, 10);
        assert!(out[..10].iter().all(|&s| s == 1.0), "{:?}", &out[..10]);

        let n = w.render(&[], LOW, 1_000, 0.0, &mut out);
        assert_eq!(n, 10);
        assert!(out[..10].iter().all(|&s| s == 0.0), "{:?}", &out[..10]);
    }

    #[test]
    fn an_edge_part_way_through_a_window_moves_it_by_exactly_that_fraction() {
        let mut w = toy();
        let mut out = [0.0f32; 16];
        // Window 3 covers T-states 300..400. Going high a quarter of the way
        // in leaves three quarters of the window high.
        let n = w.render(&[pack(325, HIGH)], LOW, 1_000, 0.0, &mut out);

        assert_eq!(n, 10);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.75);
        assert_eq!(out[4], 1.0);
    }

    #[test]
    fn two_edges_inside_one_window_average_to_their_duty() {
        let mut w = toy();
        let mut out = [0.0f32; 16];
        // High for 300..340 and 360..380 — sixty of a hundred T-states, in a
        // window a point sample would have called silent either way.
        let edges = [
            pack(300, HIGH),
            pack(340, LOW),
            pack(360, HIGH),
            pack(380, LOW),
        ];
        let n = w.render(&edges, LOW, 1_000, 0.0, &mut out);

        assert_eq!(n, 10);
        assert_eq!(out[3], 0.6);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[4], 0.0);
    }

    #[test]
    fn a_window_straddling_the_frame_boundary_is_carried_over_not_rounded() {
        // Windows of 1000/9 T-states against frames of 700, so no frame ends
        // on a boundary: window 6 spans 666⅔ to 777⁷⁄₉ and the join is at 700.
        let mut w = Windowed::new(Rates::new(1_000, 9, 1));
        let mut out = [0.0f32; 16];

        // Frame one is silent; frame two is high throughout. The window that
        // spans the join must come out part way between, not at either end.
        let first = w.render(&[], LOW, 700, 0.0, &mut out);
        let straddler = out[first - 1];
        let second = w.render(&[], HIGH, 700, 0.0, &mut out);

        assert_eq!(first, 6);
        assert_eq!(straddler, 0.0);
        // Three tenths of the window fell in the silent frame and seven in the
        // loud one — exactly, the boundaries being exact.
        assert!(
            (out[0] - 0.7).abs() < 1e-6,
            "the straddling window should be part high: {}",
            out[0]
        );
        assert_eq!(out[1], 1.0);
        // Twelve boundaries fall inside 1400 T-states, and twelve windows
        // closed, however ragged the frames were against them.
        assert_eq!(first + second, 12);
    }

    #[test]
    fn edges_past_the_end_of_the_frame_are_left_for_the_next_one() {
        let mut w = toy();
        let mut out = [0.0f32; 16];
        let edges = [pack(1_050, HIGH)];
        let n = w.render(&edges, LOW, 1_000, 0.0, &mut out);

        assert_eq!(n, 10);
        assert!(out[..10].iter().all(|&s| s == 0.0), "{:?}", &out[..10]);
    }

    #[test]
    fn the_sample_rate_holds_exactly_over_a_thousand_frames() {
        // 69,888 T-states is 958.464 samples at 48 kHz: a resampler that
        // rounded per frame would be out by 23 samples a second.
        let mut w = spectrum();
        let mut out = vec![0.0f32; w.rates().max_windows(FRAME)];
        let mut total = 0;
        for _ in 0..1_000 {
            total += w.render(&[], LOW, FRAME, 0.0, &mut out);
        }

        let elapsed = u64::from(FRAME) * 1_000;
        let expected = (elapsed * 48_000 / 3_500_000) as usize;
        assert_eq!(total, expected, "{total} samples for {elapsed} T-states");
        // Which is a shade over twenty seconds of audio, to within a sample.
        assert_eq!(total, 958_464);
    }

    #[test]
    fn a_square_wave_averages_to_its_duty_cycle() {
        // A 1 kHz note: half a millisecond high, half a millisecond low. The
        // half-period of 1750 T-states divides by no whole number of samples,
        // but the mean over nineteen whole cycles is a half regardless.
        let mut w = spectrum();
        let mut out = vec![0.0f32; w.rates().max_windows(FRAME)];
        let edges: Vec<u32> = (0..38)
            .map(|half| pack(half * 1_750, if half % 2 == 0 { HIGH } else { LOW }))
            .collect();

        let n = w.render(&edges, LOW, 38 * 1_750, 0.0, &mut out);
        assert_eq!(n, 912);
        let mean = out[..n].iter().sum::<f32>() / n as f32;
        assert!((mean - 0.5).abs() < 1e-3, "{mean}");
    }

    #[test]
    fn the_mic_bit_moves_the_level_by_its_share() {
        let mut w = toy();
        let mut out = [0.0f32; 16];
        let mic = Levels {
            speaker: false,
            mic: true,
        };
        let n = w.render(
            &[pack(200, mic), pack(400, HIGH)],
            LOW,
            1_000,
            0.2,
            &mut out,
        );

        assert_eq!(n, 10);
        assert_eq!(out[1], 0.0);
        assert!((out[2] - 0.2).abs() < 1e-6, "{}", out[2]);
        assert!((out[4] - 0.8).abs() < 1e-6, "{}", out[4]);
    }

    #[test]
    #[should_panic(expected = "too short")]
    fn a_buffer_too_short_for_the_frame_is_a_mistake_and_says_so() {
        let mut w = spectrum();
        let mut out = [0.0f32; 16];
        w.render(&[], LOW, FRAME, 0.0, &mut out);
    }

    /// Push a sine of `cycles_per_inner_sample` through a decimator and return
    /// the amplitude of what comes out, past the startup transient.
    fn response(factor: u32, cycles_per_sample: f64) -> f64 {
        let mut d = Decimator::new(factor);
        let mut peak: f64 = 0.0;
        let n = 4_000;
        for i in 0..n {
            let x = (2.0 * PI * cycles_per_sample * i as f64).sin() as f32;
            if let Some(y) = d.push(x)
                && i > DECIMATOR_TAPS * 2
            {
                peak = peak.max(f64::from(y).abs());
            }
        }
        peak
    }

    #[test]
    fn a_decimator_takes_one_sample_in_every_factor() {
        let mut d = Decimator::new(4);
        let outputs = (0..40).filter_map(|i| d.push(i as f32)).count();
        assert_eq!(outputs, 10);

        let mut passthrough = Decimator::new(1);
        let outputs = (0..40).filter_map(|i| passthrough.push(i as f32)).count();
        assert_eq!(outputs, 40);
    }

    #[test]
    fn a_pass_through_decimator_changes_nothing_at_all() {
        let mut d = Decimator::new(1);
        for i in 0..100 {
            let x = (i as f32 * 0.37).sin();
            assert_eq!(d.push(x), Some(x));
        }
    }

    #[test]
    fn a_held_level_comes_through_at_its_own_height() {
        // Unity at DC: a decimator that did not normalise would make every
        // held level quieter or louder than it was.
        let mut d = Decimator::new(4);
        let mut last = 0.0;
        for _ in 0..2_000 {
            if let Some(y) = d.push(0.75) {
                last = y;
            }
        }
        assert!((last - 0.75).abs() < 1e-4, "{last}");
    }

    #[test]
    fn the_filter_is_flat_below_twenty_kilohertz_and_deep_above_twenty_eight() {
        // At 192 kHz inner rate: 20 kHz is 0.1042 cycles a sample, 28 kHz is
        // 0.1458, and 48 kHz — the first image of anything at DC — is 0.25.
        for audible in [0.001, 0.01, 0.05, 0.1042] {
            let db = 20.0 * response(4, audible).log10();
            assert!(db > -1.0, "{audible} cycles/sample is {db} dB down");
        }
        for stopband in [0.1458, 0.2, 0.25, 0.4] {
            let db = 20.0 * response(4, stopband).log10();
            assert!(db < -70.0, "{stopband} cycles/sample is only {db} dB down");
        }
    }

    #[test]
    fn the_filter_is_symmetric_so_that_it_delays_without_smearing() {
        let d = Decimator::new(4);
        for i in 0..DECIMATOR_TAPS / 2 {
            let left = d.taps[i];
            let right = d.taps[DECIMATOR_TAPS - 1 - i];
            assert!((left - right).abs() < 1e-9, "tap {i}: {left} vs {right}");
        }
        assert_eq!(d.group_delay(), 66);
    }
}
