//! A Z80 program plays a note, and the note comes out at the right pitch.
//!
//! This is ticket 0014's acceptance criterion in its strongest form. The unit
//! tests in `rkw-audio` feed the resampler an edge list built by hand, which
//! proves the arithmetic and assumes the wiring; this drives real instructions
//! through the real CPU, the real ULA and the real slice loop, and measures
//! what comes out of the ring. Everything in between — the T-state a write
//! lands on, the frame boundary, the order `service_event` does its two jobs
//! in — is under test rather than stipulated.
//!
//! # The tone, and why its period is what it is
//!
//! The loop below flips bit 4 once per pass and takes `45 + 13(n−1)` T-states
//! for a delay count of `n`, so the note is `3_500_000 / (2 × (45 + 13(n−1)))`.
//! At `n = 132` that is 1001.14 Hz — near a round number and deliberately not
//! on one, because a resampler with an off-by-one in its window arithmetic
//! tends to come out exactly right on frequencies that divide the clock and
//! wrong on the ones that do not.
//!
//! The measurement is a Goertzel, for the reasons `rkw-audio`'s spectrum test
//! gives, and it runs against [`Speaker::Flat`] because the piezo's 300 Hz
//! high-pass and 2.5 kHz resonance would be measuring the cone rather than the
//! path.

use rkw_audio::beeper::Config;
use rkw_audio::filter::Speaker;
use rkw_audio::ring;
use rkw_debug::Debugger;
use rkw_debug::command::Command;
use rkw_debug::emu::{self, Emu, RunState};
use rkw_spectrum::{AudioMachine, CLOCK_HZ, Spectrum, T_STATES_PER_FRAME};
use z80::Cpu;

/// A tone loop, flipping the speaker once per pass.
///
/// ```text
/// 8000  3E 10      start:  LD A,$10        ; 7   speaker high
/// 8002  D3 FE      loop:   OUT ($FE),A     ; 11
/// 8004  06 nn              LD B,n          ; 7
/// 8006  10 FE      delay:  DJNZ delay      ; 13 taken, 8 falling through
/// 8008  EE 10              XOR $10         ; 7   flip the speaker bit
/// 800A  18 F6              JR loop         ; 12
/// ```
///
/// One pass is `11 + 7 + [13(n−1) + 8] + 7 + 12` = `45 + 13(n−1)` T-states,
/// which is a half-period of the note.
fn tone(delay: u8) -> Vec<u8> {
    vec![
        0x3E, 0x10, // ld a,$10
        0xD3, 0xFE, // out ($fe),a
        0x06, delay, // ld b,delay
        0x10, 0xFE, // djnz -2
        0xEE, 0x10, // xor $10
        0x18, 0xF6, // jr loop
    ]
}

/// `EI` and `RETI`, poked into ROM because that is where `IM 1` sends it.
const HANDLER: &[u8] = &[0xFB, 0xED, 0x4D];

/// Half the ring, in samples: big enough to hold every frame this test runs.
const RING: usize = 1 << 16;

/// Run `frames` frames of the tone loop and return the samples it made.
fn play(delay: u8, frames: u64, sample_rate: u32) -> Vec<f32> {
    let (tx, mut rx) = ring::channel(RING);

    let mut spectrum = Spectrum::new();
    spectrum.memory.load(0x8000, &tone(delay));
    spectrum.memory.load(0x0038, HANDLER);

    let config = Config::new(CLOCK_HZ, T_STATES_PER_FRAME as u32, sample_rate)
        .speaker(Speaker::Flat);
    let machine = AudioMachine::new(spectrum, config, tx);

    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;

    let (mut e, mut handle) = Emu::new(
        cpu,
        machine,
        Debugger::new(),
        emu::Config {
            event_capacity: 16,
            command_capacity: 16,
            control_interval: 224,
            log_capacity: 0,
        },
    );
    handle.send(Command::Resume).unwrap();

    let until = frames * T_STATES_PER_FRAME;
    while e.machine.spectrum.ula.frames() < frames {
        assert_eq!(e.slice(), RunState::Running);
        assert!(
            e.machine.spectrum.t_states() < until + 10 * T_STATES_PER_FRAME,
            "the machine ran past its deadline without ending frames"
        );
    }

    assert_eq!(e.machine.dropped(), 0, "the ring was too small for the test");
    assert_eq!(e.machine.edges_dropped(), 0, "the edge log overflowed");

    let mut samples = vec![0.0f32; rx.len()];
    let n = rx.pop(&mut samples);
    samples.truncate(n);
    samples
}

/// The magnitude of one bin of `signal`, by the Goertzel recurrence, where
/// `cycles` is cycles per record rather than hertz.
fn bin(signal: &[f32], cycles: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * cycles / signal.len() as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in signal {
        let s = f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0).sqrt()
}

