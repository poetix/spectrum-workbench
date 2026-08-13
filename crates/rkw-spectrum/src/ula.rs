//! The ULA: the frame clock, the interrupt, the border and port `0xFE`.
//!
//! The ULA is the whole of the Spectrum that is not the Z80 and the RAM. This
//! module is the part of it that ticket 0012 needs: what time it is within the
//! frame, the 50 Hz interrupt that starts each one, the flash cadence, and the
//! border colour. The keyboard half of port `0xFE` is ticket 0013, the speaker
//! bit is 0014, and contention is 0020 — all three read state this already
//! keeps.
//!
//! # The border is a log, not a value
//!
//! Storing "the border is red" would be wrong in the way that matters. The
//! border is drawn by a beam that is somewhere specific when the `OUT` happens,
//! so a program that writes a different colour every scanline gets stripes, and
//! that trick — timing made visible — is how a great deal of Spectrum software
//! signals what it is doing. So a write records a colour *against a scanline*,
//! and the frame the frontend gets is one colour per line rather than one
//! colour.
//!
//! Recording is a catch-up: a write fills every line since the last one with
//! the colour that was in force, then takes effect from the current line. The
//! cost is a fill of a few bytes per `OUT` to `0xFE`, which is nothing next to
//! a per-T-state log, and nothing is allocated (ADR-0007).
//!
//! Sub-scanline border effects — two colours on one line — are not modelled.
//! They exist, they need the T-state position within the line, and they need
//! contention (0020) to be worth having.
//!
//! # The interrupt is derived, not raised
//!
//! `INT` is asserted for [`INTERRUPT_LENGTH`] T-states at the top of every
//! frame, and [`Ula::interrupt_pending`] works that out from the clock instead
//! of setting and clearing a flag. Two things fall out of that: nothing has to
//! run to keep the interrupt line honest, so a machine which was stepped
//! through a frame boundary in the debugger sees the same interrupt a
//! free-running one does; and the CPU accepting the interrupt does not clear
//! the line, exactly as on the hardware, where what stops a second one being
//! taken is `IFF1` going down.

use crate::frame::{INTERRUPT_LENGTH, LINES_PER_FRAME, T_STATES_PER_FRAME, line_of};

/// Frames per half of the flash cycle: ink and paper swap for 16 frames, then
/// swap back, so the whole cycle is 32 frames and takes about two thirds of a
/// second.
pub const FLASH_FRAMES: u64 = 16;

use crate::screen::Flash;

/// The border and the frame clock.
#[derive(Clone)]
pub struct Ula {
    /// The last byte written to port `0xFE`, whole. Bits 0-2 are the border,
    /// bit 3 the MIC output and bit 4 the speaker (ticket 0014).
    port_fe: u8,
    /// The border colour in force now, which is the low three bits of the
    /// above kept apart so the fill loop does not mask on every line.
    border: u8,
    /// One colour per line of the frame in progress. Only the first `filled`
    /// entries mean anything.
    lines: [u8; LINES_PER_FRAME],
    filled: usize,
    /// The last complete frame's border, which is what gets rendered. Kept
    /// separately so that rendering part way through a frame shows the frame
    /// before rather than half a frame and half a stale one.
    presented: [u8; LINES_PER_FRAME],
    /// The T-state at which the current frame's interrupt was raised.
    frame_start: u64,
    frames: u64,
}

impl Default for Ula {
    fn default() -> Self {
        Self::new()
    }
}

impl Ula {
    /// A ULA at the top of frame zero, with a black border — which is not what
    /// a real machine shows at power-on, because there the ROM sets the border
    /// white before anything else happens.
    pub fn new() -> Ula {
        Ula {
            port_fe: 0,
            border: 0,
            lines: [0; LINES_PER_FRAME],
            filled: 0,
            presented: [0; LINES_PER_FRAME],
            frame_start: 0,
            frames: 0,
        }
    }

