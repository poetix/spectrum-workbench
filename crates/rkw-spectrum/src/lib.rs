//! The 48K ZX Spectrum: the memory map, the ULA and the screen.
//!
//! The [`z80`] crate knows nothing about this machine and this crate knows
//! nothing about instruction decoding; between them sits [`Bus`](z80::Bus),
//! which the [`Spectrum`] implements. What it adds to a flat 64K is everything
//! the hardware does: ROM that ignores writes, a frame clock, the 50 Hz
//! interrupt that paces every Spectrum program ever written, the border, and
//! the display file.
//!
//! ```
//! use rkw_spectrum::{Flash, Spectrum, screen};
//! use z80::Bus;
//!
//! let mut machine = Spectrum::new();
//! // Ink 7 on paper 0, and a solid byte of pixels in the top-left cell.
//! machine.write(screen::attr_addr(0x4000, 0, 0), 0x07);
//! machine.write(screen::pixel_addr(0x4000, 0, 0), 0xFF);
//!
//! let frame = machine.frame();
//! assert_eq!(frame.pixel(48, 48), 7); // top-left pixel of the display
//! ```
//!
//! # How it is laid out
//!
//! - [`frame`] is the geometry: a frame in T-states and in pixels, and the one
//!   place the two are related.
//! - [`memory`] is the map, which on a 48K machine is the single decision that
//!   writes below `0x4000` do nothing.
//! - [`keyboard`] is the matrix: forty keys as eight half-rows of five, read
//!   through whichever address lines the program drove low.
//! - [`keymap`] is the bridge from a host keyboard to that matrix, as a table
//!   so that layouts can be swapped without touching code.
//! - [`screen`] is the display decode and the framebuffer. It takes a byte
//!   source and a base address rather than a machine, so the debugger's screen
//!   pane, a `.scr` file and the live display all render through it
//!   (ADR-0020).
//! - [`contention`] is the ULA taking the memory bus away from the CPU, and
//!   the byte it leaves behind on the data bus when it does. The two are the
//!   same fact seen from either side, which is why they are one module
//!   (ADR-0009, ADR-0023).
//! - [`ula`] is the frame clock, the interrupt, the flash cadence, the border
//!   — the last of which is recorded per scanline, because that is how
//!   Spectrum software makes timing visible — and the two halves of port
//!   `0xFE`.
//! - [`tape`] is the deck: what is mounted, where the head is, and the `EAR`
//!   line it drives as its pulses go past. What a pulse is belongs to
//!   [`rkw_tape`]; this is the part that has to be inside the machine, because
//!   a loader's timing depends on it (ADR-0022).
//! - [`spectrum`] wires them to the CPU's bus and to the emulation thread's
//!   [`Machine`](rkw_debug::Machine).
//! - [`audio`] wraps that in a machine that also makes a noise, which is where
//!   the beeper's per-frame work runs without becoming machine state
//!   (ADR-0021).
//! - [`save`] wraps it in one that writes down what it puts on the `MIC` line,
//!   for the same reason and in the same place.

pub mod audio;
pub mod contention;
pub mod frame;
pub mod keyboard;
pub mod keymap;
pub mod memory;
pub mod save;
pub mod screen;
pub mod spectrum;
pub mod tape;
pub mod ula;

pub use audio::AudioMachine;
pub use contention::{floating_bus, is_contended};
pub use frame::{
    CLOCK_HZ, HEIGHT, LINES_PER_FRAME, T_STATES_PER_FRAME, T_STATES_PER_LINE, WIDTH, line_of,
};
pub use keyboard::{Key, Keyboard};
pub use keymap::{HostKey, HostKeys, KeyMap};
pub use memory::{Memory, RomTooLarge, SCREEN_BASE};
pub use save::Saving;
pub use screen::{DISPLAY_BYTES, Flash, Framebuffer, PALETTE, decode, decode_into};
pub use spectrum::Spectrum;
pub use tape::{LD_BYTES, Loaded, Tape, ld_bytes};
pub use ula::Ula;
