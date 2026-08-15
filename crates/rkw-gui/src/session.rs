//! The machine, the thread it runs on, and the four things the window needs
//! from it.
//!
//! Assembling a frontend's machine is three wrappers deep, and the order is
//! the answer to "who reads the frame first":
//!
//! ```text
//!   Presenting<AudioMachine>   paint the finished frame for the window
//!     AudioMachine             make the frame's sound before the log rolls
//!       Spectrum               the machine itself
//! ```
//!
//! Each of them is a [`Machine`](rkw_debug::machine::Machine) and each adds
//! one thing to `service_event`, which is where per-frame host work belongs
//! (ADR-0021). Nothing is added to [`Spectrum`], which stays plain machine
//! state so that the checkpoint ring of ticket 0027 can clone it.
//!
//! What comes back out is deliberately narrow: commands in through the ring,
//! frames out through the swap chain, sound out through the sample ring, and a
//! run state to read. The window does not have a reference to the machine and
//! cannot get one.

use std::thread::JoinHandle;

use rkw_audio::{Volume, ring};
use rkw_debug::command::{Command, Tape};
use rkw_debug::emu::{self, Config, Emu, Handle, RunState};
use rkw_debug::machine::Clock;
use rkw_debug::{Debugger, StopReason};
use rkw_spectrum::keymap::{HostKey, HostKeys, KeyMap};
use rkw_spectrum::{AudioMachine, FrameSource, Framebuffer, Presenting, Spectrum, present};
use z80::Cpu;

use crate::pacing::{Pacer, Speed, SpeedControl};
use crate::speaker::{self, Speaker};

/// How much sound the ring holds, as frames of a 50 Hz machine.
///
/// Four frames is 80 ms: long enough that a scheduling hiccup on the way to
/// the device is covered, short enough that a keypress is not heard a fifth of
/// a second late. Rounded up to a power of two, which the ring requires.
const RING_FRAMES: u32 = 4;

/// The rate the beeper is built for when there is no device to ask. Nothing
/// plays it, so what matters is only that the resampler has a rate it can work
/// with.
const FALLBACK_RATE: u32 = 48_000;

/// The machine a frontend runs: a Spectrum that makes a noise and paints its
/// frames where the window can find them.
pub type Machine48 = Presenting<AudioMachine>;

/// Everything the window talks to.
pub struct Session {
    handle: Handle,
    join: Option<JoinHandle<Emu<Machine48>>>,
    frames: FrameSource,
    /// Held because dropping it stops the stream. `None` on a host with no
    /// output device, where the machine runs on in silence.
    speaker: Option<Speaker>,
    /// The knob the mute policy turns, which exists whether or not there is a
    /// speaker to turn it on.
    volume: Volume,
    speed: SpeedControl,
    /// The host keys the user is holding, and the table they are read through.
    held: HostKeys,
    keymap: KeyMap,
    /// The matrix last sent, so that a key event that changed nothing — an
    /// auto-repeat, a modifier the table has no use for — does not put a
    /// command in the ring.
    matrix: u64,
    /// The user's own mute, as distinct from the mute that comes of running at
    /// a speed nobody wants to listen to.
    muted: bool,
}

impl Session {
    /// Put `spectrum` on a thread, with sound and a window's worth of picture.
    ///
    /// The device is opened first, because its sample rate is what the
    /// machine's resampler has to be built for and a machine cannot be told
    /// afterwards.
    ///
    /// A host with no output device is not an error: the machine runs, the
    /// window works, and the pacing falls back to the wall clock, because a
    /// sample ring nobody drains would fill once and stop the machine for
    /// good. What the caller gets told is the error, so that it can say so.
    pub fn new(spectrum: Spectrum) -> (Session, Option<speaker::Error>) {
        Session::starting_at(spectrum, Cpu::new())
    }