    /// A write to port `0xFE` at T-state `t`.
    pub fn write_port_fe(&mut self, t: u64, value: u8) {
        self.port_fe = value;
        let colour = value & 0x07;
        if colour != self.border {
            self.fill_to(self.line_now(t));
            self.border = colour;
        }
    }

    /// What a read of port `0xFE` returns.
    ///
    /// Every bit of it belongs to a later ticket: bits 0-4 are the keyboard
    /// half-row selected by the address lines (0013) and bit 6 is the EAR input
    /// the tape loader reads (0016). Until then no key is down and the tape is
    /// silent, which is all ones.
    pub fn read_port_fe(&self, _port: u16) -> u8 {
        0xFF
    }

    /// The last byte written to port `0xFE`.
    pub fn port_fe(&self) -> u8 {
        self.port_fe
    }

    /// The border colour in force now, 0-7.
    pub fn border(&self) -> u8 {
        self.border
    }

    /// The speaker bit, bit 4 of the last port `0xFE` write (ticket 0014).
    pub fn speaker(&self) -> bool {
        self.port_fe & 0x10 != 0
    }

    /// The MIC bit, bit 3, which is what tape saving drives (ticket 0016).
    pub fn mic(&self) -> bool {
        self.port_fe & 0x08 != 0
    }

    /// True while `INT` is asserted, which is for [`INTERRUPT_LENGTH`]
    /// T-states from the top of each frame.
    pub fn interrupt_pending(&self, t: u64) -> bool {
        t.wrapping_sub(self.frame_start) < INTERRUPT_LENGTH
    }

    /// The T-state of the next frame interrupt: the scheduled event the
    /// emulation thread bounds its slices by.
    pub fn next_interrupt(&self) -> u64 {
        self.frame_start + T_STATES_PER_FRAME
    }

    /// The T-state the current frame started at.
    pub fn frame_start(&self) -> u64 {
        self.frame_start
    }

    /// Frames completed since the machine was made.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Which half of the flash cycle the frame in progress is in.
    pub fn flash(&self) -> Flash {
        if (self.frames / FLASH_FRAMES) % 2 == 0 {
            Flash::Normal
        } else {
            Flash::Inverted
        }
    }

    /// The border of the last complete frame, one colour per line of the
    /// frame — retrace lines included, so the index is a frame line and not a
    /// visible one.
    pub fn border_lines(&self) -> &[u8; LINES_PER_FRAME] {
        &self.presented
    }

    /// The clock has reached [`Ula::next_interrupt`]: finish the frame, present
    /// it, and start the next one.
    ///
    /// The new frame starts at the scheduled T-state rather than at whatever
    /// the clock has actually reached, because the last instruction of a slice
    /// runs past its deadline by up to twenty-odd T-states and a frame clock
    /// that took its start from that would drift by however much of the frame
    /// the emulated program happened to spend in long instructions.
    pub fn end_frame(&mut self) {
        self.fill_to(LINES_PER_FRAME);
        self.presented = self.lines;
        self.filled = 0;
        self.frame_start += T_STATES_PER_FRAME;
        self.frames += 1;
    }

    /// The line the beam is on at `t`, saturating at the end of the frame so
    /// that a write a few T-states past the boundary lands on the last line of
    /// the frame it was issued in.
    fn line_now(&self, t: u64) -> usize {
        line_of(t.saturating_sub(self.frame_start))
    }

