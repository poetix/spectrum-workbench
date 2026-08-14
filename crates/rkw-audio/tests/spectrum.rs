//! What a square wave sounds like coming out of the other end.
//!
//! This is ticket 0014's acceptance criterion. A beeper program makes a note
//! by flipping one bit at a steady rate, so the thing worth measuring is what
//! happens to a square wave of a frequency we chose: the fundamental has to be
//! where we put it, the harmonics have to fall off as a square wave's do, and
//! the harmonics above half the sample rate have to not come back.
//!
//! # Why a Goertzel and not a transform
//!
//! Six bins are wanted, not a spectrum. A Goertzel is fifteen lines against a
//! real FFT's several hundred, needs no table, and the workspace has exactly
//! one third-party dependency, which is not going to be spent on a test.
//!
//! # Why there is no window function
//!
//! The frequencies here divide the Z80 clock exactly and are measured over a
//! whole number of their own cycles, so every component sits on a bin centre
//! and a rectangular window leaks nothing. That is worth arranging rather than
//! papering over with a Hann window: a windowed measurement of an alias floor
//! measures partly the window, and the numbers below are meant to be the
//! signal.
//!
//! - 1 kHz has a half-period of exactly 1750 T-states, and one cycle is
//!   exactly 48 samples at 48 kHz.
//! - 7 kHz has a period of exactly 500 T-states, and 7000 cycles fit in the
//!   48000-sample record exactly.
//!
//! # The alias meter
//!
//! A 7 kHz square wave's odd harmonics are at 21 kHz (in band), 35 kHz (folds
//! to 13 kHz) and 49 kHz — which folds to 1 kHz. Nothing legitimate is at
//! 1 kHz, because 1 kHz is not a harmonic of 7 kHz, so whatever is in that bin
//! got there by aliasing and its depth below the fundamental is the number
//! this ticket is really about.
//!
//! Point sampling puts the seventh harmonic there at full strength, which is
//! 1/7 of the fundamental, or −16.9 dB. That is measured here too, as the
//! control: a test that only ever asserts a good number cannot tell a working
//! resampler from a broken analyser.

use rkw_audio::{Decimator, Levels, Rates, Windowed, levels_of, pack, tick_of};

const CLOCK: u64 = 3_500_000;
const RATE: u32 = 48_000;

/// One second of audio, which at 48 kHz is a bin per hertz.
const RECORD: usize = 48_000;

/// Samples thrown away before the record starts, so that any filter's startup
/// transient is not part of what is measured. Ninety-six samples is two whole
/// cycles of 1 kHz and fourteen of 7 kHz, so discarding them leaves both tones
/// still sitting on bin centres.
const SETTLE: usize = 96;

const LOW: Levels = Levels {
    speaker: false,
    mic: false,
};
const HIGH: Levels = Levels {
    speaker: true,
    mic: false,
};

/// The edges of a square wave of the given half-period, filling `ticks`
/// T-states.
fn square(half_period: u32, ticks: u32) -> Vec<u32> {
    (0..)
        .map(|half| half * half_period)
        .take_while(|&t| t < ticks)
        .enumerate()
        .map(|(i, t)| pack(t, if i % 2 == 0 { HIGH } else { LOW }))
        .collect()
}

/// The magnitude of one bin, by the Goertzel recurrence.
///
/// `bin` is in cycles per record, which for a 48000-sample record at 48 kHz is
/// hertz.
fn bin(signal: &[f32], bin: usize) -> f64 {
    let w = 2.0 * std::f64::consts::PI * bin as f64 / signal.len() as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in signal {
        let s = f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    // |X[k]| from the last two states, without forming the complex value.
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0).sqrt()
}

/// How far `bin` sits below `reference`, in dB. Positive means quieter.
fn below(signal: &[f32], reference: usize, other: usize) -> f64 {
    let r = bin(signal, reference);
    let o = bin(signal, other);
    20.0 * (r / o.max(1e-30)).log10()
}

/// Render a square wave through windows running `oversample` times faster than
/// the device, decimate back down, and return one second of the result past
/// the settling period.
fn resampled_at(half_period: u32, oversample: u32) -> Vec<f32> {
    let rates = Rates::new(CLOCK, RATE, oversample);
    let mut windowed = Windowed::new(rates);
    let mut decimator = Decimator::new(oversample);

    // Enough T-states to cover the settling period and the record, rendered as
    // one long frame; the frame length is a parameter, and this test is not
    // about frame boundaries.
    let ticks = ((SETTLE + RECORD + 64) as u64 * CLOCK / u64::from(RATE)) as u32;
    let edges = square(half_period, ticks);

    let mut inner = vec![0.0f32; rates.max_windows(ticks)];
    let n = windowed.render(&edges, LOW, ticks, 0.0, &mut inner);
    let out: Vec<f32> = inner[..n].iter().filter_map(|&x| decimator.push(x)).collect();

    assert!(
        out.len() >= SETTLE + RECORD,
        "{} samples is not a whole record",
        out.len()
    );
    out[SETTLE..SETTLE + RECORD].to_vec()
}

