//! Running a real ROM: the fixture, the slice loop, a keyboard that types, and
//! reading the screen back as text.
//!
//! # Reading the screen as text rather than as an image
//!
//! A boot test wants to assert that the copyright message is on the screen. The
//! obvious way is a reference image, and the obvious way is wrong twice over: a
//! picture of the ROM's own output is the ROM's copyrightable content in the
//! tree, and a failure reports "40,000 pixels differ" when what happened was one
//! wrong character.
//!
//! So the screen is read back through the ROM's own font. Characters 32 to 127
//! live at [`GLYPH_BASE`] as eight bytes each, the display is character-cell
//! aligned, and matching each cell against that table turns the screen into
//! twenty-four lines of text — which is what the test actually means, prints
//! legibly when it fails, and stays true if the palette or the border geometry
//! changes. The framebuffer hash covers what this cannot: the pixels, the
//! border, and everything that is not in a cell.
//!
//! # Typing
//!
//! Nothing in the machine buffers a keypress. The ROM samples the matrix in its
//! interrupt routine once a frame and debounces in software, so a key that
//! appears and vanishes between two frames is a key that was never pressed. A
//! keystroke is therefore a *duration*: hold for [`HOLD_FRAMES`], release for
//! [`GAP_FRAMES`], and the gap matters as much as the hold because the ROM
//! ignores a key that has not been let go of since the last one.

use std::collections::HashMap;
use std::path::PathBuf;

use rkw_debug::machine::Machine;
use rkw_spectrum::frame::T_STATES_PER_FRAME;
use rkw_spectrum::screen::{COLUMNS, ROWS, pixel_addr};
use rkw_spectrum::{Framebuffer, Key, SCREEN_BASE, Spectrum};
use z80::Cpu;

/// Where the ROM's font starts. The `CHARS` system variable holds `0x3C00`,
/// which is this less the 256 bytes of the thirty-two characters below space
/// that the font does not contain — the ROM indexes it as `CHARS + code * 8`.
pub const GLYPH_BASE: u16 = 0x3D00;

/// The first character the font has a glyph for.
pub const FIRST_GLYPH: u8 = 32;

/// Characters in the font: space to `©` inclusive.
pub const GLYPH_COUNT: usize = 96;

/// Frames a key is held down for. Two would do — the ROM wants to see the same
/// key twice running before it believes it — and three is one frame of margin
/// against a keystroke landing awkwardly across the interrupt.
pub const HOLD_FRAMES: u64 = 3;

/// Frames every key is up for between keystrokes.
///
/// Eight because of `KSTATE` at `$5C00`, the ROM's two-set debounce. A set is
/// claimed by a key and freed only when its countdown — 5, decremented once per
/// interrupt — runs out, so it outlives the release by five frames, and a press
/// of the *same* key inside that window is treated as a repeat of the set and
/// emits nothing until `REPDEL`, which is 35.
///
/// At three frames of gap that swallows the second `L` of `HELLO`, and the `"`
/// after `PRINT`: SYMBOL SHIFT and `P` scans as the `P` key, and the set is
/// still holding `P`. Eight clears the countdown and stays well under `REPDEL`.
pub const GAP_FRAMES: u64 = 8;

/// The 48K ROM, or `None` if nobody has fetched it.
///
/// `$RKW_48K_ROM` wins over the fixture, so a ROM that is already on the
/// machine somewhere does not have to be copied into the tree.
pub fn rom() -> Option<Vec<u8>> {
    let path = match std::env::var_os("RKW_48K_ROM") {
        Some(path) => PathBuf::from(path),
        None => rom_path(),
    };
    std::fs::read(path).ok()
}

/// Where the fixture goes.
pub fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/48.rom")
}

/// The message a test prints when it skips for want of a ROM.
pub fn skip_message() -> String {
    format!(
        "skipping: no 48K ROM at {}\n\
         run scripts/fetch-rom.sh to install one, or set $RKW_48K_ROM",
        rom_path().display()
    )
}

/// A CPU and a machine, running together.
///
/// The slice loop is `rkw_debug::Emu`'s, in the same miniature form the
/// machine tests use: this crate does not depend on the debugger's session
/// machinery, and a boot test that went through a `Session` would be testing
/// two things at once.
pub struct Board {
    pub cpu: Cpu,
    pub machine: Spectrum,
}

impl Board {
    /// A machine at reset with `rom` in it: `PC` at `0x0000`, interrupts
    /// disabled, `IM 0` — which is what the Z80 comes up in and what the ROM's
    /// first instructions expect.
    pub fn new(rom: &[u8]) -> Board {
        let mut cpu = Cpu::new();
        cpu.reset();
        Board {
            cpu,
            machine: Spectrum::with_rom(rom).expect("ROM image fits the map"),
        }
    }

    /// Run until the clock reaches `target`, servicing frame interrupts on the
    /// way.
    pub fn run_to(&mut self, target: u64) {
        while self.machine.t_states() < target {
            let event = self.machine.next_event().expect("the ULA schedules frames");
            let deadline = event.min(target);
            while self.machine.t_states() < deadline {
                self.cpu.step(&mut self.machine);
            }
            if self.machine.t_states() >= event {
                self.machine.service_event();
            }
        }
    }