    /// Record the colour in force for every line up to but not including
    /// `line`.
    fn fill_to(&mut self, line: usize) {
        if line > self.filled {
            self.lines[self.filled..line].fill(self.border);
            self.filled = line;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::T_STATES_PER_LINE;

    fn frame_of(ula: &mut Ula) -> [u8; LINES_PER_FRAME] {
        ula.end_frame();
        *ula.border_lines()
    }

    #[test]
    fn a_border_write_takes_effect_from_the_line_it_was_made_on() {
        let mut ula = Ula::new();
        ula.write_port_fe(0, 1);
        ula.write_port_fe(100 * T_STATES_PER_LINE, 2);
        ula.write_port_fe(200 * T_STATES_PER_LINE, 6);
        let frame = frame_of(&mut ula);

        assert_eq!(frame[0], 1);
        assert_eq!(frame[99], 1);
        assert_eq!(frame[100], 2);
        assert_eq!(frame[199], 2);
        assert_eq!(frame[200], 6);
        assert_eq!(frame[LINES_PER_FRAME - 1], 6);
    }

    #[test]
    fn the_last_write_on_a_line_is_the_one_that_line_gets() {
        let mut ula = Ula::new();
        let line = 40 * T_STATES_PER_LINE;
        ula.write_port_fe(line, 1);
        ula.write_port_fe(line + 10, 2);
        ula.write_port_fe(line + 20, 3);
        let frame = frame_of(&mut ula);

        assert_eq!(frame[39], 0);
        assert_eq!(frame[40], 3);
        assert_eq!(frame[41], 3);
    }

    #[test]
    fn the_border_carries_over_into_the_next_frame() {
        let mut ula = Ula::new();
        ula.write_port_fe(T_STATES_PER_LINE * 8, 5);
        frame_of(&mut ula);
        let next = frame_of(&mut ula);
        assert!(next.iter().all(|&c| c == 5), "{:?}", &next[..4]);
    }

    #[test]
    fn only_the_low_three_bits_are_the_border() {
        let mut ula = Ula::new();
        ula.write_port_fe(0, 0xFF);
        assert_eq!(ula.border(), 7);
        assert_eq!(ula.port_fe(), 0xFF);
        assert!(ula.speaker());
        assert!(ula.mic());
    }

    #[test]
    fn the_interrupt_is_asserted_for_thirty_two_t_states_every_frame() {
        let mut ula = Ula::new();
        assert!(ula.interrupt_pending(0));
        assert!(ula.interrupt_pending(INTERRUPT_LENGTH - 1));
        assert!(!ula.interrupt_pending(INTERRUPT_LENGTH));
        assert!(!ula.interrupt_pending(T_STATES_PER_FRAME - 1));

        assert_eq!(ula.next_interrupt(), T_STATES_PER_FRAME);
        ula.end_frame();
        assert!(ula.interrupt_pending(T_STATES_PER_FRAME));
        assert!(!ula.interrupt_pending(T_STATES_PER_FRAME + INTERRUPT_LENGTH));
    }

    #[test]
    fn the_frame_clock_does_not_drift_when_frames_end_late() {
        let mut ula = Ula::new();
        // A slice ends on the last instruction to start before the deadline,
        // so the clock is past the boundary by the time the frame is ended.
        ula.write_port_fe(T_STATES_PER_FRAME + 17, 3);
        ula.end_frame();
        assert_eq!(ula.frame_start(), T_STATES_PER_FRAME);
        assert_eq!(ula.next_interrupt(), 2 * T_STATES_PER_FRAME);
        // The beam was already past the last line of the frame that is being
        // ended, so the write belongs to the frame after it and not to this
        // one, however late this one was closed.
        assert_eq!(ula.border_lines()[LINES_PER_FRAME - 1], 0);
        assert_eq!(frame_of(&mut ula)[0], 3);
    }

    #[test]
    fn flash_inverts_for_sixteen_frames_out_of_thirty_two() {
        let mut ula = Ula::new();
        let mut phases = Vec::new();
        for _ in 0..64 {
            phases.push(ula.flash());
            ula.end_frame();
        }
        for (frame, phase) in phases.iter().enumerate() {
            let expected = if (frame / 16) % 2 == 0 {
                Flash::Normal
            } else {
                Flash::Inverted
            };
            assert_eq!(*phase, expected, "frame {frame}");
        }
        assert_eq!(phases[0], Flash::Normal);
        assert_eq!(phases[15], Flash::Normal);
        assert_eq!(phases[16], Flash::Inverted);
        assert_eq!(phases[31], Flash::Inverted);
        assert_eq!(phases[32], Flash::Normal);
    }
}
