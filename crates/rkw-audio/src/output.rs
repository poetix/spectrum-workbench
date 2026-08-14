//! The device's end: volume, mute, and what to play when there is nothing.
//!
//! # Volume is applied here and not upstream
//!
//! It would be easier to scale the samples as they are made. It would also be
//! wrong three times over.
//!
//! ADR-0017 is the first and the binding one. A volume control is host state.
//! Getting it onto the emulation thread means either putting it through the
//! command ring — which that ADR reserves for things the emulation thread
//! could not have worked out for itself, and a volume knob is not one, and
//! putting it there would write it into the replay log as though it were
//! input — or having the emulation thread read a shared atomic mid-run, which
//! is the same leak through a different hole.
//!
//! The second is latency. Samples already in the ring were scaled when they
//! were made, so a gain change takes effect only once the ring drains: at a
//! 60 ms buffer, a mute you hear a sixteenth of a second after you press it
//! and a slider that visibly lags the mouse.
//!
//! The third is that it keeps the ring's contents a pure function of the
//! machine. That is what lets the spectrum tests measure the resampler without
//! knowing the volume, and what would let ticket 0029 compare a replay's audio
//! against the original's.
//!
//! # Gain ramps, and silence fades
//!
//! Two places where the obvious thing clicks:
//!
//! **Mute is not a multiplication by zero.** The speaker is somewhere when the
//! mute arrives, and dropping it to the axis in one sample is a step, and a
//! step is a click — the loudest thing in the whole program, arriving exactly
//! when the user asked for quiet. So gain slews to its target over a few
//! milliseconds.
//!
//! **An underrun is not a buffer of zeros.** The same argument: the last
//! sample was somewhere, and the machine is very likely holding the speaker
//! high. So an underrun holds the last sample and fades it out, which turns a
//! bang into a small sigh. This is the common case rather than the rare one —
//! a debugger sitting at a breakpoint underruns continuously, for as long as
//! the person is reading the screen.
//!
//! Muting does not stop the draining, either. A consumer that stopped popping
//! would let the ring back up, and the front end paces itself on how full the
//! ring is, so muting would change how fast the machine ran.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::ring::SampleRx;

/// How long the gain takes to reach a new setting, in seconds.
///
/// Long enough that a step becomes a slope below anything anyone can hear as a
/// click; short enough that a mute is a mute and not a fade-out.
const RAMP_SECONDS: f32 = 0.005;

/// How long an underrun takes to fade to silence, in seconds.
///
/// Shorter than the gain ramp: this is damage control rather than a control
/// gesture, and holding a level for longer than a couple of milliseconds
/// starts to be audible as a buzz at the callback rate.
const FADE_SECONDS: f32 = 0.002;

/// A volume knob, shared between whoever turns it and whoever reads it.
///
/// Cloning gives another handle on the same knob.
#[derive(Clone, Debug)]
pub struct Volume {
    /// The target gain, as `f32::to_bits`.
    gain: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
}

impl Default for Volume {
    fn default() -> Self {
        Volume::new(1.0)
    }
}

