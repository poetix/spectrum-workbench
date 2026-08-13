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
//! - [`screen`] is the display decode and the framebuffer. It takes a byte
//!   source and a base address rather than a machine, so the debugger's screen
//!   pane, a `.scr` file and the live display all render through it
//!   (ADR-0020).
//! - [`ula`] is the frame clock, the interrupt, the flash cadence and the
//!   border — the last of which is recorded per scanline, because that is how
//!   Spectrum software makes timing visible.
//! - [`spectrum`] wires them to the CPU's bus and to the emulation thread's
//!   [`Machine`](rkw_debug::Machine).

pub mod frame;
pub mod memory;
pub mod screen;
pub mod spectrum;
pub mod ula;

pub use frame::{HEIGHT, LINES_PER_FRAME, T_STATES_PER_FRAME, T_STATES_PER_LINE, WIDTH, line_of};
pub use memory::{Memory, RomTooLarge, SCREEN_BASE};
pub use screen::{DISPLAY_BYTES, Flash, Framebuffer, PALETTE, decode, decode_into};
pub use spectrum::Spectrum;
pub use ula::Ula;