/// The shipping path: windows four times faster than the device, filtered on
/// the way down.
fn resampled(half_period: u32) -> Vec<f32> {
    resampled_at(half_period, 4)
}

/// The same edges read at each sample instant instead of averaged over each
/// window — what this ticket exists to not do.
fn point_sampled(half_period: u32) -> Vec<f32> {
    let ticks = ((SETTLE + RECORD + 16) as u64 * CLOCK / u64::from(RATE)) as u32;
    let edges = square(half_period, ticks);

    let mut level = LOW;
    let mut next = 0;
    let mut out = Vec::with_capacity(RECORD);
    for i in SETTLE..SETTLE + RECORD {
        let t = i as u64 * CLOCK / u64::from(RATE);
        while let Some(&edge) = edges.get(next) {
            if u64::from(tick_of(edge)) > t {
                break;
            }
            level = levels_of(edge);
            next += 1;
        }
        out.push(level.amplitude(0.0));
    }
    out
}

#[test]
fn a_one_kilohertz_square_wave_has_a_square_wave_s_harmonics() {
    let signal = resampled(1_750);

    // The odd harmonics of a square wave fall off as 1/n, and nothing else is
    // the loudest thing in the record.
    assert!(
        below(&signal, 1_000, 3_000) > 0.0,
        "the fundamental should be the loudest bin"
    );
    assert!(
        (below(&signal, 1_000, 3_000) - 9.54).abs() < 1.0,
        "third harmonic should be a third: {} dB",
        below(&signal, 1_000, 3_000)
    );
    assert!(
        (below(&signal, 1_000, 5_000) - 13.98).abs() < 1.0,
        "fifth harmonic should be a fifth: {} dB",
        below(&signal, 1_000, 5_000)
    );

    // A square wave has no even harmonics, and a resampler whose windows were
    // biased one way would put something there.
    for even in [2_000, 4_000, 6_000] {
        assert!(
            below(&signal, 1_000, even) > 60.0,
            "even harmonic at {even} Hz: {} dB down",
            below(&signal, 1_000, even)
        );
    }
}

#[test]
fn a_seven_kilohertz_square_wave_does_not_fold_back_into_the_band() {
    let signal = resampled(250);

    // The third harmonic at 21 kHz is still under the Nyquist and still a
    // third, which says the windows are narrow enough not to be rolling off
    // inside the band: a box filter at the output rate would put this at
    // 12.2 dB rather than 9.5.
    let third = below(&signal, 7_000, 21_000);
    assert!(
        (third - 9.54).abs() < 1.0,
        "third harmonic should be a third: {third} dB"
    );

    // The seventh harmonic is at 49 kHz, a kilohertz over the sample rate, and
    // the whole of this ticket is that it does not reappear at 1 kHz.
    let alias = below(&signal, 7_000, 1_000);
    assert!(alias > 70.0, "alias at 1 kHz is only {alias} dB down");
}

#[test]
fn oversampling_is_what_buys_the_last_thirty_decibels() {
    // The control for the stage that added it. Window averaging alone leaves
    // the seventh harmonic about 50 dB down, which is the characteristic
    // grunge of an emulator that stopped there; running the windows four times
    // as fast and filtering on the way down is what takes it past 70. If this
    // test ever stops failing its first assertion, the oversampled path has
    // quietly stopped being oversampled.
    let plain = below(&resampled_at(250, 1), 7_000, 1_000);
    assert!(
        plain < 55.0,
        "window averaging alone should not reach 55 dB, but it managed {plain}"
    );

    let oversampled = below(&resampled_at(250, 4), 7_000, 1_000);
    assert!(
        oversampled - plain > 30.0,
        "oversampling bought only {} dB",
        oversampled - plain
    );
}

#[test]
fn point_sampling_the_same_edges_aliases_badly_enough_to_prove_the_point() {
    // The control. Reading the speaker at each sample instant passes the
    // seventh harmonic of a 7 kHz square wave straight through to 1 kHz at its
    // full 1/7 amplitude, which is 16.9 dB down. If this test ever starts
    // reporting a good number, the analyser above is measuring nothing.
    let naive = point_sampled(250);
    let alias = below(&naive, 7_000, 1_000);

    assert!(
        alias < 25.0,
        "point sampling should alias badly, but the 1 kHz bin is {alias} dB down"
    );
    assert!(
        (alias - 16.9).abs() < 3.0,
        "the seventh harmonic should come through at about 1/7: {alias} dB"
    );

    // And the same edges through the real path are dramatically better, which
    // is the comparison the ticket is asking for.
    let windowed = below(&resampled(250), 7_000, 1_000);
    assert!(
        windowed - alias > 20.0,
        "windowing bought only {} dB over point sampling",
        windowed - alias
    );
}
