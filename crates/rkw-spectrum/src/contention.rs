//! The ULA taking the memory bus away from the CPU, and what the CPU sees when
//! it reads a port nothing is answering.
//!
//! Both of these are the same fact seen from two sides. For 128 T-states of
//! every display line the ULA is fetching the bytes it is about to put on the
//! screen, out of the same 16K of RAM the CPU wants. Where they collide the
//! ULA wins and the CPU is held; and whatever the ULA fetched is left on the
//! data bus, so a read of a port with nothing attached picks it up.
//!
//! # The eight T-state pattern
//!
//! The ULA fetches in pairs — a bitmap byte and its attribute — and does two
//! pairs in every eight T-states, which is one character cell's worth of both.
//! It needs six of those eight to do it, so a CPU access landing on the first
//! of the eight waits six T-states for its turn, one landing on the second
//! waits five, and so on down to the last two, which are free:
//!
//! ```text
//!   T into the group   0  1  2  3  4  5  6  7
//!   CPU waits          6  5  4  3  2  1  0  0
//!   ULA is fetching       B  A  B  A            (bitmap, attribute)
//! ```
//!
//! The row of the table an access lands on is a function of the T-state alone,
//! which is why this is arithmetic and not a 68 KB table (ADR-0009).
//!
//! # What is contended
//!
//! Addresses `0x4000-0x7FFF`, which on a 48K machine is the one bank of RAM
//! wired to the ULA — the display file lives at the bottom of it and the rest
//! shares its fate. And only during the 192 display lines, and within those
//! only the 128 T-states of each line that the ULA spends fetching: the
//! borders and the retrace are free.
//!
//! # Where the numbers come from
//!
//! ADR-0009 recorded these constants as remembered and not yet load-bearing.
//! They are load-bearing now, so they were checked against Fuse — the same
//! project this crate's conformance data and ROM come from (ADR-0005) — at
//! `libspectrum/timings.c`, `fuse/spectrum.c` and `fuse/machines/spec48.c`.
//! Everything in [`crate::frame`] agrees with the 48K row of Fuse's table. The
//! one number that was not already in the tree is [`FIRST_CONTENDED_T`], and it
//! is *not* [`FIRST_DISPLAY_T`]: contention starts one T-state before the ULA's
//! first fetch, because an access beginning on that T-state would still be
//! holding the bus when the ULA wants it.
//!
//! Fuse spells the same rule with the pattern rotated one place and the window
//! shifted one place to match (`{5,4,3,2,1,0,0,6}` from an offset of 1); the
//! two forms give the same delay for every T-state of the frame, and the test
//! at the bottom of this file checks that against Fuse's arithmetic directly
//! rather than against a transcription of its results.

use z80::disasm::Peek;

use crate::frame::{FIRST_DISPLAY_T, T_STATES_PER_FRAME, T_STATES_PER_LINE};
use crate::screen::{attr_addr, pixel_addr};

/// The first address the ULA contends for, and the first byte of the display
/// file.
pub const CONTENDED_BASE: u16 = 0x4000;

/// One past the last contended address. The 48K machine's other two banks are
/// on the far side of the ULA and are never held.
pub const CONTENDED_END: u16 = 0x8000;

/// T-states of each line the ULA spends fetching, out of
/// [`T_STATES_PER_LINE`]. Two pixels per T-state across 256 pixels.
pub const FETCH_T_STATES: u64 = 128;

/// Display lines, and so contended lines.
pub const CONTENDED_LINES: u64 = 192;

/// How long the CPU waits, by where in the ULA's eight T-state fetch group its
/// access falls.
pub const PATTERN: [u8; 8] = [6, 5, 4, 3, 2, 1, 0, 0];

/// The first T-state of the frame on which an access to contended memory is
/// delayed — one before [`FIRST_DISPLAY_T`], and the origin of [`PATTERN`].
pub const FIRST_CONTENDED_T: u64 = FIRST_DISPLAY_T - 1;

/// True for the addresses the ULA arbitrates for.
///
/// This is the whole of the decode: a 48K machine has no paging, so the bank
/// is a range and not a lookup.
#[inline]
pub fn is_contended(addr: u16) -> bool {
    (CONTENDED_BASE..CONTENDED_END).contains(&addr)
}