    /// The same, with the CPU the caller wants rather than one at reset.
    ///
    /// A machine booting a ROM starts at `0x0000` and needs nothing said about
    /// it. A program loaded straight into RAM — an assembled game, a snapshot —
    /// has an entry point instead, and this is where it comes in.
    pub fn starting_at(spectrum: Spectrum, cpu: Cpu) -> (Session, Option<speaker::Error>) {
        // The device comes first because its rate is what the beeper is built
        // for, and the stream is opened before the thread is spawned, so that
        // a device which fails half way through has not left a machine
        // running behind it.
        let device = speaker::Device::default_output();
        let sample_rate = match &device {
            Ok(device) => device.sample_rate(),
            Err(_) => FALLBACK_RATE,
        };
        let capacity = (sample_rate / 50 * RING_FRAMES).next_power_of_two() as usize;

        let (samples, rx) = ring::channel(capacity);
        let (sink, frames) = present::channel();
        let machine = Presenting::new(
            AudioMachine::with_defaults(spectrum, sample_rate, samples),
            sink,
        );

        let volume = Volume::default();
        let opened = device.and_then(|device| device.open(rx, volume.clone()));
        let (speaker, failure) = match opened {
            Ok(speaker) => (Some(speaker), None),
            Err(e) => (None, Some(e)),
        };

        let speed = SpeedControl::default();
        let mut pacer = match speaker {
            Some(_) => Pacer::new(speed.clone(), capacity, sample_rate),
            // Nothing is draining the ring, so it fills once and stays full;
            // pacing on it would stop the machine for good.
            None => Pacer::silent(speed.clone()),
        };
        let (mut handle, join) = emu::spawn_paced(
            cpu,
            machine,
            Debugger::new(),
            Config::default(),
            move |emu| pacer.wait(emu.machine.t_states(), emu.machine.inner().fill()),
        );

        // A frontend's machine starts running: the debugger's reason for
        // starting paused is that it has a prompt to put a breakpoint at, and
        // this has a window with a picture in it.
        let _ = handle.send(Command::Resume);

        let session = Session {
            handle,
            join: Some(join),
            frames,
            speaker,
            volume,
            speed,
            held: HostKeys::new(),
            keymap: KeyMap::default(),
            matrix: 0,
            muted: false,
        };
        (session, failure)
    }

    /// The newest frame, if one has been painted since the last call.
    pub fn take_frame(&mut self) -> Option<&Framebuffer> {
        self.frames.take()
    }

    /// The frame drawn last, for a redraw that is not a new frame — a resize,
    /// or the first paint of a machine that has not finished one yet.
    pub fn latest_frame(&self) -> &Framebuffer {
        self.frames.latest()
    }

    pub fn state(&self) -> RunState {
        self.handle.state()
    }

    pub fn stop_reason(&self) -> Option<StopReason> {
        self.handle.stop_reason()
    }

    pub fn speed(&self) -> Speed {
        self.speed.get()
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Frames the machine painted that the window never drew, and callbacks
    /// the machine did not keep up with. Both are zero when the assumptions
    /// hold, which is what makes them worth showing.
    pub fn missed_frames(&self) -> u64 {
        self.frames.missed()
    }

    /// Frames the machine has painted, drawn or not. Fifty a second on a
    /// machine that is keeping up, which is how a caller checks that it is.
    pub fn frames(&self) -> u64 {
        self.frames.taken() + self.frames.missed()
    }

    pub fn underruns(&self) -> u64 {
        self.speaker.as_ref().map_or(0, Speaker::underruns)
    }

    /// Whether there is a device at all. A window may want to say so, since a
    /// silent machine otherwise looks like a broken one.
    pub fn has_sound(&self) -> bool {
        self.speaker.is_some()
    }

    /// Hold or let go of a host key, and send the matrix it makes.
    ///
    /// The matrix is rebuilt from everything held rather than edited, which is
    /// what keeps a released combination from letting a modifier up that the
    /// user is still holding (see [`rkw_spectrum::keymap`]).
    pub fn key(&mut self, key: HostKey, pressed: bool) {
        if pressed {
            self.held.press(key);
        } else {
            self.held.release(key);
        }
        self.send_matrix();
    }

    /// Let every key up.
    ///
    /// A window that has lost focus stops being told about key releases, so
    /// without this the machine would be left leaning on whatever was down
    /// when the user alt-tabbed away.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.send_matrix();
    }

