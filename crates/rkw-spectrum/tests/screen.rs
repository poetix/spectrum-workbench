//! What a known screen renders to.
//!
//! The hash tests are regression tests and are not the argument on their own —
//! a hash tells you something changed and nothing about whether it was ever
//! right. So each one is paired with assertions about pixels whose value is
//! derivable by hand from the layout, and the hash exists to catch the
//! thousands of pixels that are not worth naming individually.

use rkw_spectrum::frame::{
    BORDER_TOP, BORDER_X, DISPLAY_HEIGHT, DISPLAY_WIDTH, FIRST_VISIBLE_LINE, HEIGHT,
    LINES_PER_FRAME, T_STATES_PER_LINE, WIDTH,
};
use rkw_spectrum::screen::{ATTRIBUTE_BYTES, COLUMNS, attr_addr, decode, pixel_addr};
use rkw_spectrum::{DISPLAY_BYTES, Flash, Framebuffer, SCREEN_BASE, Spectrum};

/// FNV-1a, so that a hash in a test is a value anyone can recompute rather
/// than a dependency.
fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A display file whose every byte is a function of where it is, so that a
/// decode which transposed rows, mixed up thirds or lost the stride cannot
/// produce the same picture.
fn known_display() -> [u8; DISPLAY_BYTES] {
    let mut bytes = [0; DISPLAY_BYTES];
    for y in 0..DISPLAY_HEIGHT {
        for column in 0..COLUMNS {
            let at = pixel_addr(0, column, y) as usize;
            bytes[at] = (y as u8).wrapping_mul(5) ^ (column as u8).wrapping_mul(37);
        }
    }
    // Every attribute byte there is, three times over: all sixteen ink
    // colours against all sixteen papers, flashing and not.
    for cell in 0..ATTRIBUTE_BYTES {
        bytes[DISPLAY_BYTES - ATTRIBUTE_BYTES + cell] = (cell % 256) as u8;
    }
    bytes
}

fn machine_with_known_display() -> Spectrum {
    let mut machine = Spectrum::new();
    machine.memory.load(SCREEN_BASE, &known_display());
    machine
}

#[test]
fn a_known_screen_renders_to_the_expected_pixels() {
    let bytes = known_display();
    let src: &[u8] = &bytes;
    let pixels = decode(&src, 0x0000, Flash::Normal);

    assert_eq!(pixels.len(), DISPLAY_WIDTH * DISPLAY_HEIGHT);

    // The top-left cell has attribute 0: ink black on paper black, so whatever
    // the pixel byte says, the cell is one colour.
    assert!(pixels[..8].iter().all(|&p| p == 0));

    // Cell (1, 0) has attribute 1: ink blue on paper black. Its first pixel
    // byte is column 1 of row 0, which the pattern above makes 37.
    let byte = 0u8.wrapping_mul(5) ^ 37u8;
    assert_eq!(byte, 37);
    for bit in 0..8 {
        let expected = if byte & (0x80 >> bit) != 0 { 1 } else { 0 };
        assert_eq!(pixels[8 + bit], expected, "bit {bit}");
    }

    assert_eq!(hash(&pixels), 0xa71f_e7ec_2fee_95c5);
}

#[test]
fn a_screen_renders_the_same_from_a_slice_as_from_the_machine() {
    let bytes = known_display();
    let src: &[u8] = &bytes;
    let from_file = decode(&src, 0x0000, Flash::Normal);

    let machine = machine_with_known_display();
    let from_machine = decode(&machine, SCREEN_BASE, Flash::Normal);

    assert_eq!(from_file, from_machine);
}

#[test]
fn a_back_buffer_renders_through_the_same_path_as_the_display() {
    let mut machine = machine_with_known_display();
    // The same image again, 32K higher up, where no ULA will ever look at it.
    machine.memory.load(0xC000, &known_display());

    assert_eq!(
        decode(&machine, SCREEN_BASE, Flash::Normal),
        decode(&machine, 0xC000, Flash::Normal)
    );
}

#[test]
fn the_flash_phase_swaps_only_the_cells_whose_attribute_asks_for_it() {
    let machine = machine_with_known_display();
    let normal = decode(&machine, SCREEN_BASE, Flash::Normal);
    let inverted = decode(&machine, SCREEN_BASE, Flash::Inverted);

    let mut swapped = 0;
    let mut untouched = 0;
    for row in 0..DISPLAY_HEIGHT / 8 {
        for column in 0..COLUMNS {
            let y = row * 8;
            let attr = machine.memory.read(attr_addr(SCREEN_BASE, column, y));
            let at = y * DISPLAY_WIDTH + column * 8;
            let (a, b) = (&normal[at..at + 8], &inverted[at..at + 8]);
            if attr & 0x80 != 0 {
                swapped += 1;
                // Ink and paper have exchanged, so a cell of one colour looks
                // the same and any other cell does not.
                let (ink, paper) = rkw_spectrum::screen::colours(attr, Flash::Normal);
                if ink != paper {
                    assert_ne!(a, b, "attr {attr:#04x}");
                }
            } else {
                untouched += 1;
                assert_eq!(a, b, "attr {attr:#04x}");
            }
        }
    }
    assert_eq!(swapped, ATTRIBUTE_BYTES / 2);
    assert_eq!(untouched, ATTRIBUTE_BYTES / 2);
}

#[test]
fn the_border_is_one_colour_per_scanline_around_the_display() {
    let mut machine = machine_with_known_display();
    // A different colour every eight lines, written at the top of each.
    for line in 0..LINES_PER_FRAME {
        machine
            .ula
            .write_port_fe(line as u64 * T_STATES_PER_LINE, ((line / 8) % 8) as u8);
    }
    machine.ula.end_frame();

    let frame = machine.frame();
    assert_eq!(frame.width(), WIDTH);
    assert_eq!(frame.height(), HEIGHT);

    // A visible line's border is the colour that was in force on that line of
    // the frame, retrace lines included in the numbering.
    for y in 0..HEIGHT {
        let expected = (((FIRST_VISIBLE_LINE + y) / 8) % 8) as u8;
        assert_eq!(frame.pixel(0, y), expected, "line {y}, left");
        assert_eq!(frame.pixel(WIDTH - 1, y), expected, "line {y}, right");
    }

    // The display sits inside the border and is untouched by it.
    let pixels = decode(&machine, SCREEN_BASE, Flash::Normal);
    for y in 0..DISPLAY_HEIGHT {
        for x in 0..DISPLAY_WIDTH {
            assert_eq!(
                frame.pixel(BORDER_X + x, BORDER_TOP + y),
                pixels[y * DISPLAY_WIDTH + x],
                "display pixel ({x}, {y})"
            );
        }
    }

    assert_eq!(hash(frame.pixels()), 0xab7d_1d46_ec1f_fec5);
}

#[test]
fn a_framebuffer_flattens_to_three_bytes_a_pixel() {
    let mut frame = Framebuffer::new();
    frame.draw_border(&[7; LINES_PER_FRAME]);
    let rgb = frame.to_rgb();
    assert_eq!(rgb.len(), WIDTH * HEIGHT * 3);
    assert_eq!(&rgb[..3], &[0xD7, 0xD7, 0xD7]);
}
