//! A Spectrum that also makes a noise.
//!
//! # Where does per-frame host code run?
//!
//! Sound has to be made once a frame, on the emulation thread, from state the
//! machine has just finished writing. There is exactly one moment that fits:
//! [`Machine::service_event`], which the slice loop calls when the clock
//! reaches the frame interrupt. But that method belongs to the `Machine` impl,
//! and the `Machine` here is [`Spectrum`] — the one type that must stay plain
//! machine state, because ticket 0027 will clone it into a checkpoint ring and
//! ADR-0017 says host state and wall-clock time may not go in there.
//!
//! `Emu` is generic over its machine, though, and a machine is a small trait.
//! So rather than putting a hook into the slice loop or a sample rate into the
//! `Ula`, this wraps one machine in another: an [`AudioMachine`] *is* a
//! `Machine`, delegates every part of being a Spectrum to the `Spectrum` it
//! holds, and adds one thing to `service_event` — make this frame's sound,
//! then end the frame.
//!
//! What the front end (ticket 0019) does differently as a result is spawn
//! `Emu<AudioMachine>` instead of `Emu<Spectrum>`. Nothing else changes: not
//! the debugger, not the command set, not the events.
//!
//! # The order matters, and is the whole reason the edge log is single-buffered
//!
//! [`Ula::end_frame`](crate::Ula::end_frame) rolls the edge log on. So the
//! sound has to be made *before* it is called, and because it is, there is never a second reader of
//! that log on another thread — which is why it needs no second buffer, unlike
//! the border. Getting this backwards would produce silence, not corruption,
//! which is the kind of bug that survives a long time; hence the test.
//!
//! # What is deliberately not checkpointed
//!
//! When 0027 lands, an `AudioMachine` will hand it the `Spectrum` and keep the
//! beeper. Restoring a checkpoint therefore leaves the filters wherever they
//! were rather than where they were at the checkpoint, and the audible result
//! is a click at the seam. That is correct: the alternative is a sample rate
//! and a filter's state inside the thing whose bit-for-bit equality 0029 wants
//! to check for divergence.

use rkw_audio::beeper::{Beeper, Config};
use rkw_audio::ring::SampleTx;
use rkw_debug::machine::{Clock, Machine};
use z80::Bus;
use z80::disasm::Peek;

use crate::frame::{CLOCK_HZ, T_STATES_PER_FRAME};
use crate::spectrum::Spectrum;

/// A [`Spectrum`] wired to a [`Beeper`], for running on the emulation thread.
pub struct AudioMachine {
    /// The machine itself. Public because everything a caller wants to do to a
    /// Spectrum — poke it, render it, press a key — it still does directly.
    pub spectrum: Spectrum,
    beeper: Beeper,
    out: SampleTx,
}

impl AudioMachine {
    /// Wrap a machine, sending its sound to `out`.
    ///
    /// The [`Config`]'s clock and frame length are overwritten with this
    /// machine's, because those are not the caller's to choose; everything
    /// else — the device's rate, the speaker, the headroom — is.
    pub fn new(spectrum: Spectrum, config: Config, out: SampleTx) -> AudioMachine {
        let config = Config {
            clock_hz: CLOCK_HZ,
            frame_ticks: T_STATES_PER_FRAME as u32,
            ..config
        };
        AudioMachine {
            spectrum,
            beeper: Beeper::new(config),
            out,
        }
    }

    /// A machine sending sound to `out` at the device's rate, with everything
    /// else defaulted.
    pub fn with_defaults(spectrum: Spectrum, sample_rate: u32, out: SampleTx) -> AudioMachine {
        let config = Config::new(CLOCK_HZ, T_STATES_PER_FRAME as u32, sample_rate);
        AudioMachine::new(spectrum, config, out)
    }

    /// What the beeper was built for.
    pub fn config(&self) -> Config {
        self.beeper.config()
    }

    /// How full the sample ring is, from 0.0 to 1.0 — how far ahead of the
    /// speaker the machine has got, and what a front end should pace itself
    /// by.
    pub fn fill(&self) -> f32 {
        self.out.fill()
    }

    /// Samples the ring had no room for. Ordinary while the machine is running
    /// faster than the speaker.
    pub fn dropped(&self) -> u64 {
        self.out.dropped()
    }

    /// Edges the log had no room for, over the life of the machine.
    pub fn edges_dropped(&self) -> u32 {
        self.spectrum.ula.audio().dropped()
    }
}

// Being a Spectrum: every one of these goes straight through.
//
// The machine-cycle wrappers are delegated as well as the raw accessors, even
// though `Spectrum` uses the trait's defaults for them today. Ticket 0020 will
// override them to add contention, and a wrapper delegated to the *default*
// body rather than to the `Spectrum`'s own would then quietly run an
// uncontended machine — with the timing wrong everywhere and nothing pointing
// at this file.
impl Bus for AudioMachine {
    fn read(&mut self, addr: u16) -> u8 {
        self.spectrum.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.spectrum.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        self.spectrum.input(port)
    }

    fn output(&mut self, port: u16, value: u8) {
        self.spectrum.output(port, value);
    }

    fn tick(&mut self, t: u32) {
        self.spectrum.tick(t);
    }

    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.spectrum.fetch_opcode(addr)
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        self.spectrum.read_cycle(addr)
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.spectrum.write_cycle(addr, value);
    }

    fn input_cycle(&mut self, port: u16) -> u8 {
        self.spectrum.input_cycle(port)
    }

    fn output_cycle(&mut self, port: u16, value: u8) {
        self.spectrum.output_cycle(port, value);
    }

    fn interrupt_data(&mut self) -> u8 {
        self.spectrum.interrupt_data()
    }

    fn interrupt_pending(&self) -> bool {
        self.spectrum.interrupt_pending()
    }

    fn nmi_pending(&mut self) -> bool {
        self.spectrum.nmi_pending()
    }
}

impl Peek for AudioMachine {
    fn peek(&self, addr: u16) -> u8 {
        self.spectrum.peek(addr)
    }
}

/// How a wrapper outside this one reaches the machine — the tape recorder of
/// ticket 0016 is one, and it needs the ULA's edge log and the frame boundary.
impl AsRef<Spectrum> for AudioMachine {
    fn as_ref(&self) -> &Spectrum {
        &self.spectrum
    }
}

impl AsMut<Spectrum> for AudioMachine {
    fn as_mut(&mut self) -> &mut Spectrum {
        &mut self.spectrum
    }
}

impl Clock for AudioMachine {
    fn t_states(&self) -> u64 {
        self.spectrum.t_states()
    }
}

impl Machine for AudioMachine {
    fn next_event(&self) -> Option<u64> {
        self.spectrum.next_event()
    }

    /// Make the frame's sound, then end the frame — in that order, because
    /// ending it rolls the edge log on.
    ///
    /// Only when the frame is what came due. Since ticket 0016 a tape edge is
    /// a scheduled event as well, and it arrives every few hundred T-states
    /// while a tape is running; rendering the frame's sound at each one would
    /// produce a frame's worth of samples per pulse.
    fn service_event(&mut self) {
        if self.spectrum.frame_due() {
            let (edges, start) = self.spectrum.ula.audio().frame();
            self.beeper.render_into(edges, start, &mut self.out);
        }
        self.spectrum.service_event();
    }
}

impl std::fmt::Debug for AudioMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioMachine")
            .field("t", &self.spectrum.t_states())
            .field("fill", &self.fill())
            .finish()
    }
}
