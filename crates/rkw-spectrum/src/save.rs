//! A machine that also writes down what it puts on the `MIC` line.
//!
//! Saving is the second consumer of the edge log ADR-0021 put in the ULA for
//! the beeper, and it wants the same moment: once a frame, on the emulation
//! thread, before [`Ula::end_frame`](crate::Ula::end_frame) rolls the log on.
//! So it is built the same way, as a machine wrapping a machine — [`Saving`]
//! delegates every part of being a Spectrum and adds one thing to
//! `service_event`.
//!
//! It is generic over what it wraps, so `Saving<Spectrum>` and
//! `Saving<AudioMachine>` are both machines and the stack composes in the
//! order a front end wants: the recorder reads the log first, the beeper reads
//! it second, and the innermost machine ends the frame.
//!
//! # Why the recorder is not in the machine
//!
//! Loading is machine state and saving is not, which looks inconsistent until
//! you ask what a checkpoint has to restore. A load in progress *is* machine
//! state: the loader is timing pulses, and a machine restored without the tape
//! position resumes into a measurement of a waveform that is no longer
//! playing. A save in progress is an output. Restoring past one loses a
//! partial block, and the failure is a file that is visibly short rather than
//! a machine that is invisibly different.
//!
//! The other half of the reason is size. A block being assembled is up to
//! 64 KB, and ADR-0017's checkpoint ring holds a machine per emulated second;
//! putting the recorder in `Spectrum` would double what a minute of history
//! costs to save something nobody can step backwards into.
//!
//! # What it costs when nothing is saving
//!
//! A walk over the frame's edges, which is a few hundred `u32`s in the worst
//! case and empty in the ordinary one, plus the `idle` call that notices the
//! silence. The recorder's own state machine sees only `MIC` transitions, and
//! a program that never touches bit 3 produces none.

use rkw_audio::{levels_of, tick_of};
use rkw_debug::machine::{Clock, Machine};
use rkw_tape::{Recorder, Tap};
use z80::Bus;
use z80::disasm::Peek;

use crate::frame::T_STATES_PER_FRAME;
use crate::spectrum::Spectrum;

/// A machine whose `MIC` output is being recorded onto a tape.
#[derive(Debug)]
pub struct Saving<M> {
    inner: M,
    recorder: Recorder,
}

impl<M: Machine + AsRef<Spectrum>> Saving<M> {
    /// Wrap a machine, recording with the timings its own deck plays.
    pub fn new(inner: M) -> Saving<M> {
        let timing = inner.as_ref().tape.timing();
        Saving {
            inner,
            recorder: Recorder::new(timing),
        }
    }

    /// Wrap a machine with a recorder that has already been set up — a
    /// different tolerance, or buffers of a size the caller has a reason for.
    pub fn with_recorder(inner: M, recorder: Recorder) -> Saving<M> {
        Saving { inner, recorder }
    }

    /// What has been recorded.
    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    pub fn recorder_mut(&mut self) -> &mut Recorder {
        &mut self.recorder
    }

    /// The blocks recorded so far as a TAP file, ready to be written out.
    ///
    /// Only meaningful once the tape has gone quiet: a block is not published
    /// until the silence after it has been waited out, because until then it
    /// is indistinguishable from a block that is still being written.
    pub fn to_tap(&self) -> Tap {
        self.recorder.to_tap()
    }

    /// The machine underneath.
    pub fn inner(&self) -> &M {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut M {
        &mut self.inner
    }

    pub fn into_inner(self) -> M {
        self.inner
    }

    /// Feed the frame's `MIC` transitions to the recorder.
    ///
    /// Bounded at the end of the frame rather than at the clock, because the
    /// log may hold edges that overran it and
    /// [`EdgeLog::roll`](rkw_audio::EdgeLog::roll) is about to rebase those
    /// onto the next frame. Reading them here as well as there would record
    /// every overrunning edge twice, and a duplicated pulse in the middle of a
    /// block is a byte that decodes to something else.
    fn drain(&mut self) {
        let spectrum = self.inner.as_ref();
        let (edges, start) = spectrum.ula.audio().frame();
        let frame_start = spectrum.ula.frame_start();
        let mut mic = start.mic;
        for &edge in edges {
            let tick = tick_of(edge);
            if u64::from(tick) >= T_STATES_PER_FRAME {
                break;
            }
            let levels = levels_of(edge);
            if levels.mic != mic {
                mic = levels.mic;
                self.recorder.edge(frame_start + u64::from(tick));
            }
        }
        // The end of a block looks exactly like the middle of one until the
        // silence after it has been waited out, and this is what waits it out.
        self.recorder.idle(frame_start + T_STATES_PER_FRAME);
    }
}

impl<M: Machine + AsRef<Spectrum>> AsRef<Spectrum> for Saving<M> {
    fn as_ref(&self) -> &Spectrum {
        self.inner.as_ref()
    }
}

impl<M: Machine + AsMut<Spectrum>> AsMut<Spectrum> for Saving<M> {
    fn as_mut(&mut self) -> &mut Spectrum {
        self.inner.as_mut()
    }
}

// Being the machine underneath: every one of these goes straight through, and
// the machine-cycle wrappers are delegated as well as the raw accessors for
// the reason `audio.rs` gives — ticket 0020 will override them for contention,
// and a wrapper left on the trait's default body would quietly run an
// uncontended machine.
impl<M: Machine> Bus for Saving<M> {
    fn read(&mut self, addr: u16) -> u8 {
        self.inner.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.inner.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        self.inner.input(port)
    }

    fn output(&mut self, port: u16, value: u8) {
        self.inner.output(port, value);
    }

    fn tick(&mut self, t: u32) {
        self.inner.tick(t);
    }

    fn tick_at(&mut self, addr: u16, t: u32) {
        self.inner.tick_at(addr, t);
    }

    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.inner.fetch_opcode(addr)
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        self.inner.read_cycle(addr)
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.inner.write_cycle(addr, value);
    }

    fn input_cycle(&mut self, port: u16) -> u8 {
        self.inner.input_cycle(port)
    }

    fn output_cycle(&mut self, port: u16, value: u8) {
        self.inner.output_cycle(port, value);
    }

    fn interrupt_data(&mut self) -> u8 {
        self.inner.interrupt_data()
    }

    fn interrupt_pending(&self) -> bool {
        self.inner.interrupt_pending()
    }

    fn nmi_pending(&mut self) -> bool {
        self.inner.nmi_pending()
    }
}

impl<M: Machine> Peek for Saving<M> {
    fn peek(&self, addr: u16) -> u8 {
        self.inner.peek(addr)
    }
}

impl<M: Machine> Clock for Saving<M> {
    fn t_states(&self) -> u64 {
        self.inner.t_states()
    }
}

impl<M: Machine + AsRef<Spectrum>> Machine for Saving<M> {
    fn next_event(&self) -> Option<u64> {
        self.inner.next_event()
    }

    /// Take the frame's `MIC` edges, then let the machine underneath have its
    /// turn — in that order, because the frame ends down there and the log
    /// rolls on with it.
    ///
    /// Only at the end of a frame: a tape edge wakes this too, and draining a
    /// log that has not moved thousands of times a frame would be pure cost.
    fn service_event(&mut self) {
        if self.inner.as_ref().frame_due() {
            self.drain();
        }
        self.inner.service_event();
    }

    fn set_keys(&mut self, matrix: u64) {
        self.inner.set_keys(matrix);
    }
}
