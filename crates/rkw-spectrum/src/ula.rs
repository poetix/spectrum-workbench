//! The ULA: the frame clock, the interrupt, the border and port `0xFE`.
//!
//! The ULA is the whole of the Spectrum that is not the Z80 and the RAM: what
//! time it is within the frame, the 50 Hz interrupt that starts each one, the
//! flash cadence, the border colour, and both halves of port `0xFE` — the byte
//! written, whose low three bits are the border, and the byte read, which is
//! the keyboard and the `EAR` line. Contention is 0020, and reads state this
//! already keeps.
//!
//! # Port `0xFE` is two different registers
//!
//! Writing it sets the border, the speaker and the `MIC` output. Reading it
//! returns five bits of keyboard, chosen by the address ([`crate::keyboard`]),
//! and the `EAR` input on bit 6. The read has nothing to do with the last
//! write: reading back what was written to the border is not possible on this
//! machine, and software that wants to know its own border colour keeps a copy
//! in RAM, which is what the ROM's `BORDCR` is.
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
//! # The speaker is a log for the same reason, and is not double-buffered
//!
//! Bits 4 and 3 of the same write are the speaker and the `MIC` output, and
//! they are recorded the same way and for the same reason: what a program does
//! with the speaker is entirely a matter of *when*, and a level held is silent
//! however high it is held. So a write that moves either bit appends to an
//! [`EdgeLog`] with its T-state, and 20 ms of sound is a few hundred words
//! rather than 69,888 samples of a bit.
//!
//! The border is double-buffered because it is rendered later, on another
//! thread, from a frame that has to have stopped changing. The speaker is not,
//! because it is drained where it is made — inside the same `service_event`
//! that ends the frame, before [`Ula::end_frame`] rolls the log on. There is
//! no reader to race, so there is nothing to present to; a second buffer would
//! be 8 KB more in every checkpoint ticket 0027 takes, and a memcpy a frame,
//! to protect against a consumer that does not exist.
//!
//! Nothing here knows what a sample rate is. The log is the whole of the
//! machine's part in this, and [`rkw_audio`] — which cannot name a `Spectrum`
//! — is the rest (ADR-0021).
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

use rkw_audio::{EdgeLog, Levels};

use crate::frame::{INTERRUPT_LENGTH, LINES_PER_FRAME, T_STATES_PER_FRAME, line_of};
use crate::keyboard::Keyboard;

/// Frames per half of the flash cycle: ink and paper swap for 16 frames, then
/// swap back, so the whole cycle is 32 frames and takes about two thirds of a
/// second.
pub const FLASH_FRAMES: u64 = 16;

use crate::screen::Flash;

/// The bits of a port `0xFE` read that are not the keyboard and not `EAR`.
/// Bits 5 and 7 are not driven on a 48K machine and read as ones.
const UNUSED_BITS: u8 = 0b1010_0000;

/// The `EAR` input, bit 6 of a port `0xFE` read.
const EAR_BIT: u8 = 0b0100_0000;

/// The border and the frame clock.
#[derive(Clone)]
pub struct Ula {
    /// Which keys are down. Written by the frontend (through a command, once
    /// ticket 0026 puts input in the log) and read by the emulated program.
    pub keyboard: Keyboard,
    /// The level on the `EAR` socket, which is what the tape loader listens to
    /// (ticket 0016). High with nothing plugged in.
    ear: bool,
    /// The last byte written to port `0xFE`, whole. Bits 0-2 are the border,
    /// bit 3 the MIC output and bit 4 the speaker.
    port_fe: u8,
    /// Every move of bits 4 and 3 in the frame in progress, stamped with its
    /// T-state. Drained once a frame by whoever is making sound out of it.
    audio: EdgeLog,
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
            keyboard: Keyboard::new(),
            ear: true,
            port_fe: 0,
            audio: EdgeLog::new(),
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

