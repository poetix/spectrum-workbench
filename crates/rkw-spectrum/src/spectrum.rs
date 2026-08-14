//! The machine: memory, ULA and clock, wired to the CPU's [`Bus`].
//!
//! This is the smallest object that is a Spectrum. It counts T-states, decodes
//! the one port the 48K machine has, keeps the ULA's frame clock, and answers
//! the interrupt line — and it is what [`rkw_debug::Emu`] runs.
//!
//! # What is deliberately not here yet
//!
//! **Contention.** Every machine cycle costs its nominal T-states, because the
//! wait states the ULA inserts are ticket 0020. That is a change to
//! [`Bus::read_cycle`] and its siblings and to nothing else, which is the
//! property ADR-0002 exists to preserve; the raw accessors below stay as they
//! are.
//!
//! **The floating bus.** A read of an unattached port returns `0xFF` rather
//! than the byte the ULA happens to be fetching, which is the same ticket.

use rkw_debug::machine::{Clock, Machine};
use rkw_tape::Image;
use z80::Bus;
use z80::disasm::Peek;

use crate::memory::{Memory, RomTooLarge, SCREEN_BASE};
use crate::screen::{Flash, Framebuffer};
use crate::tape::Tape;
use crate::ula::Ula;

/// A 48K Spectrum.
#[derive(Clone)]
pub struct Spectrum {
    pub memory: Memory,
    pub ula: Ula,
    /// The tape deck, which is machine state because the loader's timing
    /// depends on it (ADR-0022). Empty and stopped on a machine nobody has put
    /// a tape into, and free until then.
    pub tape: Tape,
    /// T-states since the machine was made. Never reset: the debugger's
    /// deadlines and the ULA's frame clock are both absolute (ADR-0007).
    t: u64,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    /// A machine with no ROM in it, which is useful for running assembled code
    /// that does not need one and is what every test here does.
    pub fn new() -> Spectrum {
        Spectrum {
            memory: Memory::new(),
            ula: Ula::new(),
            tape: Tape::new(),
            t: 0,
        }
    }

    /// A machine with a ROM image at `0x0000`.
    pub fn with_rom(rom: &[u8]) -> Result<Spectrum, RomTooLarge> {
        let mut machine = Spectrum::new();
        machine.memory.load_rom(rom)?;
        Ok(machine)
    }

    pub fn t_states(&self) -> u64 {
        self.t
    }

    /// Put a tape in the deck, rewound and stopped: a [`rkw_tape::Tap`], a
    /// [`rkw_tape::Tzx`], or either of them behind an `Arc` already.
    pub fn mount_tape(&mut self, image: impl Into<Image>) {
        self.tape.mount(image);
    }

    /// Start the tape from where the head is. The first pulse begins now, so a
    /// program that is already in its loading loop sees the pilot immediately.
    pub fn play_tape(&mut self) {
        self.tape.play(self.t);
        self.ula.set_ear(self.tape.level());
    }

    /// Stop the tape and let the `EAR` line go back to idle.
    pub fn stop_tape(&mut self) {
        self.tape.stop();
        self.ula.set_ear(self.tape.level());
    }

    /// True when the clock has reached the end of the frame in progress.
    ///
    /// [`Machine::service_event`] is called for tape edges as well as for the
    /// frame interrupt, so anything that does per-frame work from inside one —
    /// the beeper of ADR-0021, the tape recorder of ADR-0022 — has to ask
    /// which of the two it was woken for. Doing that work on a tape edge would
    /// run it thousands of times a frame over a log that has barely changed.
    pub fn frame_due(&self) -> bool {
        self.t >= self.ula.next_interrupt()
    }

