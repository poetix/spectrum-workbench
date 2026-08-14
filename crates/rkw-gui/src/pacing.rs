//! How fast the machine is allowed to run.
//!
//! The core does about 360× real time (ADR-0007), so something has to hold it
//! back, and what holds it back at normal speed is the speaker. The machine
//! makes a frame's worth of sound, pushes it into the ring, and when the ring
//! is nearly full it waits — which is exactly what the device is doing at the
//! other end, one callback at a time. Nothing here counts frames or corrects
//! for drift, because the audio device's own clock is the only clock in the
//! system that the user can hear.
//!
//! # Why not a 50 Hz timer
//!
//! A frame timer and an audio device disagree, always: the device's crystal is
//! not 50 Hz times an integer, and the difference shows up as a ring that
//! drifts full or empty over a minute or two and then clicks. Pacing on the
//! ring's fill level removes the second clock instead of trying to reconcile
//! it, and the frame rate falls out at whatever the device's rate implies.
//!
//! # A host with no sound card
//!
//! Then there is no clock to pace against, and the fallback is the wall clock
//! — the same path 2× uses, at 1×. It drifts against nothing anybody can hear,
//! because there is nothing anybody can hear.
//!
//! # And the speeds that are not normal
//!
//! At 2× the audio would have to be resampled to stay in tune, and nobody
//! wants to listen to it anyway, so the sound is muted (see
//! [`Speed::is_normal`]) and the pacing falls back to the wall clock: emulated
//! time against elapsed time, twice as fast. At full speed there is no pacing
//! at all — the machine runs as fast as the host will carry it, which is what
//! a tape loading at 360× wants.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use rkw_spectrum::CLOCK_HZ;

/// How full the sample ring is allowed to get before the machine waits.
///
/// Not 1.0, because the machine overshoots by a frame — a slice is bounded by
/// a T-state deadline, not by a sample count — and a ring that was full when
/// it did would drop the overshoot and take the frame's sound with it. Not
/// much lower either: what is in the ring is what covers a scheduling hiccup
/// on the way to the device.
pub const HIGH_WATER: f32 = 0.75;

/// How fast the machine is being asked to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Speed {
    /// 3.5 MHz, paced by the speaker.
    Normal = 0,
    /// Twice that, paced by the wall clock and silent.
    Double = 1,
    /// As fast as the host manages, paced by nothing and silent.
    Max = 2,
}

impl Speed {
    fn from_u8(v: u8) -> Speed {
        match v {
            1 => Speed::Double,
            2 => Speed::Max,
            _ => Speed::Normal,
        }
    }

    /// The next speed round.
    pub fn next(self) -> Speed {
        match self {
            Speed::Normal => Speed::Double,
            Speed::Double => Speed::Max,
            Speed::Max => Speed::Normal,
        }
    }

    /// Whether the machine is running at the speed its sound was recorded at,
    /// which is the only speed worth listening to.
    pub fn is_normal(self) -> bool {
        self == Speed::Normal
    }

    /// How many times faster than the real machine, for the speeds where that
    /// is a number.
    fn multiplier(self) -> Option<u32> {
        match self {
            Speed::Normal => Some(1),
            Speed::Double => Some(2),
            Speed::Max => None,
        }
    }

    /// What to put in a title bar.
    pub fn label(self) -> &'static str {
        match self {
            Speed::Normal => "1x",
            Speed::Double => "2x",
            Speed::Max => "max",
        }
    }
}

/// The speed knob, shared between the window that turns it and the pacer on
/// the emulation thread that reads it.
#[derive(Debug, Clone)]
pub struct SpeedControl(Arc<AtomicU8>);

impl Default for SpeedControl {
    fn default() -> Self {
        SpeedControl::new(Speed::Normal)
    }
}

impl SpeedControl {
    pub fn new(speed: Speed) -> SpeedControl {
        SpeedControl(Arc::new(AtomicU8::new(speed as u8)))
    }

    pub fn get(&self) -> Speed {
        Speed::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, speed: Speed) {
        self.0.store(speed as u8, Ordering::Relaxed);
    }

    /// Move to the next speed and report it.
    pub fn cycle(&self) -> Speed {
        let speed = self.get().next();
        self.set(speed);
        speed
    }
}

/// How long to wait, given how far ahead of the speaker the machine has got.
///
/// `ring` is how much sound the ring holds when full. Above the high-water
/// mark the machine waits for the excess to drain and no longer: waiting for
/// the ring to empty would leave nothing to cover the next hiccup, and waiting
/// for less would mean waking up to wait again.
pub fn audio_wait(fill: f32, ring: Duration) -> Option<Duration> {
    let excess = fill - HIGH_WATER;
    (excess > 0.0).then(|| ring.mul_f32(excess.min(1.0)))
}

/// How long to wait, given how much emulated time has been run and how much
/// real time it took.
///
/// `None` when the machine is behind, which is the ordinary answer on a host
/// that cannot keep up: there is no catching up to do, because emulated time
/// is measured from the same anchor and the shortfall is already in it.
pub fn clock_wait(emulated: Duration, elapsed: Duration) -> Option<Duration> {
    emulated.checked_sub(elapsed).filter(|d| !d.is_zero())
}

/// Emulated time, from T-states.
pub fn emulated_time(t_states: u64) -> Duration {
    Duration::from_secs_f64(t_states as f64 / CLOCK_HZ as f64)
}

