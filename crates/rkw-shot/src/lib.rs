//! Assemble a program, run it on a Spectrum with nobody watching, and look at
//! what it drew.
//!
//! This is the workbench a game is written on before there is a window to run
//! it in, and it stays useful after there is one: a frame it produces is a
//! value, so a test can assert about the picture, and a border stripe is a
//! measurement, so a test can assert about the time.
//!
//! # Time is measured in border colours
//!
//! A routine that sets the border on the way in and out leaves a stripe down
//! the side of the screen exactly as long as it took, because the ULA records
//! the border colour for every line of the frame (that is what
//! [`rkw_spectrum::Ula::border_lines`] is). Counting lines of each colour turns
//! that into T-states at 224 per line — the same technique a Spectrum
//! programmer uses on real hardware, and here it costs nothing and cannot
//! perturb what it measures.
//!
//! It is coarse: a line is 224 T-states, so a stripe is accurate to about
//! 0.3% of a frame. That is the right resolution for "does the blitter fit in
//! a frame", which is the question this exists to answer.

pub mod png;

use std::collections::HashMap;
use std::path::Path;

use rkw_asm::{SourceMap, assemble};
use rkw_debug::machine::Machine;
use rkw_spectrum::frame::{LINES_PER_FRAME, T_STATES_PER_FRAME, T_STATES_PER_LINE};
use rkw_spectrum::{Framebuffer, Spectrum};
use z80::{Cpu, disasm::Peek};

/// Where the stack goes before the program has said otherwise. A program that
/// sets its own `SP` — every game does — never sees this.
const DEFAULT_SP: u16 = 0xFF00;

/// A CPU, a Spectrum, and the symbols of the program running on them.
pub struct Rig {
    pub cpu: Cpu,
    pub machine: Spectrum,
    /// Every label and constant the assembler settled on, so a caller can say
    /// "the terrain buffer" rather than `0x8000`.
    pub symbols: HashMap<String, u16>,
}

impl Rig {
    /// Assemble `path`, load every segment where it was assembled for, and
    /// start the CPU at the lowest of them.
    pub fn assemble(path: &Path) -> Result<Rig, String> {
        let mut map = SourceMap::new();
        let file = map
            .load(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut assembled = assemble(&mut map, file);
        if assembled.has_errors() {
            let rendered: String = assembled
                .diagnostics
                .iter()
                .map(|d| map.render(d))
                .collect();
            return Err(rendered);
        }

        let mut machine = Spectrum::new();
        for segment in assembled.image.segments() {
            machine.memory.load(segment.origin, &segment.bytes);
        }

        let symbols = assembled
            .symbols
            .iter_values()
            .into_iter()
            .map(|(name, value)| (name, value as u16))
            .collect();

        let mut cpu = Cpu::new();
        cpu.regs.pc = assembled.image.origin().unwrap_or(0);
        cpu.regs.sp = DEFAULT_SP;

        Ok(Rig {
            cpu,
            machine,
            symbols,
        })
    }

    /// The address of a label, or a panic naming it: a test that misspells a
    /// symbol wants to be told so, not to poke address zero.
    pub fn symbol(&self, name: &str) -> u16 {
        *self
            .symbols
            .get(name)
            .unwrap_or_else(|| panic!("no symbol `{name}` in the program"))
    }

    pub fn t_states(&self) -> u64 {
        self.machine.t_states()
    }

    pub fn peek(&self, address: u16) -> u8 {
        self.machine.peek(address)
    }

    pub fn peek_word(&self, address: u16) -> u16 {
        u16::from_le_bytes([self.peek(address), self.peek(address.wrapping_add(1))])
    }

    /// Run until the clock reaches `target`, servicing hardware events on the
    /// way. The slice loop of ADR-0007 in miniature, as the machine tests
    /// write it.
    pub fn run_to(&mut self, target: u64) {
        while self.machine.t_states() < target {
            let event = self
                .machine
                .next_event()
                .expect("the ULA always schedules a frame");
            let deadline = event.min(target);
            while self.machine.t_states() < deadline {
                self.cpu.step(&mut self.machine);
            }
            if self.machine.t_states() >= event {
                self.machine.service_event();
            }
        }
    }

    pub fn run_frames(&mut self, frames: u64) {
        self.run_to(self.machine.t_states() + frames * T_STATES_PER_FRAME);
    }

    /// The picture as it stands now, border included.
    pub fn frame(&self) -> Framebuffer {
        self.machine.frame()
    }

    /// The last complete frame's border, one colour per line of the frame.
    pub fn border_lines(&self) -> &[u8; LINES_PER_FRAME] {
        self.machine.ula.border_lines()
    }

    /// How the last complete frame's time was divided between border colours.
    pub fn profile(&self) -> Profile {
        let mut lines = [0usize; 8];
        for &colour in self.border_lines().iter() {
            lines[usize::from(colour & 0x07)] += 1;
        }
        Profile { lines }
    }
}

/// Lines of each border colour in one frame, and what that is worth in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    lines: [usize; 8],
}

/// The border colours, in the order the hardware numbers them.
pub const COLOUR_NAMES: [&str; 8] = [
    "black", "blue", "red", "magenta", "green", "cyan", "yellow", "white",
];

impl Profile {
    pub fn lines(&self, colour: u8) -> usize {
        self.lines[usize::from(colour & 0x07)]
    }

    /// T-states spent with the border this colour, to the nearest line.
    pub fn t_states(&self, colour: u8) -> u64 {
        self.lines(colour) as u64 * T_STATES_PER_LINE
    }

    /// The share of a frame this colour took, as a percentage.
    pub fn percent(&self, colour: u8) -> f64 {
        100.0 * self.t_states(colour) as f64 / T_STATES_PER_FRAME as f64
    }

    /// One line per colour that appeared at all, longest first.
    pub fn report(&self) -> String {
        let mut colours: Vec<u8> = (0..8).filter(|&c| self.lines(c) > 0).collect();
        colours.sort_by_key(|&c| std::cmp::Reverse(self.lines(c)));
        colours
            .iter()
            .map(|&c| {
                format!(
                    "  {:<8} {:>3} lines  {:>6} T  {:>5.1}%\n",
                    COLOUR_NAMES[usize::from(c)],
                    self.lines(c),
                    self.t_states(c),
                    self.percent(c),
                )
            })
            .collect()
    }
}