/// The frequency, in hertz, with the most energy in it — found by scanning
/// rather than by peak-picking a transform, because a few hundred Goertzels
/// are cheaper than a transform and this only needs the one answer.
///
/// Coarse first, then fine around the winner, so that resolving to a quarter
/// of a hertz does not cost a hundred thousand passes over the record.
///
/// The coarse pass runs over a short prefix rather than the whole record, and
/// that is not an optimisation but a correctness point: a long record has bins
/// under a hertz wide, so a coarse grid steps straight over the peaks and
/// whichever component happens to land nearest a grid point wins. Measured
/// that way a square wave's third harmonic beats its own fundamental about a
/// third of the time. Two thousand samples puts the bins 23 Hz apart, wide
/// enough that a 12 Hz step cannot miss one, and then the fine pass gets the
/// resolution back from the full record.
fn dominant_hz(signal: &[f32], sample_rate: u32) -> f64 {
    let nyquist = f64::from(sample_rate) / 2.0;

    let scan = |over: &[f32], from: f64, to: f64, step: f64| {
        let seconds = over.len() as f64 / f64::from(sample_rate);
        let mut best = (0.0f64, -1.0f64);
        let mut hz = from.max(20.0);
        while hz < to.min(nyquist - 10.0) {
            let magnitude = bin(over, hz * seconds);
            if magnitude > best.1 {
                best = (hz, magnitude);
            }
            hz += step;
        }
        best.0
    };

    let prefix = &signal[..2_048.min(signal.len())];
    let coarse = scan(prefix, 100.0, nyquist, 12.0);
    scan(signal, coarse - 40.0, coarse + 40.0, 0.25)
}

/// The note a delay count of `n` should produce, from the loop's own T-states.
fn expected_hz(delay: u8) -> f64 {
    let half_period = 45.0 + 13.0 * (f64::from(delay) - 1.0);
    CLOCK_HZ as f64 / (2.0 * half_period)
}

#[test]
fn a_z80_program_playing_a_note_produces_that_note() {
    let rate = 48_000;
    // Long enough to resolve under a hertz, and to average over the interrupt
    // handler stealing time from the loop every frame.
    let samples = play(132, 60, rate);
    assert!(samples.len() > 45_000, "{} samples", samples.len());

    // Discard the decimator's startup and the first interrupt's disruption.
    let found = dominant_hz(&samples[2_000..], rate);
    let expected = expected_hz(132);

    // 1001.14 Hz. The frame interrupt steals thirty-odd T-states once every
    // 69,888, which lengthens one half-period in twenty and flattens the note
    // by well under a hertz; the tolerance is a couple of hertz rather than a
    // fraction of one, which is still a fifth of a semitone.
    assert!(
        (found - expected).abs() < 3.0,
        "expected {expected:.2} Hz, measured {found} Hz"
    );
}

#[test]
fn a_slower_loop_produces_a_lower_note_in_the_right_proportion() {
    // The control for the test above. A frequency measurement that agreed with
    // one hand-computed number could be a coincidence or a constant; two
    // notes in the ratio the program puts them in cannot be.
    let rate = 48_000;
    let fast = dominant_hz(&play(66, 60, rate)[2_000..], rate);
    let slow = dominant_hz(&play(132, 60, rate)[2_000..], rate);

    // Doubling the delay count does not double the loop, because only the
    // DJNZ scales with it: 1748 T-states against 890 is a ratio of 1.964 and
    // not of 2.
    let expected = expected_hz(66) / expected_hz(132);
    let ratio = fast / slow;
    assert!(
        (ratio - expected).abs() < 0.01,
        "the notes are in the ratio {ratio}, not {expected:.4}"
    );
    assert!((expected - 1.964).abs() < 0.001, "{expected}");
}

#[test]
fn the_machine_makes_a_second_of_audio_for_every_second_it_emulates() {
    // What "without underruns at normal speed" means as something a test can
    // actually assert. A wall-clock test would be flaky; this is the property
    // underneath it — the machine's output rate matches the device's input
    // rate exactly, so a paced front end can never run dry systematically.
    for rate in [44_100, 48_000] {
        let frames = 50;
        let samples = play(132, frames, rate);

        let elapsed = frames * T_STATES_PER_FRAME;
        let expected = (elapsed * u64::from(rate) / CLOCK_HZ) as usize;
        assert!(
            samples.len().abs_diff(expected) <= 1,
            "{rate} Hz: {} samples for {elapsed} T-states, expected {expected}",
            samples.len()
        );
    }
}

#[test]
fn the_sound_is_made_before_the_frame_is_rolled_on() {
    // The one ordering that matters in AudioMachine::service_event. Ending the
    // frame first would clear the edge log before it was read, and every frame
    // would come out silent — which is a bug that looks exactly like "audio
    // not wired up yet" and could live for a long time.
    let samples = play(132, 10, 48_000);
    let loudest = samples.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    assert!(
        loudest > 0.05,
        "the machine was playing a note and produced silence: peak {loudest}"
    );
}