/// How many T-states an access to contended memory is delayed by, for an
/// access starting `t` T-states into the frame.
///
/// `t` is measured from the frame interrupt and may run past the end of the
/// frame, which is how a slice that overran its deadline is accounted for.
#[inline]
pub fn delay(t: u64) -> u32 {
    let t = t % T_STATES_PER_FRAME;
    if t < FIRST_CONTENDED_T {
        return 0;
    }
    let since = t - FIRST_CONTENDED_T;
    if since >= CONTENDED_LINES * T_STATES_PER_LINE {
        return 0;
    }
    let into_line = since % T_STATES_PER_LINE;
    if into_line >= FETCH_T_STATES {
        return 0;
    }
    PATTERN[(into_line % 8) as usize] as u32
}

/// The byte the ULA is putting on the data bus `t` T-states into the frame,
/// which is what a read of an unattached port picks up.
///
/// Outside the fetch — the borders, the retrace, and the two idle T-states of
/// every group of eight — nothing is driving the bus and it floats high, so the
/// answer is `0xFF`. That is what makes the effect usable: a program that spins
/// on `IN A,(0xFF)` waiting for the value to stop being `0xFF` has synchronised
/// itself to the beam without a single interrupt, which is how a good deal of
/// software gets a tear-free screen.
///
/// Reading `src` rather than a `Spectrum` for the same reason [`crate::screen`]
/// does: what the ULA fetches is a function of the display file's bytes and its
/// base address, not of a whole machine.
pub fn floating_bus<S: Peek + ?Sized>(src: &S, base: u16, t: u64) -> u8 {
    const IDLE: u8 = 0xFF;

    let t = t % T_STATES_PER_FRAME;
    if t < FIRST_DISPLAY_T {
        return IDLE;
    }
    let since = t - FIRST_DISPLAY_T;

    let line = since / T_STATES_PER_LINE;
    if line >= CONTENDED_LINES {
        return IDLE;
    }
    let into_line = since % T_STATES_PER_LINE;
    if into_line >= FETCH_T_STATES {
        return IDLE;
    }

    // Two cells' worth of fetch in every eight T-states, in the order the ULA
    // needs them: bitmap then attribute, twice.
    let column = (into_line / 8) as usize * 2;
    let line = line as usize;
    match into_line % 8 {
        2 => src.peek(pixel_addr(base, column, line)),
        3 => src.peek(attr_addr(base, column, line)),
        4 => src.peek(pixel_addr(base, column + 1, line)),
        5 => src.peek(attr_addr(base, column + 1, line)),
        _ => IDLE,
    }
}

