//! The machine: memory, ULA and clock, wired to the CPU's [`Bus`].
//!
//! This is the smallest object that is a Spectrum. It counts T-states, decodes
//! the one port the 48K machine has, keeps the ULA's frame clock, answers the
//! interrupt line, and is where the ULA takes the bus away from the CPU — and
//! it is what [`rkw_debug::Emu`] runs.
//!
//! # Where the time is spent
//!
//! Every machine cycle is issued here rather than through the [`Bus`] trait's
//! default bodies, because on this machine none of them costs its nominal
//! duration unconditionally. The rule is the same for all of them: at the
//! T-state the address goes on the bus, ask [`crate::contention`] how long the
//! ULA will hold it, spend that, then spend the cycle.
//!
//! The exception is I/O, which follows different rules to memory and is
//! [`Spectrum::port_early`] and [`Spectrum::port_late`] below.
//!
//! Contention is a per-machine-cycle cost on the hottest path there is, so it
//! is written to be a compare and an arithmetic sequence with no table and no
//! branch on machine type. What that costs is measured in
//! `tests/throughput.rs`.

use rkw_debug::command::Tape as TapeButton;
use rkw_debug::machine::{Clock, Machine};
use rkw_tape::Image;
use z80::Bus;
use z80::disasm::Peek;

use crate::contention;
use crate::keyboard::Keyboard;
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
        self.drive_ear();
    }

    /// Stop the tape and let the `EAR` line go back to what the machine's own
    /// speaker output puts on it.
    pub fn stop_tape(&mut self) {
        self.tape.stop();
        self.drive_ear();
    }

    /// Hand the ULA whoever is driving the `EAR` socket: the tape while it is
    /// playing, and nobody once it stops.
    fn drive_ear(&mut self) {
        let level = self.tape.is_playing().then(|| self.tape.level());
        self.ula.set_ear(level);
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

    /// Wait for the ULA, whatever is on the address bus. The port rules need
    /// this without the address test that [`Spectrum::contend`] does.
    #[inline]
    fn stall(&mut self) {
        self.t += u64::from(contention::delay(
            self.t.wrapping_sub(self.ula.frame_start()),
        ));
    }

    /// Wait for the ULA if `addr` is in the bank it arbitrates for.
    #[inline]
    fn contend(&mut self, addr: u16) {
        if contention::is_contended(addr) {
            self.stall();
        }
    }

    /// The first T-state of an I/O cycle, in which the CPU has put the port on
    /// the address bus but has not yet asserted `IORQ`.
    ///
    /// The ULA cannot tell this apart from the start of a memory cycle — all it
    /// has is sixteen address lines — so it contends on the *port number* as
    /// though it were an address. A port whose high byte happens to fall in
    /// `0x40-0x7F` is therefore slower than one that does not, which is a
    /// genuine property of the machine and not an artefact.
    #[inline]
    fn port_early(&mut self, port: u16) {
        self.contend(port);
        self.t += 1;
    }

    /// The remaining three T-states, in which `IORQ` is asserted.
    ///
    /// Now the ULA has a second reason to hold the CPU: if the port is one it
    /// answers, it has to finish what it is doing before it can. So a ULA port
    /// is contended once for the whole of the access however the address lines
    /// fall, and a non-ULA port in the contended range is contended on each of
    /// the three T-states separately — the CPU is holding the address bus
    /// across all of them and the ULA arbitrates each in turn.
    #[inline]
    fn port_late(&mut self, port: u16) {
        if Spectrum::is_ula(port) {
            self.stall();
            self.t += 2;
        } else if contention::is_contended(port) {
            self.stall();
            self.t += 1;
            self.stall();
            self.t += 1;
            self.stall();
        } else {
            self.t += 2;
        }
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
            // Nothing is attached, so what comes back is whatever the ULA is
            // holding on the data bus at this instant.
            contention::floating_bus(
                &self.memory,
                SCREEN_BASE,
                self.t.wrapping_sub(self.ula.frame_start()),
            )
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

    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.contend(addr);
        let v = self.memory.read(addr);
        self.t += 4;
        v
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        self.contend(addr);
        let v = self.memory.read(addr);
        self.t += 3;
        v
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.contend(addr);
        self.memory.write(addr, value);
        self.t += 3;
    }

    /// Each T-state of an internal cycle is arbitrated on its own account,
    /// because the address the CPU is holding is on the bus for all of them and
    /// the ULA has no way to tell them apart from a read (ADR-0023).
    fn tick_at(&mut self, addr: u16, t: u32) {
        if !contention::is_contended(addr) {
            self.t += u64::from(t);
            return;
        }
        for _ in 0..t {
            self.stall();
            self.t += 1;
        }
    }

    /// The port is sampled three T-states in rather than one, which is where
    /// the CPU actually latches it — and where the floating bus has to be read
    /// for its byte to be the one the ULA was fetching.
    fn input_cycle(&mut self, port: u16) -> u8 {
        self.port_early(port);
        self.port_late(port);
        let v = self.input(port);
        self.t += 1;
        v
    }

    /// A write is issued one T-state in, where a read is sampled three, because
    /// the CPU puts the data on the bus as soon as `IORQ` goes down. That is
    /// what makes a border stripe land on the scanline the `OUT` was on.
    fn output_cycle(&mut self, port: u16, value: u8) {
        self.port_early(port);
        self.output(port, value);
        self.port_late(port);
        self.t += 1;
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
            // The tape may have run off its end here, in which case it stops
            // driving the line rather than holding it at the last level.
            self.tape.advance_to(self.t);
            self.drive_ear();
        }
        if self.frame_due() {
            self.ula.end_frame();
        }
    }

    /// Put the whole matrix down at once: see [`Keyboard::from_matrix`].
    ///
    /// A frontend sends the state it is holding rather than the change, so
    /// this replaces the keyboard instead of editing it, and a machine whose
    /// window has just lost focus is a matrix of zero like any other.
    ///
    /// It is latched rather than applied, so the keys arrive at the top of the
    /// next frame and not in the middle of whatever scan is running — see
    /// [`Ula::set_keyboard`], which is where the reason is.
    fn set_keys(&mut self, matrix: u64) {
        self.ula.set_keyboard(Keyboard::from_matrix(matrix));
    }

    /// The transport buttons, and the `EAR` line each of them leaves behind.
    ///
    /// A deck with no tape in it does nothing, which is what a real one does
    /// too, and pressing play on a tape already running restarts nothing: the
    /// head is where it is and the next pulse is due when it was due.
    fn tape(&mut self, button: TapeButton) {
        match button {
            TapeButton::Play => self.play_tape(),
            TapeButton::Stop => self.stop_tape(),
            TapeButton::Rewind => {
                self.tape.rewind();
                self.drive_ear();
            }
        }
    }
}
