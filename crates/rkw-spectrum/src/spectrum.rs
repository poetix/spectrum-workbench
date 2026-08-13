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
//!
//! **The keyboard.** [`Ula::read_port_fe`] says why (ticket 0013).

use rkw_debug::machine::{Clock, Machine};
use z80::Bus;
use z80::disasm::Peek;

use crate::memory::{Memory, RomTooLarge, SCREEN_BASE};
use crate::screen::{Flash, Framebuffer};
use crate::ula::Ula;

/// A 48K Spectrum.
#[derive(Clone)]
pub struct Spectrum {
    pub memory: Memory,
    pub ula: Ula,
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
    /// `0xBFFE` and the rest to select keyboard half-rows (ticket 0013).
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

impl Clock for Spectrum {
    fn t_states(&self) -> u64 {
        self.t
    }
}

impl Machine for Spectrum {
    /// The next frame interrupt. The tape will schedule edges here too (0016),
    /// at which point this becomes the earlier of the two.
    fn next_event(&self) -> Option<u64> {
        Some(self.ula.next_interrupt())
    }

    fn service_event(&mut self) {
        self.ula.end_frame();
    }
}