    /// Run for `frames` frames.
    pub fn run_frames(&mut self, frames: u64) {
        self.run_to(self.machine.t_states() + frames * T_STATES_PER_FRAME);
    }

    /// Press `keys`, hold them for [`HOLD_FRAMES`], then let go for
    /// [`GAP_FRAMES`].
    pub fn press(&mut self, keys: &[Key]) {
        for key in keys {
            self.machine.ula.keyboard.press(*key);
        }
        self.run_frames(HOLD_FRAMES);
        for key in keys {
            self.machine.ula.keyboard.release(*key);
        }
        self.run_frames(GAP_FRAMES);
    }

    /// Type a string, a keystroke at a time.
    ///
    /// What a character *does* is the ROM's business and not this function's: at
    /// the `K` cursor a letter is a keyword, so typing `"P"` at the start of a
    /// line enters `PRINT`, which is exactly how one types a BASIC program on
    /// this machine.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(&keys_for(c));
        }
    }

    /// The display as twenty-four lines of thirty-two characters, trailing
    /// spaces trimmed.
    pub fn screen_text(&self) -> Vec<String> {
        let font = self.font();
        (0..ROWS)
            .map(|row| {
                let line: String = (0..COLUMNS)
                    .map(|column| {
                        let cell = self.cell(column, row);
                        font.get(&cell)
                            .or_else(|| font.get(&invert(cell)))
                            .copied()
                            .unwrap_or(if cell == [0; 8] { ' ' } else { '?' })
                    })
                    .collect();
                line.trim_end().to_string()
            })
            .collect()
    }

    /// Every line of [`Board::screen_text`] joined, for a test that only wants
    /// to know whether something is on the screen at all.
    pub fn screen_contains(&self, text: &str) -> bool {
        self.screen_text().iter().any(|line| line.contains(text))
    }

    /// The eight pixel bytes of one character cell.
    fn cell(&self, column: usize, row: usize) -> [u8; 8] {
        let mut bytes = [0; 8];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = self
                .machine
                .memory
                .read(pixel_addr(SCREEN_BASE, column, row * 8 + i));
        }
        bytes
    }

    /// The ROM's font, as a lookup from glyph to character.
    ///
    /// Two characters share a glyph in the 48K font — `"` is not one of them —
    /// so the map is one-to-one in practice; where it would not be, the first
    /// character wins and the test that cared would be reading the wrong one
    /// either way.
    fn font(&self) -> HashMap<[u8; 8], char> {
        let mut font = HashMap::with_capacity(GLYPH_COUNT);
        for index in 0..GLYPH_COUNT {
            let code = FIRST_GLYPH + index as u8;
            let base = GLYPH_BASE + (index as u16) * 8;
            let mut glyph = [0; 8];
            for (i, byte) in glyph.iter_mut().enumerate() {
                *byte = self.machine.memory.read(base + i as u16);
            }
            font.entry(glyph).or_insert(char_of(code));
        }
        font
    }
}

/// The Spectrum's character set is ASCII up to `0x7F`, where `©` sits in place
/// of the delete control code.
fn char_of(code: u8) -> char {
    if code == 0x7F { '©' } else { code as char }
}

/// A cell in inverse video, which is how the ROM draws the cursor and the
/// bottom-line prompts.
fn invert(cell: [u8; 8]) -> [u8; 8] {
    cell.map(|byte| !byte)
}

/// The keys held down to type `c`.
///
/// Only the part of the keyboard a BASIC line needs: this is a test helper, not
/// [`rkw_spectrum::KeyMap`], which is where a host key layout belongs.
pub fn keys_for(c: char) -> Vec<Key> {
    #[rustfmt::skip]
    let letters = [
        Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H, Key::I,
        Key::J, Key::K, Key::L, Key::M, Key::N, Key::O, Key::P, Key::Q, Key::R,
        Key::S, Key::T, Key::U, Key::V, Key::W, Key::X, Key::Y, Key::Z,
    ];
    #[rustfmt::skip]
    let digits = [
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4,
        Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
    ];
    match c {
        'A'..='Z' => vec![letters[c as usize - 'A' as usize]],
        'a'..='z' => vec![letters[c as usize - 'a' as usize]],
        '0'..='9' => vec![digits[c as usize - '0' as usize]],
        ' ' => vec![Key::Space],
        '\n' => vec![Key::Enter],
        // The punctuation a `PRINT` statement needs, which is all on SYMBOL
        // SHIFT.
        '"' => vec![Key::SymbolShift, Key::P],
        ';' => vec![Key::SymbolShift, Key::O],
        ',' => vec![Key::SymbolShift, Key::N],
        '+' => vec![Key::SymbolShift, Key::K],
        other => panic!("no key for {other:?}"),
    }
}

/// FNV-1a over the palette indices of a frame.
///
/// A hash rather than a stored image, because the image would be the ROM's
/// output and this repository does not carry the ROM. One number is a poor
/// error message, which is why the text assertions exist alongside it; what it
/// adds is coverage of every pixel that is not in a character cell.
pub fn hash(frame: &Framebuffer) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &pixel in frame.pixels() {
        h ^= u64::from(pixel);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