impl Volume {
    /// A knob set to `gain`, unmuted.
    pub fn new(gain: f32) -> Volume {
        Volume {
            gain: Arc::new(AtomicU32::new(gain.clamp(0.0, 1.0).to_bits())),
            muted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the gain, from silent to unity.
    pub fn set(&self, gain: f32) {
        self.gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The gain the knob is set to, which is not the gain in force until the
    /// ramp has caught up.
    pub fn get(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    /// Mute or unmute without disturbing the gain, so that unmuting returns to
    /// where the knob was.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// Whether it is muted.
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Flip the mute, returning the new state.
    pub fn toggle_mute(&self) -> bool {
        let muted = !self.is_muted();
        self.set_muted(muted);
        muted
    }

    /// What the ramp is heading for: the gain, or zero if muted.
    fn target(&self) -> f32 {
        if self.is_muted() { 0.0 } else { self.get() }
    }
}

/// What one call to [`Output::fill`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    /// Samples that came from the machine.
    pub written: usize,
    /// Samples the ring could not supply, which were filled with the fade.
    pub missing: usize,
}

impl Fill {
    /// Whether the device asked for more than there was.
    pub fn underran(&self) -> bool {
        self.missing > 0
    }
}

/// The consumer end of the beeper: drains the ring, applies the knob, and
/// covers for the machine when it falls behind.
pub struct Output {
    rx: SampleRx,
    volume: Volume,
    /// The gain actually in force, which chases [`Volume::target`].
    gain: f32,
    /// The most the gain may move in one sample.
    ramp: f32,
    /// The last sample played, which is where a fade starts from.
    last: f32,
    /// How much of the last sample is left, during a fade.
    fade: f32,
    /// How much the fade loses per sample.
    fade_step: f32,
    /// Callbacks that could not be filled from the ring.
    underruns: u64,
}

impl Output {
    /// Wrap the consumer end of a ring, at the device's sample rate.
    pub fn new(rx: SampleRx, volume: Volume, sample_rate: u32) -> Output {
        let rate = sample_rate.max(1) as f32;
        Output {
            gain: volume.target(),
            rx,
            volume,
            ramp: 1.0 / (RAMP_SECONDS * rate),
            last: 0.0,
            fade: 0.0,
            fade_step: 1.0 / (FADE_SECONDS * rate),
            underruns: 0,
        }
    }

    /// The knob this is reading.
    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Callbacks that ran short, over the life of the stream.
    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Samples the machine made that there was no room for, over the life of
    /// the stream. Ordinary while the machine is running ahead of the speaker.
    pub fn dropped(&self) -> u64 {
        self.rx.dropped()
    }

    /// Samples waiting in the ring — how far ahead of the speaker the machine
    /// has got.
    pub fn buffered(&self) -> usize {
        self.rx.len()
    }

    /// Fill a device buffer. Never blocks, never allocates, always fills the
    /// whole of `out`.
    pub fn fill(&mut self, out: &mut [f32]) -> Fill {
        let written = self.rx.pop(out);
        let target = self.volume.target();

        for sample in out[..written].iter_mut() {
            self.gain += (target - self.gain).clamp(-self.ramp, self.ramp);
            self.last = *sample;
            *sample *= self.gain;
        }

        // Whatever the ring could not supply: hold the last sample where it
        // was and let it go, rather than stepping to zero.
        if written < out.len() {
            self.underruns += 1;
            if written > 0 {
                // A fresh underrun starts from a full hold; one that runs on
                // from the last callback carries on from where it faded to.
                self.fade = 1.0;
            }
            for sample in out[written..].iter_mut() {
                self.gain += (target - self.gain).clamp(-self.ramp, self.ramp);
                self.fade = (self.fade - self.fade_step).max(0.0);
                *sample = self.last * self.fade * self.gain;
            }
        } else {
            self.fade = 1.0;
        }

        Fill {
            written,
            missing: out.len() - written,
        }
    }
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output")
            .field("buffered", &self.buffered())
            .field("underruns", &self.underruns)
            .field("gain", &self.gain)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring;

    const RATE: u32 = 48_000;

    /// An output fed `n` copies of `value`, with its ramp already settled.
    fn fed(value: f32, n: usize, volume: Volume) -> (Output, ring::SampleTx) {
        let (mut tx, rx) = ring::channel(8_192);
        tx.push(&vec![value; n]);
        let mut output = Output::new(rx, volume, RATE);
        // The ramp starts at the target, so nothing to settle unless the knob
        // moves; this is just to make the intent explicit.
        output.gain = output.volume.target();
        (output, tx)
    }

    /// The largest jump between one sample and the next.
    fn largest_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn what_the_machine_made_comes_out_at_the_volume_it_is_set_to() {
        let (mut output, _tx) = fed(0.5, 1_000, Volume::new(0.5));
        let mut buf = [0.0f32; 100];
        let fill = output.fill(&mut buf);

        assert_eq!(fill, Fill { written: 100, missing: 0 });
        assert!(!fill.underran());
        assert!(buf.iter().all(|&s| (s - 0.25).abs() < 1e-6), "{:?}", &buf[..4]);
    }

    #[test]
    fn a_mute_reaches_silence_without_a_click() {
        let (mut output, _tx) = fed(1.0, 8_000, Volume::default());
        let mut buf = [0.0f32; 480];

        output.fill(&mut buf);
        assert!(buf.iter().all(|&s| s > 0.99), "should be at full volume");

        output.volume().set_muted(true);
        let mut ramping = vec![0.0f32; 480];
        output.fill(&mut ramping);

        // Silent by the end of ten milliseconds, and never stepping by more
        // than the ramp allows on the way.
        assert_eq!(*ramping.last().unwrap(), 0.0);
        assert!(
            largest_step(&ramping) < 0.01,
            "the mute stepped by {}",
            largest_step(&ramping)
        );

        // And it stays muted.
        let mut after = [0.0f32; 100];
        output.fill(&mut after);
        assert!(after.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn unmuting_returns_to_the_volume_the_knob_was_left_at() {
        let volume = Volume::new(0.6);
        let (mut output, _tx) = fed(1.0, 8_000, volume.clone());
        let mut buf = [0.0f32; 480];

        volume.set_muted(true);
        output.fill(&mut buf);
        assert_eq!(*buf.last().unwrap(), 0.0);

        volume.set_muted(false);
        output.fill(&mut buf);
        assert!(
            (buf.last().unwrap() - 0.6).abs() < 1e-3,
            "came back to {} rather than 0.6",
            buf.last().unwrap()
        );
        assert!(largest_step(&buf) < 0.01);
    }

    #[test]
    fn toggling_the_mute_reports_where_it_ended_up() {
        let volume = Volume::default();
        assert!(!volume.is_muted());
        assert!(volume.toggle_mute());
        assert!(volume.is_muted());
        assert!(!volume.toggle_mute());
        assert!(!volume.is_muted());
    }

    #[test]
    fn an_underrun_fades_the_last_sample_out_rather_than_dropping_it() {
        // The machine stops mid-buffer with the speaker somewhere loud, which
        // is what a breakpoint looks like from down here.
        let (mut output, _tx) = fed(0.8, 50, Volume::default());
        let mut buf = [0.0f32; 480];
        let fill = output.fill(&mut buf);

        assert_eq!(fill.written, 50);
        assert_eq!(fill.missing, 430);
        assert!(fill.underran());
        assert_eq!(output.underruns(), 1);

        // No cliff where the samples ran out...
        assert!(
            largest_step(&buf) < 0.01,
            "the underrun stepped by {}",
            largest_step(&buf)
        );
        // ...and silence by the end, not a held tone.
        assert_eq!(*buf.last().unwrap(), 0.0);
    }

    #[test]
    fn a_machine_that_never_comes_back_stays_quiet_rather_than_buzzing() {
        // A paused debugger underruns every callback, for as long as the
        // person is reading the screen. Holding the last sample each time
        // would be a tone at the callback rate.
        let (mut output, _tx) = fed(0.8, 50, Volume::default());
        let mut buf = [0.0f32; 480];
        output.fill(&mut buf);

        for _ in 0..100 {
            let fill = output.fill(&mut buf);
            assert_eq!(fill.written, 0);
            assert!(buf.iter().all(|&s| s == 0.0), "still making noise: {:?}", &buf[..4]);
        }
        assert_eq!(output.underruns(), 101);
    }

    #[test]
    fn the_output_reports_how_far_ahead_the_machine_is() {
        let (mut output, mut tx) = fed(0.5, 1_000, Volume::default());
        assert_eq!(output.buffered(), 1_000);

        let mut buf = [0.0f32; 400];
        output.fill(&mut buf);
        assert_eq!(output.buffered(), 600);

        tx.push(&vec![0.5; 10_000]);
        assert!(output.dropped() > 0, "a full ring should have refused some");
    }
}