/// The pacer itself: the policy above, plus the anchor the wall-clock speeds
/// measure from.
///
/// One of these goes onto the emulation thread as
/// [`spawn_paced`](rkw_debug::emu::spawn_paced)'s closure, and it is asked
/// after every slice how long to wait.
#[derive(Debug)]
pub struct Pacer {
    speed: SpeedControl,
    /// How much sound the ring holds when it is full.
    ring: Duration,
    /// Whether anything is draining that ring. Without a device the fill level
    /// only ever goes up, so pacing on it would stop the machine for good.
    audio: bool,
    /// Where the wall clock and the emulated clock were last agreed to be
    /// together, and at what speed. Dropped whenever the speed changes,
    /// because the old anchor would demand the new speed's time back.
    anchor: Option<(Instant, u64, Speed)>,
}

impl Pacer {
    /// A pacer for a ring of `capacity` samples at `sample_rate`, with a
    /// device at the other end of it.
    pub fn new(speed: SpeedControl, capacity: usize, sample_rate: u32) -> Pacer {
        Pacer {
            speed,
            ring: Duration::from_secs_f64(capacity as f64 / sample_rate.max(1) as f64),
            audio: true,
            anchor: None,
        }
    }

    /// A pacer for a machine whose sound goes nowhere, which paces on the wall
    /// clock at every speed.
    pub fn silent(speed: SpeedControl) -> Pacer {
        Pacer {
            speed,
            ring: Duration::ZERO,
            audio: false,
            anchor: None,
        }
    }

    /// How long the machine should wait before its next slice, having reached
    /// `t_states` with the sample ring `fill` full.
    pub fn wait(&mut self, t_states: u64, fill: f32) -> Option<Duration> {
        let speed = self.speed.get();
        // An anchor set at another speed measures the wrong thing, and
        // correcting it is more arithmetic than starting again is worth.
        if matches!(self.anchor, Some((_, _, was)) if was != speed) {
            self.anchor = None;
        }
        match speed.multiplier() {
            Some(1) if self.audio => {
                self.anchor = None;
                audio_wait(fill, self.ring)
            }
            Some(multiplier) => {
                let (since, from, _) =
                    *self.anchor.get_or_insert((Instant::now(), t_states, speed));
                let emulated = emulated_time(t_states.saturating_sub(from)) / multiplier;
                clock_wait(emulated, since.elapsed())
            }
            None => {
                self.anchor = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_below_the_high_water_mark_does_not_wait() {
        let ring = Duration::from_millis(100);
        assert_eq!(audio_wait(0.0, ring), None);
        assert_eq!(audio_wait(HIGH_WATER, ring), None);
    }

    #[test]
    fn a_full_ring_waits_for_the_excess_to_drain_and_no_longer() {
        let ring = Duration::from_millis(100);
        // A quarter of a ring above the mark is a quarter of a ring of sound.
        // Within a microsecond: the fill level is an `f32`, and the exactness
        // being asserted is the policy's and not the arithmetic's.
        let close = |wait: Option<Duration>, expected: Duration| {
            let wait = wait.expect("a wait");
            assert!(
                wait.abs_diff(expected) < Duration::from_micros(1),
                "{wait:?}"
            );
        };
        close(audio_wait(1.0, ring), Duration::from_millis(25));
        close(audio_wait(0.85, ring), Duration::from_millis(10));
    }

    #[test]
    fn the_wall_clock_waits_only_while_the_machine_is_ahead() {
        let ahead = clock_wait(Duration::from_millis(20), Duration::from_millis(5));
        assert_eq!(ahead, Some(Duration::from_millis(15)));
        // Behind, or exactly level: run on.
        assert_eq!(
            clock_wait(Duration::from_millis(5), Duration::from_millis(20)),
            None
        );
        assert_eq!(
            clock_wait(Duration::from_millis(5), Duration::from_millis(5)),
            None
        );
    }

    #[test]
    fn a_frames_worth_of_t_states_is_a_fiftieth_of_a_second() {
        let frame = emulated_time(rkw_spectrum::T_STATES_PER_FRAME);
        let expected = Duration::from_secs_f64(1.0 / 50.08);
        assert!(
            frame.abs_diff(expected) < Duration::from_micros(20),
            "{frame:?}"
        );
    }

    #[test]
    fn full_speed_never_waits_and_normal_speed_ignores_the_wall_clock() {
        let control = SpeedControl::new(Speed::Max);
        let mut pacer = Pacer::new(control.clone(), 4096, 48_000);
        assert_eq!(pacer.wait(0, 1.0), None);

        control.set(Speed::Normal);
        // 4096 samples at 48 kHz is about 85 ms of sound; a full ring is a
        // quarter of that above the mark.
        let wait = pacer.wait(0, 1.0).expect("a full ring waits");
        assert!(wait > Duration::from_millis(20) && wait < Duration::from_millis(22));
    }

    /// Without a device the ring never drains, so a pacer that waited on it
    /// would wait for ever the first time it filled.
    #[test]
    fn a_silent_machine_paces_on_the_wall_clock_instead_of_a_ring_nobody_drains() {
        let mut pacer = Pacer::silent(SpeedControl::default());
        // The first call is where the two clocks are agreed to be together; a
        // full ring says nothing here, and what matters is that a machine
        // which then runs a second of emulated time in no time at all waits.
        assert_eq!(pacer.wait(0, 1.0), None);
        let wait = pacer
            .wait(rkw_spectrum::CLOCK_HZ, 1.0)
            .expect("a machine a second ahead of the clock waits");
        assert!(wait > Duration::from_millis(900), "{wait:?}");
    }

    #[test]
    fn the_speeds_go_round() {
        let control = SpeedControl::default();
        assert_eq!(control.get(), Speed::Normal);
        assert!(control.get().is_normal());
        assert_eq!(control.cycle(), Speed::Double);
        assert_eq!(control.cycle(), Speed::Max);
        assert_eq!(control.cycle(), Speed::Normal);
        assert!(!Speed::Double.is_normal());
    }
}
