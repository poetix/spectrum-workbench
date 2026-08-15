//! Decoding the display file, and painting a frame.
//!
//! # The decode is a function of bytes, not of a machine
//!
//! Everything here takes a [`Peek`] and a base address rather than a running
//! Spectrum. A `Peek` is anything that can answer "the byte at this address" —
//! the machine's memory, a 6912-byte `.scr` file, a closure over a snapshot —
//! so the same code renders the live screen at `0x4000`, a back buffer a game
//! is drawing into, a 128K shadow screen, and a screen out of a saved state.
//! The debugger's screen pane (ticket 0025) is not a special case of this; it
//! is the ordinary case with a different base address.
//!
//! The flash phase is a parameter for the same reason. If the decode read a
//! frame counter, two stops at the same breakpoint would render differently and
//! a test would have to run the machine to a particular frame to get a stable
//! image.
//!
//! # The layout
//!
//! The display file is 6144 bytes of pixels followed by 768 bytes of
//! attributes. The pixel address is not linear — it interleaves, so that the
//! ULA's fetch pattern falls out of address arithmetic:
//!
//! ```text
//!   15 14 13 12 11 10  9  8   7  6  5  4  3  2  1  0
//!  +--+--+--+--+--+--+--+--+ +--+--+--+--+--+--+--+--+
//!  | base  | third | pixel  | | char row  |  column   |
//!  |       | of 3  | row 0-7| |   0-7     |   0-31    |
//! ```
//!
//! So consecutive addresses walk *across* the screen, the next 256 bytes walk
//! down the eight pixel rows of a character cell, and only then does the
//! character row advance. Three thirds of 64 lines each, and the thirds are the
//! top two address bits. Every "which line is this byte on" question in
//! Spectrum programming is this diagram.
//!
//! The attribute area is linear: one byte per 8x8 cell, 32 per character row.

use z80::disasm::Peek;

use crate::frame::{
    BORDER_TOP, BORDER_X, DISPLAY_HEIGHT, DISPLAY_WIDTH, FIRST_VISIBLE_LINE, HEIGHT,
    LINES_PER_FRAME, WIDTH,
};

/// Character cells across the display.
pub const COLUMNS: usize = DISPLAY_WIDTH / 8;

/// Character cells down the display.
pub const ROWS: usize = DISPLAY_HEIGHT / 8;

/// Bytes of pixel data, before the attributes.
pub const PIXEL_BYTES: usize = COLUMNS * DISPLAY_HEIGHT;

/// Bytes of attribute data.
pub const ATTRIBUTE_BYTES: usize = COLUMNS * ROWS;

/// The whole display file: pixels then attributes.
pub const DISPLAY_BYTES: usize = PIXEL_BYTES + ATTRIBUTE_BYTES;

/// Where the flash bit is in an attribute byte.
pub const FLASH_BIT: u8 = 0x80;

/// Where the bright bit is in an attribute byte.
pub const BRIGHT_BIT: u8 = 0x40;

/// Which half of the flash cycle a frame is in.
///
/// A parameter rather than a global: see the module note. [`Flash::Inverted`]
/// swaps ink and paper in every cell whose attribute has [`FLASH_BIT`] set, and
/// leaves every other cell alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flash {
    Normal,
    Inverted,
}

/// The address of the eight pixels at character column `column` on pixel row
/// `y`, in a display file based at `base`.
///
/// `y` is 0 at the top. The three fields of the module diagram, assembled.
pub fn pixel_addr(base: u16, column: usize, y: usize) -> u16 {
    let y = y as u16;
    let offset = ((y & 0xC0) << 5) | ((y & 0x07) << 8) | ((y & 0x38) << 2) | column as u16;
    base.wrapping_add(offset)
}

/// The address of the attribute governing character column `column` on pixel
/// row `y`.
pub fn attr_addr(base: u16, column: usize, y: usize) -> u16 {
    let cell = (y / 8) * COLUMNS + column;
    base.wrapping_add(PIXEL_BYTES as u16 + cell as u16)
}

