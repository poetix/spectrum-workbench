//! The shape of a frame, in T-states and in pixels.
//!
//! Everything else in this crate reads its geometry from here, because the two
//! halves of a frame — when the ULA is doing something, and where on the screen
//! it is doing it — are the same numbers seen from two directions. A border
//! write is a T-state that has to become a scanline; the display period is a
//! range of scanlines that has to become a range of T-states.
//!
//! ```text
//!  line    0  +--------------------------+  interrupt raised here, T = 0
//!             |   vertical retrace, 16   |
//!         16  +--------------------------+  first visible line
//!             |     top border, 48       |
//!         64  +----+----------------+----+  first display line, T = 14336
//!             | 48 |   256 x 192    | 48 |
//!        256  +----+----------------+----+
//!             |    bottom border, 56     |
//!        312  +--------------------------+  end of frame, T = 69888
//! ```
//!
//! One line is 224 T-states: 128 of display (two pixels per T-state), 24 of
//! right border, 48 of horizontal retrace and 24 of left border, which is where
//! the 48-pixel side borders come from. The vertical figures work the same way
//! — 312 lines of 224 is 69,888 T-states, and at 3.5 MHz that is 50.08 frames
//! per second, the "50 Hz" everything on this machine is paced by.
//!
//! **These constants are the ones ticket 0020 has to check against a published
//! reference before contention lands.** Nothing here is timing-critical on its
//! own: a frame interrupt at the wrong T-state makes software feel wrong, where
//! contention at the wrong T-state makes it render wrong.

/// The Z80 clock, in Hz.
pub const CLOCK_HZ: u64 = 3_500_000;

/// T-states in one scanline, border and retrace included.
pub const T_STATES_PER_LINE: u64 = 224;

/// Scanlines in one frame, retrace included.
pub const LINES_PER_FRAME: usize = 312;

/// T-states in one frame. 50.08 Hz against [`CLOCK_HZ`].
pub const T_STATES_PER_FRAME: u64 = T_STATES_PER_LINE * LINES_PER_FRAME as u64;

/// How long `INT` stays asserted at the top of the frame. Long enough that the
/// CPU cannot miss it between two instruction boundaries, short enough that a
/// routine which re-enables interrupts inside its own handler does not
/// immediately take a second one.
pub const INTERRUPT_LENGTH: u64 = 32;

/// The frame line the 192 lines of display start on.
pub const FIRST_DISPLAY_LINE: usize = 64;

/// The T-state, measured from the interrupt, at which the ULA starts fetching
/// display bytes. Where contention begins, when there is contention (0020).
pub const FIRST_DISPLAY_T: u64 = FIRST_DISPLAY_LINE as u64 * T_STATES_PER_LINE;

/// The frame line the visible picture starts on: the first line after vertical
/// retrace, and the top of the border a frontend shows.
pub const FIRST_VISIBLE_LINE: usize = 16;

/// Border lines above the display, within the visible picture.
pub const BORDER_TOP: usize = FIRST_DISPLAY_LINE - FIRST_VISIBLE_LINE;

/// Border lines below the display, within the visible picture.
pub const BORDER_BOTTOM: usize = LINES_PER_FRAME - FIRST_DISPLAY_LINE - DISPLAY_HEIGHT;

/// Border pixels either side of the display, within the visible picture.
pub const BORDER_X: usize = 48;

pub const DISPLAY_WIDTH: usize = 256;
pub const DISPLAY_HEIGHT: usize = 192;

/// The visible picture, border included.
pub const WIDTH: usize = DISPLAY_WIDTH + 2 * BORDER_X;
pub const HEIGHT: usize = BORDER_TOP + DISPLAY_HEIGHT + BORDER_BOTTOM;

/// Which line of the frame the ULA is drawing `t` T-states after the
/// interrupt. Saturates at the end of the frame rather than wrapping, so a
/// caller that is a few T-states past the boundary fills the frame it was in
/// rather than the top of the next one.
pub fn line_of(t_into_frame: u64) -> usize {
    ((t_into_frame / T_STATES_PER_LINE) as usize).min(LINES_PER_FRAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_is_fifty_hertz_to_within_a_tenth_of_a_percent() {
        assert_eq!(T_STATES_PER_FRAME, 69_888);
        let frames_per_second = CLOCK_HZ as f64 / T_STATES_PER_FRAME as f64;
        assert!(
            (frames_per_second - 50.0).abs() < 0.1,
            "{frames_per_second} Hz"
        );
    }

    #[test]
    fn the_visible_picture_accounts_for_every_line_that_is_not_retrace() {
        assert_eq!(BORDER_TOP, 48);
        assert_eq!(BORDER_BOTTOM, 56);
        assert_eq!(WIDTH, 352);
        assert_eq!(HEIGHT, 296);
        assert_eq!(FIRST_VISIBLE_LINE + HEIGHT, LINES_PER_FRAME);
    }

    #[test]
    fn the_display_starts_at_the_t_state_contention_will_start_at() {
        assert_eq!(FIRST_DISPLAY_T, 14_336);
        assert_eq!(line_of(FIRST_DISPLAY_T), FIRST_DISPLAY_LINE);
        assert_eq!(line_of(FIRST_DISPLAY_T - 1), FIRST_DISPLAY_LINE - 1);
    }

    #[test]
    fn a_line_that_is_past_the_end_of_the_frame_saturates() {
        assert_eq!(line_of(0), 0);
        assert_eq!(line_of(T_STATES_PER_FRAME), LINES_PER_FRAME);
        assert_eq!(line_of(T_STATES_PER_FRAME * 3), LINES_PER_FRAME);
    }
}