    /// Paint the last complete frame: the screen at `0x4000` as it stands now,
    /// the border as it was recorded line by line, and the flash phase the ULA
    /// has reached.
    ///
    /// Rendering is done here rather than at the end of each frame on purpose.
    /// A frontend at 50 Hz and a debugger stopped at a breakpoint both want a
    /// picture at a moment of their own choosing, and a machine that painted
    /// 104 KB into a buffer every frame would be doing it for nobody when
    /// running headless.
    ///
    /// The screen bytes are read live, so a render part way through a frame can
    /// catch a routine mid-draw. That is a feature where the caller is a
    /// debugger — it is what ticket 0025 is for — and invisible where it is a
    /// frontend rendering at the frame boundary.
    pub fn render(&self, out: &mut Framebuffer) {
        out.draw_border(self.ula.border_lines());
        out.draw_display(&self.memory, SCREEN_BASE, self.ula.flash());
    }

    /// The screen as a fresh framebuffer. The allocating convenience over
    /// [`Spectrum::render`]; a caller rendering every frame keeps one
    /// [`Framebuffer`] and reuses it.
    pub fn frame(&self) -> Framebuffer {
        let mut out = Framebuffer::new();
        self.render(&mut out);
        out
    }

    /// Rendered with a flash phase of the caller's choosing, for a screen that
    /// has to look the same every time it is drawn (ticket 0025).
    pub fn render_with(&self, base: u16, flash: Flash, out: &mut Framebuffer) {
        out.draw_border(self.ula.border_lines());
        out.draw_display(&self.memory, base, flash);
    }

    /// True for the port the ULA answers, which is any port with `A0` low —
    /// `0xFE` is the conventional spelling of it, and the ROM uses `0x7FFE`,
    /// `0xBFFE` and the rest to select keyboard half-rows, which is why the
    /// whole address goes through to [`Ula::read_port_fe`] and not just the
    /// low byte.
    fn is_ula(port: u16) -> bool {
        port & 1 == 0
    }
}

impl Bus for Spectrum {
    fn read(&mut self, addr: u16) -> u8 {
        self.memory.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.memory.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        if Spectrum::is_ula(port) {
            self.ula.read_port_fe(port)
        } else {
            // The floating bus, until ticket 0020 makes it float.
            0xFF
        }
    }

    fn output(&mut self, port: u16, value: u8) {
        if Spectrum::is_ula(port) {
            // At the T-state the ULA sees the write, which the machine cycle
            // wrapper has already advanced the clock to.
            self.ula.write_port_fe(self.t, value);
        }
    }

    fn tick(&mut self, t: u32) {
        self.t += u64::from(t);
    }

    fn interrupt_pending(&self) -> bool {
        self.ula.interrupt_pending(self.t)
    }
}

impl Peek for Spectrum {
    fn peek(&self, addr: u16) -> u8 {
        self.memory.read(addr)
    }
}

/// The bottom of the wrapper stack: a `Spectrum` is the machine a
/// [`Saving`](crate::save::Saving) or an [`AudioMachine`](crate::AudioMachine)
/// is looking for, and so is itself.
impl AsRef<Spectrum> for Spectrum {
    fn as_ref(&self) -> &Spectrum {
        self
    }
}

impl AsMut<Spectrum> for Spectrum {
    fn as_mut(&mut self) -> &mut Spectrum {
        self
    }
}

impl Clock for Spectrum {
    fn t_states(&self) -> u64 {
        self.t
    }
}

impl Machine for Spectrum {
    /// The earlier of the next frame interrupt and the next tape edge.
    ///
    /// A running tape puts an event here every few hundred T-states, which is
    /// finer than the control tick and is what a loader's own timing is made
    /// of. A stopped one costs a branch.
    fn next_event(&self) -> Option<u64> {
        let interrupt = self.ula.next_interrupt();
        Some(match self.tape.next_edge() {
            Some(edge) => edge.min(interrupt),
            None => interrupt,
        })
    }

    /// Whichever of the two was due — and both, when a single step has carried
    /// the clock past a frame boundary and a pulse at once.
    fn service_event(&mut self) {
        if self.tape.next_edge().is_some_and(|edge| self.t >= edge) {
            let level = self.tape.advance_to(self.t);
            self.ula.set_ear(level);
        }
        if self.frame_due() {
            self.ula.end_frame();
        }
    }
}