/// The ink and paper palette indices an attribute byte selects, flash applied.
pub fn colours(attr: u8, flash: Flash) -> (u8, u8) {
    let bright = if attr & BRIGHT_BIT != 0 { 8 } else { 0 };
    let ink = (attr & 0x07) | bright;
    let paper = ((attr >> 3) & 0x07) | bright;
    if flash == Flash::Inverted && attr & FLASH_BIT != 0 {
        (paper, ink)
    } else {
        (ink, paper)
    }
}

/// Decode 256x192 pixels of display into `out`, one palette index per pixel,
/// `stride` bytes per row.
///
/// `out` is the top-left corner of the display area: a caller rendering into a
/// bordered framebuffer passes the offset of the display within it and that
/// framebuffer's stride, and one that wants the bare screen passes a
/// 49,152-byte buffer and a stride of 256.
pub fn decode_into<S: Peek + ?Sized>(
    src: &S,
    base: u16,
    flash: Flash,
    out: &mut [u8],
    stride: usize,
) {
    for y in 0..DISPLAY_HEIGHT {
        let row = &mut out[y * stride..y * stride + DISPLAY_WIDTH];
        for column in 0..COLUMNS {
            let bits = src.peek(pixel_addr(base, column, y));
            let (ink, paper) = colours(src.peek(attr_addr(base, column, y)), flash);
            for bit in 0..8 {
                row[column * 8 + bit] = if bits & (0x80 >> bit) != 0 {
                    ink
                } else {
                    paper
                };
            }
        }
    }
}

/// The bare 256x192 display, without border, as palette indices.
pub fn decode<S: Peek + ?Sized>(src: &S, base: u16, flash: Flash) -> Vec<u8> {
    let mut out = vec![0; DISPLAY_WIDTH * DISPLAY_HEIGHT];
    decode_into(src, base, flash, &mut out, DISPLAY_WIDTH);
    out
}

/// The sixteen colours, as 8-bit RGB.
///
/// Eight hues at two brightnesses, where bright black is black again — the
/// hardware has no separate bright-black level, and the duplicate index is
/// kept rather than folded away because an attribute byte can produce it and a
/// palette that renumbered would stop matching the attribute.
///
/// The non-bright level is 0xD7 rather than 0xC0 or 0xCD: real machines vary,
/// and this is the value most emulators settled on.
pub const PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], // black
    [0x00, 0x00, 0xD7], // blue
    [0xD7, 0x00, 0x00], // red
    [0xD7, 0x00, 0xD7], // magenta
    [0x00, 0xD7, 0x00], // green
    [0x00, 0xD7, 0xD7], // cyan
    [0xD7, 0xD7, 0x00], // yellow
    [0xD7, 0xD7, 0xD7], // white
    [0x00, 0x00, 0x00], // bright black
    [0x00, 0x00, 0xFF], // bright blue
    [0xFF, 0x00, 0x00], // bright red
    [0xFF, 0x00, 0xFF], // bright magenta
    [0x00, 0xFF, 0x00], // bright green
    [0x00, 0xFF, 0xFF], // bright cyan
    [0xFF, 0xFF, 0x00], // bright yellow
    [0xFF, 0xFF, 0xFF], // bright white
];

/// A visible picture: [`WIDTH`] x [`HEIGHT`] palette indices, border included.
///
/// Indices rather than pixels, because the thing a frontend uploads to a
/// texture and the thing a debugger pane draws are the same array under two
/// different palettes, and because one byte per pixel is a quarter of the
/// bandwidth of four (ticket 0025's adapter response is sized for a stop).
/// [`Framebuffer::to_rgb`] is there for callers that want it flattened.
#[derive(Clone)]
pub struct Framebuffer {
    pixels: Box<[u8]>,
}

impl Default for Framebuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framebuffer {
    pub fn new() -> Framebuffer {
        Framebuffer {
            pixels: vec![0; WIDTH * HEIGHT].into_boxed_slice(),
        }
    }

    pub const fn width(&self) -> usize {
        WIDTH
    }