/// The last T-state of the frame on which anything is contended, for tests and
/// for a caller that wants to know whether the beam is past the display.
pub const LAST_CONTENDED_T: u64 =
    FIRST_CONTENDED_T + (CONTENDED_LINES - 1) * T_STATES_PER_LINE + FETCH_T_STATES - 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FIRST_DISPLAY_LINE, LINES_PER_FRAME};
    use crate::memory::{Memory, SCREEN_BASE};

    /// Fuse's own arithmetic, transcribed from `contend_delay_common` in
    /// `fuse/spectrum.c` with the 48K row of `libspectrum/timings.c` and
    /// `DISPLAY_BORDER_HEIGHT` / `DISPLAY_BORDER_WIDTH_COLS` from
    /// `fuse/display.h` substituted in.
    ///
    /// Kept in the shape Fuse wrote it rather than simplified, because the
    /// point of it is to be recognisable as the reference — which is also why
    /// the range check below is left as two comparisons.
    #[allow(clippy::manual_range_contains)]
    fn fuse_delay(time: i64) -> u32 {
        const PATTERN_65432100: [u32; 8] = [5, 4, 3, 2, 1, 0, 0, 6];
        const TSTATES_PER_LINE: i64 = 224;
        const LEFT_BORDER: i64 = 24;
        const HORIZONTAL_SCREEN: i64 = 128;
        const DISPLAY_BORDER_HEIGHT: i64 = 24;
        const DISPLAY_BORDER_WIDTH_COLS: i64 = 4;
        const DISPLAY_HEIGHT: i64 = 192;
        const TOP_LEFT_PIXEL: i64 = 14336;
        const OFFSET: i64 = 1;

        let line_times_0 = TOP_LEFT_PIXEL
            - DISPLAY_BORDER_HEIGHT * TSTATES_PER_LINE
            - 4 * DISPLAY_BORDER_WIDTH_COLS;

        let line = (time - line_times_0).div_euclid(TSTATES_PER_LINE);
        let through_line = (time - line_times_0 + (LEFT_BORDER - DISPLAY_BORDER_WIDTH_COLS * 4))
            .rem_euclid(TSTATES_PER_LINE);

        if line < DISPLAY_BORDER_HEIGHT || line >= DISPLAY_BORDER_HEIGHT + DISPLAY_HEIGHT {
            return 0;
        }
        if through_line < LEFT_BORDER - OFFSET {
            return 0;
        }
        if through_line >= LEFT_BORDER + HORIZONTAL_SCREEN - OFFSET {
            return 0;
        }
        PATTERN_65432100[(through_line % 8) as usize]
    }

    #[test]
    fn the_delay_agrees_with_fuse_for_every_t_state_of_the_frame() {
        for t in 0..T_STATES_PER_FRAME {
            assert_eq!(
                delay(t),
                fuse_delay(t as i64),
                "T {t}, line {}, {} into line",
                t / T_STATES_PER_LINE,
                t % T_STATES_PER_LINE
            );
        }
    }

    #[test]
    fn contention_starts_one_t_state_before_the_first_fetch() {
        assert_eq!(FIRST_CONTENDED_T, 14_335);
        assert_eq!(FIRST_DISPLAY_T, 14_336);
        assert_eq!(delay(FIRST_CONTENDED_T - 1), 0);
        assert_eq!(delay(FIRST_CONTENDED_T), 6);
        assert_eq!(delay(FIRST_DISPLAY_T), 5);
    }

    #[test]
    fn each_line_is_contended_for_a_hundred_and_twenty_eight_t_states() {
        let line = FIRST_CONTENDED_T + 100 * T_STATES_PER_LINE;
        assert_eq!(delay(line + FETCH_T_STATES - 1), 0); // the last of the 6,5,4,3,2,1,0,0
        assert_eq!(delay(line + FETCH_T_STATES - 3), 1);
        assert_eq!(delay(line + FETCH_T_STATES), 0);
        assert_eq!(delay(line + T_STATES_PER_LINE - 1), 0);
        assert_eq!(delay(line + T_STATES_PER_LINE), 6);

        let contended = (0..T_STATES_PER_LINE)
            .filter(|i| delay(line + i) > 0)
            .count();
        // Two of every eight are free even inside the fetch.
        assert_eq!(contended, (FETCH_T_STATES as usize / 8) * 6);
    }

    #[test]
    fn nothing_is_contended_outside_the_display_lines() {
        for t in 0..FIRST_CONTENDED_T {
            assert_eq!(delay(t), 0, "T {t}");
        }
        for t in LAST_CONTENDED_T + 1..T_STATES_PER_FRAME {
            assert_eq!(delay(t), 0, "T {t}");
        }
        assert_eq!(LAST_CONTENDED_T, 57_246);
        assert_eq!(delay(LAST_CONTENDED_T), 0); // the eighth of its group
        assert_eq!(delay(LAST_CONTENDED_T - 7), 6);
    }

    #[test]
    fn the_frame_repeats() {
        for t in 0..1000 {
            assert_eq!(delay(t), delay(t + T_STATES_PER_FRAME));
            assert_eq!(delay(t), delay(t + 7 * T_STATES_PER_FRAME));
        }
    }

    #[test]
    fn only_the_bottom_bank_is_contended() {
        assert!(!is_contended(0x0000));
        assert!(!is_contended(0x3FFF));
        assert!(is_contended(0x4000));
        assert!(is_contended(0x7FFF));
        assert!(!is_contended(0x8000));
        assert!(!is_contended(0xFFFF));
    }

    /// A display file whose every byte is its own low address byte, so that a
    /// floating-bus read names the address it came from.
    fn marked_memory() -> Memory {
        let mut mem = Memory::new();
        for addr in SCREEN_BASE..SCREEN_BASE + crate::screen::DISPLAY_BYTES as u16 {
            mem.poke(addr, (addr & 0xFF) as u8);
        }
        mem
    }

    #[test]
    fn the_floating_bus_hands_back_two_pairs_of_bytes_every_eight_t_states() {
        let mem = marked_memory();
        let at = |t| floating_bus(&mem, SCREEN_BASE, t);
        let byte_at = |addr: u16| (addr & 0xFF) as u8;

        // The first group of eight, at the top-left corner of the display.
        assert_eq!(at(FIRST_DISPLAY_T), 0xFF);
        assert_eq!(at(FIRST_DISPLAY_T + 1), 0xFF);
        assert_eq!(
            at(FIRST_DISPLAY_T + 2),
            byte_at(pixel_addr(SCREEN_BASE, 0, 0))
        );
        assert_eq!(
            at(FIRST_DISPLAY_T + 3),
            byte_at(attr_addr(SCREEN_BASE, 0, 0))
        );
        assert_eq!(
            at(FIRST_DISPLAY_T + 4),
            byte_at(pixel_addr(SCREEN_BASE, 1, 0))
        );
        assert_eq!(
            at(FIRST_DISPLAY_T + 5),
            byte_at(attr_addr(SCREEN_BASE, 1, 0))
        );
        assert_eq!(at(FIRST_DISPLAY_T + 6), 0xFF);
        assert_eq!(at(FIRST_DISPLAY_T + 7), 0xFF);

        // The next group is the next two cells along.
        assert_eq!(
            at(FIRST_DISPLAY_T + 10),
            byte_at(pixel_addr(SCREEN_BASE, 2, 0))
        );
        assert_eq!(
            at(FIRST_DISPLAY_T + 12),
            byte_at(pixel_addr(SCREEN_BASE, 3, 0))
        );

        // The last cell of the line, and then the border.
        let last = FIRST_DISPLAY_T + FETCH_T_STATES - 8;
        assert_eq!(at(last + 4), byte_at(pixel_addr(SCREEN_BASE, 31, 0)));
        assert_eq!(at(last + 5), byte_at(attr_addr(SCREEN_BASE, 31, 0)));
        assert_eq!(at(FIRST_DISPLAY_T + FETCH_T_STATES), 0xFF);
    }

    #[test]
    fn the_floating_bus_walks_down_the_screen_a_line_at_a_time() {
        let mem = marked_memory();
        let byte_at = |addr: u16| (addr & 0xFF) as u8;
        for line in [0usize, 1, 7, 8, 63, 64, 191] {
            let t = FIRST_DISPLAY_T + line as u64 * T_STATES_PER_LINE + 2;
            assert_eq!(
                floating_bus(&mem, SCREEN_BASE, t),
                byte_at(pixel_addr(SCREEN_BASE, 0, line)),
                "pixel line {line}"
            );
        }
    }

    #[test]
    fn the_bus_is_idle_in_the_borders_and_the_retrace() {
        let mem = marked_memory();
        let idle = |t| assert_eq!(floating_bus(&mem, SCREEN_BASE, t), 0xFF, "T {t}");

        idle(0);
        idle(FIRST_DISPLAY_T - 1);
        // The right border, retrace and left border of a display line.
        for i in FETCH_T_STATES..T_STATES_PER_LINE {
            idle(FIRST_DISPLAY_T + 40 * T_STATES_PER_LINE + i);
        }
        // Below the display.
        idle(FIRST_DISPLAY_T + CONTENDED_LINES * T_STATES_PER_LINE + 2);
        idle(T_STATES_PER_FRAME - 1);
    }

    #[test]
    fn the_display_period_is_where_the_frame_module_says_it_is() {
        assert_eq!(
            FIRST_DISPLAY_T,
            FIRST_DISPLAY_LINE as u64 * T_STATES_PER_LINE
        );
        assert_eq!(
            FIRST_DISPLAY_T + CONTENDED_LINES * T_STATES_PER_LINE,
            (FIRST_DISPLAY_LINE as u64 + CONTENDED_LINES) * T_STATES_PER_LINE
        );
        assert!(FIRST_DISPLAY_LINE as u64 + CONTENDED_LINES <= LINES_PER_FRAME as u64);
    }
}