        // Unclamped, unlike the border's line: an offset past the end of the
        // frame is how the log carries an overrun into the next one, and a
        // single step does not service the frame interrupt at all.
        let tick = t.saturating_sub(self.frame_start).min(u32::MAX as u64) as u32;
        self.audio.record(tick, Levels::from_port(value));

        let colour = value & 0x07;
        if colour != self.border {
            self.fill_to(self.line_now(t));
            self.border = colour;
        }
    }

    /// What a read of port `0xFE` returns: the keyboard on bits 0-4, `EAR` on
    /// bit 6, and ones on the two bits nothing drives.
    ///
    /// Which keys the low five bits answer for is a function of the whole
    /// address, not just of the `0xFE` in the low byte — see
    /// [`Keyboard::read`].
    pub fn read_port_fe(&self, port: u16) -> u8 {
        let ear = if self.ear { EAR_BIT } else { 0 };
        self.keyboard.read(port) | ear | UNUSED_BITS
    }

    /// The level on the `EAR` socket. The tape sets it as its edges go past
    /// (ticket 0016); with nothing plugged in it stays where it starts, which
    /// is high.
    ///
    /// A real 48K machine feeds a little of the `MIC` and speaker output back
    /// into this bit, and whether it reads high or low with no tape depends on
    /// which issue of the board it is. Loaders that care about the difference
    /// are rare and none of them care yet, so the line is simply what it was
    /// last set to.
    pub fn set_ear(&mut self, level: bool) {
        self.ear = level;
    }

    /// The level on the `EAR` socket.
    pub fn ear(&self) -> bool {
        self.ear
    }

    /// The last byte written to port `0xFE`.
    pub fn port_fe(&self) -> u8 {
        self.port_fe
    }

    /// The border colour in force now, 0-7.
    pub fn border(&self) -> u8 {
        self.border
    }

    /// The speaker bit, bit 4 of the last port `0xFE` write.
    pub fn speaker(&self) -> bool {
        self.port_fe & 0x10 != 0
    }

    /// The frame in progress' speaker edges: what to make this frame's sound
    /// out of.
    ///
    /// Read it before [`Ula::end_frame`], which rolls the log on. Reading it
    /// after gets the next frame, which at that point is empty.
    pub fn audio(&self) -> &EdgeLog {
        &self.audio
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
        self.audio.roll(T_STATES_PER_FRAME as u32);
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
    fn a_read_of_port_fe_is_the_keyboard_the_ear_line_and_ones() {
        use crate::keyboard::Key;

        let mut ula = Ula::new();
        assert!(ula.ear());
        assert_eq!(ula.read_port_fe(0xFEFE), 0xFF);

        ula.keyboard.press(Key::CapsShift);
        ula.keyboard.press(Key::Num6);
        assert_eq!(ula.read_port_fe(Key::CapsShift.port()), 0xFE);
        assert_eq!(ula.read_port_fe(Key::Num6.port()), 0xEF);
        // The two bits nothing drives stay high whatever is held.
        assert_eq!(ula.read_port_fe(0x00FE) & UNUSED_BITS, UNUSED_BITS);

        ula.set_ear(false);
        assert!(!ula.ear());
        assert_eq!(ula.read_port_fe(0xFFFE), 0xFF & !EAR_BIT);
    }

    #[test]
    fn what_was_written_to_port_fe_is_not_what_is_read_back() {
        // The border is write-only: an OUT of black leaves the read alone.
        let mut ula = Ula::new();
        ula.write_port_fe(0, 0x00);
        assert_eq!(ula.border(), 0);
        assert_eq!(ula.read_port_fe(0xFEFE), 0xFF);
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

    /// The T-state of each edge in the frame in progress.
    fn edge_ticks(ula: &Ula) -> Vec<u32> {
        ula.audio()
            .frame()
            .0
            .iter()
            .copied()
            .map(rkw_audio::tick_of)
            .collect()
    }

    #[test]
    fn a_speaker_write_is_recorded_at_the_t_state_it_was_made_at() {
        let mut ula = Ula::new();
        ula.write_port_fe(1000, 0x10);
        ula.write_port_fe(2750, 0x00);
        ula.write_port_fe(4500, 0x10);

        assert_eq!(edge_ticks(&ula), vec![1000, 2750, 4500]);
        assert_eq!(ula.audio().frame().1, Levels::default());
        assert!(ula.speaker());
    }

    #[test]
    fn a_border_write_that_leaves_the_speaker_alone_records_no_sound() {
        // The striping trick: a colour a scanline, all frame, silently.
        let mut ula = Ula::new();
        for line in 0..LINES_PER_FRAME as u64 {
            ula.write_port_fe(line * T_STATES_PER_LINE, (line % 8) as u8);
        }
        assert_eq!(edge_ticks(&ula), Vec::<u32>::new());

        // And with the speaker held high throughout, still silently.
        ula.write_port_fe(0, 0x10);
        for line in 0..LINES_PER_FRAME as u64 {
            ula.write_port_fe(line * T_STATES_PER_LINE, 0x10 | (line % 8) as u8);
        }
        assert_eq!(edge_ticks(&ula), vec![0]);
    }

    #[test]
    fn the_mic_bit_is_recorded_as_well_as_the_speaker() {
        let mut ula = Ula::new();
        ula.write_port_fe(100, 0x08);
        ula.write_port_fe(200, 0x18);
        ula.write_port_fe(300, 0x10);

        let (edges, _) = ula.audio().frame();
        assert_eq!(
            edges.iter().copied().map(rkw_audio::levels_of).collect::<Vec<_>>(),
            vec![
                Levels { speaker: false, mic: true },
                Levels { speaker: true, mic: true },
                Levels { speaker: true, mic: false },
            ]
        );
    }

    #[test]
    fn ending_the_frame_clears_the_log_and_carries_the_level_over() {
        let mut ula = Ula::new();
        ula.write_port_fe(1000, 0x10);
        assert_eq!(edge_ticks(&ula), vec![1000]);

        ula.end_frame();
        assert_eq!(edge_ticks(&ula), Vec::<u32>::new());
        assert_eq!(
            ula.audio().frame().1,
            Levels { speaker: true, mic: false }
        );
    }

    #[test]
    fn a_speaker_write_past_the_end_of_the_frame_belongs_to_the_next_one() {
        // The mirror of the border's version: a slice ends on the last
        // instruction to start before its deadline, so the clock is past the
        // boundary by the time the frame is ended.
        let mut ula = Ula::new();
        ula.write_port_fe(T_STATES_PER_FRAME + 17, 0x10);
        ula.end_frame();

        assert_eq!(edge_ticks(&ula), vec![17]);
        // The frame that was ended was silent; the edge belongs to the one
        // after it, which therefore opens with the speaker still low.
        assert_eq!(ula.audio().frame().1, Levels::default());
    }

    #[test]
    fn stepping_a_whole_frame_without_servicing_it_does_not_lose_the_edges() {
        // `Command::Step` runs one instruction and never services the frame
        // interrupt, so a user can step the clock past several boundaries
        // before resuming. Each catch-up `end_frame` peels one frame off.
        let mut ula = Ula::new();
        ula.write_port_fe(50, 0x10);
        ula.write_port_fe(T_STATES_PER_FRAME + 50, 0x00);
        ula.write_port_fe(2 * T_STATES_PER_FRAME + 50, 0x10);

        ula.end_frame();
        assert_eq!(
            edge_ticks(&ula),
            vec![50, T_STATES_PER_FRAME as u32 + 50]
        );

        ula.end_frame();
        assert_eq!(edge_ticks(&ula), vec![50]);
        assert_eq!(
            ula.audio().frame().1,
            Levels { speaker: false, mic: false }
        );

        ula.end_frame();
        assert_eq!(edge_ticks(&ula), Vec::<u32>::new());
        assert_eq!(
            ula.audio().frame().1,
            Levels { speaker: true, mic: false }
        );
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