    pub const fn height(&self) -> usize {
        HEIGHT
    }

    /// Row-major palette indices, [`WIDTH`] per row.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixel(&self, x: usize, y: usize) -> u8 {
        self.pixels[y * WIDTH + x]
    }

    /// Paint the display area from a byte source.
    pub fn draw_display<S: Peek + ?Sized>(&mut self, src: &S, base: u16, flash: Flash) {
        let origin = BORDER_TOP * WIDTH + BORDER_X;
        decode_into(src, base, flash, &mut self.pixels[origin..], WIDTH);
    }

    /// Paint the border from one colour per line of the frame.
    ///
    /// `lines` is indexed by frame line, not by visible line, because that is
    /// what the ULA records: the border a program set during vertical retrace
    /// is on lines nobody sees, and cropping here rather than at the recording
    /// end keeps the ULA's array a straightforward log of what happened when.
    pub fn draw_border(&mut self, lines: &[u8; LINES_PER_FRAME]) {
        for y in 0..HEIGHT {
            let colour = lines[FIRST_VISIBLE_LINE + y] & 0x07;
            let row = &mut self.pixels[y * WIDTH..(y + 1) * WIDTH];
            if (BORDER_TOP..BORDER_TOP + DISPLAY_HEIGHT).contains(&y) {
                row[..BORDER_X].fill(colour);
                row[BORDER_X + DISPLAY_WIDTH..].fill(colour);
            } else {
                row.fill(colour);
            }
        }
    }

    /// The same picture as three bytes per pixel, for a frontend that wants to
    /// hand it straight to a texture.
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for &index in self.pixels.iter() {
            out.extend_from_slice(&PALETTE[index as usize]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thirds_are_the_top_two_bits_of_the_offset() {
        assert_eq!(pixel_addr(0x4000, 0, 0), 0x4000);
        assert_eq!(pixel_addr(0x4000, 0, 64), 0x4800);
        assert_eq!(pixel_addr(0x4000, 0, 128), 0x5000);
    }

    #[test]
    fn the_pixel_row_within_a_cell_is_a_page_apart() {
        assert_eq!(pixel_addr(0x4000, 0, 1), 0x4100);
        assert_eq!(pixel_addr(0x4000, 0, 7), 0x4700);
        // Row 8 is the next character row, which is 32 bytes on.
        assert_eq!(pixel_addr(0x4000, 0, 8), 0x4020);
    }

    #[test]
    fn the_last_pixel_byte_and_the_last_attribute_are_where_they_should_be() {
        assert_eq!(
            pixel_addr(0x4000, COLUMNS - 1, DISPLAY_HEIGHT - 1),
            0x4000 + PIXEL_BYTES as u16 - 1
        );
        assert_eq!(
            attr_addr(0x4000, COLUMNS - 1, DISPLAY_HEIGHT - 1),
            0x4000 + DISPLAY_BYTES as u16 - 1
        );
        assert_eq!(DISPLAY_BYTES, 6912);
    }

    #[test]
    fn a_base_address_other_than_the_screen_shifts_the_whole_layout() {
        assert_eq!(pixel_addr(0xC000, 0, 64), 0xC800);
        assert_eq!(attr_addr(0xC000, 0, 0), 0xC000 + PIXEL_BYTES as u16);
    }

    #[test]
    fn bright_is_the_high_half_of_the_palette_and_flash_swaps_only_marked_cells() {
        // Ink 2 (red) on paper 5 (cyan), not bright, not flashing.
        assert_eq!(colours(0b0010_1010, Flash::Normal), (2, 5));
        assert_eq!(colours(0b0010_1010, Flash::Inverted), (2, 5));
        // The same with bright set.
        assert_eq!(colours(0b0110_1010, Flash::Normal), (10, 13));
        // And with flash set, which swaps in the inverted phase only.
        assert_eq!(colours(0b1010_1010, Flash::Normal), (2, 5));
        assert_eq!(colours(0b1010_1010, Flash::Inverted), (5, 2));
    }
}