    fn send_matrix(&mut self) {
        let matrix = self.held.matrix(&self.keymap).matrix();
        if matrix == self.matrix {
            return;
        }
        // A full ring means the emulation thread has not reached a control
        // tick in the time it took to send 256 commands, which a person
        // cannot do by typing. Dropping the send would leave a key stuck, so
        // the matrix is left unrecorded and the next event sends it again.
        if self.handle.send(Command::Keys(matrix)).is_ok() {
            self.matrix = matrix;
        }
    }

    /// Stop the machine, or start it again. Returns the state it is now
    /// heading for.
    pub fn toggle_pause(&mut self) -> RunState {
        match self.state() {
            RunState::Running => {
                let _ = self.handle.send(Command::Pause);
                RunState::Paused
            }
            _ => {
                let _ = self.handle.send(Command::Resume);
                RunState::Running
            }
        }
    }

    /// Whether the tape was running at the last frame the machine painted.
    pub fn tape_playing(&self) -> bool {
        self.frames.tape_playing()
    }

    /// Press play, or stop a tape that is already running.
    ///
    /// The deck's own state decides which, not a note kept here: a tape that
    /// has reached its end has stopped itself, and a toggle that did not know
    /// that would need pressing twice to start the next load.
    pub fn toggle_tape(&mut self) -> bool {
        let playing = self.tape_playing();
        let button = if playing { Tape::Stop } else { Tape::Play };
        let _ = self.handle.send(Command::Tape(button));
        // The machine may be parked in a pacing wait, and a tape that started
        // a wait later than it was asked to is a tape the loader half missed.
        self.wake();
        !playing
    }

    /// Wind back to the first block, for the second go at a load.
    pub fn rewind_tape(&mut self) {
        let _ = self.handle.send(Command::Tape(Tape::Rewind));
    }

    /// Pull the reset line: the register file survives, as it does on real
    /// hardware, and the ROM starts again at zero.
    pub fn reset(&mut self) {
        let _ = self.handle.send(Command::Reset);
        let _ = self.handle.send(Command::Resume);
    }

    /// The next speed round.
    pub fn cycle_speed(&mut self) -> Speed {
        let speed = self.speed.cycle();
        // The machine is parked between slices when it is pacing, and a change
        // of speed it does not wake up to would not take effect until the
        // wait it was already in had run out.
        self.wake();
        speed
    }

    /// Silence the speaker, or let it speak. The user's own switch: the mute
    /// that comes of pausing or fast-forwarding is
    /// [`Session::apply_mute_policy`] and is not remembered.
    pub fn toggle_mute(&mut self) -> bool {
        self.muted = !self.muted;
        self.apply_mute_policy();
        self.muted
    }

    /// Mute unless the machine is running at the speed its sound was made for.
    ///
    /// A paused machine underruns continuously and a fast one plays a chipmunk
    /// version of whatever it is doing; both are muted, and neither is a
    /// failure to be rescued from. Called once per redraw rather than at each
    /// transition, because one of the transitions — a breakpoint stopping the
    /// machine — happens on the other thread and never passes through here.
    pub fn apply_mute_policy(&self) {
        let audible = self.speed.get().is_normal() && self.state() == RunState::Running;
        self.volume.set_muted(self.muted || !audible);
    }

    /// Nudge the emulation thread, which may be parked in a pacing wait.
    fn wake(&mut self) {
        // Anything harmless will do; the point is the unpark that goes with
        // it, and a resume of a running machine is a state it is already in.
        if self.state() == RunState::Running {
            let _ = self.handle.send(Command::Resume);
        }
    }

    /// Stop the machine and take it back. The stream stops with the session.
    pub fn quit(mut self) -> Option<Emu<Machine48>> {
        let _ = self.handle.send(Command::Quit);
        self.join.take().and_then(|join| join.join().ok())
    }
}
