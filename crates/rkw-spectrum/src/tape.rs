//! The tape deck: a mounted image, where the tape has got to, and the `EAR`
//! line it drives.
//!
//! This is the whole of the machine's part in loading, and it is small: an
//! [`Image`] and a [`Player`] from [`rkw_tape`], the T-state the pulse in
//! progress ends at, and the level it put on the line. Everything about what a
//! pulse *is* lives in the tape crate, which has never heard of a Spectrum —
//! including which file format it came out of, since a TAP and a TZX are two
//! things to play and one way of playing them.
//!
//! # Why the tape is machine state when the beeper is not
//!
//! ADR-0021 put the beeper outside the machine because a sample rate is host
//! state. A tape is the opposite: the loading routine's own timing depends on
//! when the edges arrive, so a checkpoint taken mid-load and restored without
//! the tape position would resume into a machine that is measuring a pulse
//! nobody is playing. ADR-0017 says as much in its table — tape edges are
//! *regenerated* by the emulation thread rather than crossing the command
//! ring, and regenerating them means holding the position they are generated
//! from.
//!
//! The image itself is a reference count, so cloning a machine into a
//! checkpoint costs a pointer rather than a copy of the tape. It is immutable
//! and it is named by content ([`Image::hash`]), which is what ADR-0017 asks a
//! recorded session to record about it.
//!
//! # Edges arrive as scheduled events, not as commands
//!
//! [`crate::Spectrum`] reports the earlier of the frame interrupt and the next
//! tape edge from `next_event`, so the slice loop stops there and hands
//! control back. That is the whole mechanism: no callback, no polling, and
//! nothing per instruction.
//!
//! The clock overruns the deadline by up to the length of one instruction —
//! twenty-odd T-states — because a slice ends on the last instruction to
//! *start* before it. So an edge lands up to that late, against pulses of 855
//! T-states and a ROM threshold with hundreds of T-states of margin either
//! side. What it is not is *cumulative*: [`Tape::advance_to`] adds each pulse
//! to the scheduled edge rather than to the clock, so a late edge does not
//! push the one after it.
//!
//! # Trapping the ROM loader
//!
//! [`ld_bytes`] is the accelerated path: given a CPU about to enter the ROM's
//! `LD-BYTES`, it does what that routine would have done and returns. It is
//! not a substitute for the waveform and never will be — a great deal of
//! software replaces the ROM loader with its own, and those loaders can only
//! be run by playing the tape at them — which is why the waveform came first
//! and this is a convenience on top of it.

use rkw_tape::{Image, Player, Timing};
use z80::Cpu;
use z80::flag;

use crate::frame::CLOCK_HZ;
use crate::spectrum::Spectrum;

/// The ROM's `LD-BYTES`, which every ROM-loading program reaches eventually:
/// `LOAD` in BASIC, and `CALL 1366` from machine code.
pub const LD_BYTES: u16 = 0x0556;

/// A tape, and where the head is on it.
#[derive(Clone, Debug)]
pub struct Tape {
    image: Option<Image>,
    timing: Timing,
    player: Player,
    /// The T-state at which the pulse in progress ends and the next one
    /// starts. Meaningless while stopped.
    next_edge: u64,
    playing: bool,
    /// The level on the `EAR` line: what the pulse in progress put there.
    level: bool,
}

impl Default for Tape {
    fn default() -> Tape {
        Tape::new()
    }
}

impl Tape {
    /// An empty deck. The line idles high, which is where a 48K machine with
    /// nothing plugged into it sits.
    pub fn new() -> Tape {
        Tape {
            image: None,
            timing: Timing::rom(CLOCK_HZ),
            player: Player::new(),
            next_edge: 0,
            playing: false,
            level: true,
        }
    }

    /// Put a tape in, rewound and stopped. A TAP or a TZX: what the deck plays
    /// is pulses, and both formats produce them.
    pub fn mount(&mut self, image: impl Into<Image>) {
        self.image = Some(image.into());
        self.player = Player::new();
        self.playing = false;
        self.level = true;
    }

    /// Take it out.
    pub fn eject(&mut self) {
        self.image = None;
        self.playing = false;
        self.level = true;
    }

    /// What is mounted.
    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    /// The pulse lengths this deck plays. The ROM's, unless something has
    /// asked for others — a test that does not want a second of silence
    /// between blocks, or ticket 0017's TZX.
    pub fn timing(&self) -> Timing {
        self.timing
    }

    pub fn set_timing(&mut self, timing: Timing) {
        self.timing = timing;
    }

    /// Start the tape at T-state `t`, which is where the first pulse begins.
    /// A deck with nothing in it stays stopped.
    ///
    /// A tape stopped by a block that asked for it — a TZX telling the person
    /// listening to turn the cassette over — carries on from the block after
    /// it, which is what pressing play on a real one does.
    pub fn play(&mut self, t: u64) {
        self.player.resume();
        if self.image.is_some() && !self.player.finished() {
            self.playing = true;
            self.next_edge = t;
        }
    }

    /// Stop, leaving the line idle. Where the tape is is not disturbed, so
    /// playing again carries on from the same pulse — which is a slight lie
    /// about a physical deck and the useful behaviour for a debugger.
    pub fn stop(&mut self) {
        self.playing = false;
        self.level = true;
    }

    /// Wind back to the start of the tape.
    pub fn rewind(&mut self) {
        self.player.rewind();
    }

    /// Wind to the start of a block.
    pub fn seek(&mut self, block: usize) {
        self.player.seek(block);
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Which block the head is in.
    pub fn block(&self) -> usize {
        self.player.block()
    }

    /// Blocks on the mounted tape.
    pub fn blocks(&self) -> usize {
        self.image.as_ref().map_or(0, |image| image.len())
    }

    /// True once the tape has played out.
    pub fn finished(&self) -> bool {
        self.player.finished()
    }

    /// True when the tape stopped itself rather than running out: a TZX block
    /// asked for it, and playing again picks up at the block after. A front
    /// end wants to tell the two apart, because one of them is a prompt.
    pub fn stopped_by_tape(&self) -> bool {
        self.player.stopped()
    }

    /// The level on `EAR`.
    pub fn level(&self) -> bool {
        self.level
    }

    /// When the level next moves, if the tape is running. This is what the
    /// machine schedules against.
    pub fn next_edge(&self) -> Option<u64> {
        self.playing.then_some(self.next_edge)
    }

    /// Play forward to `t` and return the level the line is left at.
    ///
    /// A loop rather than a step, because the clock can arrive well past the
    /// edge that was scheduled: a single step in the debugger does not stop at
    /// hardware events at all, and one `LDIR` can outlast several pulses of a
    /// fast custom loader.
    pub fn advance_to(&mut self, t: u64) -> bool {
        while self.playing && self.next_edge <= t {
            let Some(image) = self.image.as_ref() else {
                self.stop();
                break;
            };
            match self.player.next_pulse(image, &self.timing) {
                Some(pulse) => {
                    debug_assert!(pulse.ticks > 0, "a pulse of no length is not an edge");
                    self.level = pulse.level;
                    // Added to the edge that was due, not to the clock: the
                    // clock is late by however much the last instruction
                    // overran, and adding to it would make that error
                    // accumulate over the thousands of pulses in a block.
                    self.next_edge += u64::from(pulse.ticks);
                }
                // The end of the tape, which is silence and not a level.
                None => self.stop(),
            }
        }
        self.level
    }
}

/// What a trapped `LD-BYTES` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loaded {
    /// A block was taken off the tape and the routine returned with carry set:
    /// the flag matched, the length matched, and the checksum was right.
    Ok { block: usize, bytes: u16 },
    /// A block was taken off the tape and the routine returned with carry
    /// clear, which is what the ROM does when the block is not the one being
    /// looked for or does not survive its checksum. `LOAD ""` responds by
    /// trying the next one, which is how it skips past a header it does not
    /// want.
    Rejected { block: usize },
    /// Nothing to trap with: no tape, or the tape has run out. The caller must
    /// let the ROM's own routine run, where it will wait for a tape that is
    /// not coming — exactly as a real machine does.
    NoTape,
}

/// Do what the ROM's `LD-BYTES` would have done, and return from it.
///
/// The entry conditions are the ROM's: `IX` is where the bytes go, `DE` is how
/// many are expected, `A` is the flag byte being looked for, and the carry
/// flag says load rather than verify. On the way out `IX` has advanced, `DE`
/// is zero, and carry says whether it worked.
///
/// The caller decides when to use this — it is a front end's fast-load switch,
/// armed at [`LD_BYTES`] — and the machine knows nothing about it. The
/// alternative would have been a trap inside the bus, which would put a
/// host-side convenience into the thing whose bit-for-bit equality ticket 0029
/// exists to check.
///
/// What it does not do is take any time or make any noise. A real load holds
/// the border in stripes for as long as it takes; this returns in the same
/// T-state it was called in, which is the point of it.
pub fn ld_bytes(cpu: &mut Cpu, machine: &mut Spectrum) -> Loaded {
    // The image is taken by handle rather than by reference, and rather than
    // copied: the block has to be readable while the machine is being written
    // to, and a reference count is the cheap way to hold it there.
    let Some(image) = machine.tape.image().cloned() else {
        return Loaded::NoTape;
    };
    // A TZX interleaves its data with text, group markers and control blocks,
    // none of which a loader can see: what a real tape does with them is play
    // them past, so the head moves to the next block there are bytes in.
    let Some((block, body)) = (machine.tape.block()..image.len())
        .find_map(|index| image.data_block(index).map(|body| (index, body)))
    else {
        return Loaded::NoTape;
    };
    // The head moves on whether or not this block was wanted, which is what
    // makes a search for a header terminate.
    machine.tape.seek(block + 1);

    let wanted = cpu.regs.a;
    let load = cpu.regs.flag(flag::C);
    let length = cpu.regs.de();
    let data = if body.len() < 2 {
        &[][..]
    } else {
        &body[1..body.len() - 1]
    };

    // The ROM reads the flag byte first and gives up on it alone; only then
    // does it start putting bytes anywhere.
    let mut ok = body[0] == wanted && data.len() == usize::from(length);
    if ok {
        // Verify reads the tape and compares rather than writing, and the ROM
        // reports a mismatch the same way it reports a bad checksum.
        for (i, &byte) in data.iter().enumerate() {
            let addr = cpu.regs.ix.wrapping_add(i as u16);
            if load {
                machine.memory.write(addr, byte);
            } else if machine.memory.read(addr) != byte {
                ok = false;
                break;
            }
        }
        ok = ok && rkw_tape::checksum(body) == 0;
    }

    if ok {
        cpu.regs.ix = cpu.regs.ix.wrapping_add(length);
        cpu.regs.set_de(0);
        cpu.regs.a = 0;
        cpu.regs.set_flag(flag::Z, true);
    }
    cpu.regs.set_flag(flag::C, ok);
    ret(cpu, machine);

    if ok {
        Loaded::Ok {
            block,
            bytes: length,
        }
    } else {
        Loaded::Rejected { block }
    }
}

/// The `RET` the trapped routine never got to execute.
fn ret(cpu: &mut Cpu, machine: &mut Spectrum) {
    let lo = machine.memory.read(cpu.regs.sp);
    let hi = machine.memory.read(cpu.regs.sp.wrapping_add(1));
    cpu.regs.sp = cpu.regs.sp.wrapping_add(2);
    cpu.regs.pc = u16::from_le_bytes([lo, hi]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkw_tape::tap::DATA_FLAG;
    use rkw_tape::{Tap, Tzx};

    fn tape_of(image: impl Into<Image>) -> Tape {
        let mut tape = Tape::new();
        tape.mount(image);
        tape
    }

    #[test]
    fn a_stopped_tape_schedules_nothing_and_holds_the_line_high() {
        let mut tape = tape_of(Tap::builder().block(DATA_FLAG, &[0x00]).build());
        assert_eq!(tape.next_edge(), None);
        assert!(tape.level());
        assert!(tape.advance_to(1_000_000));
    }

    #[test]
    fn playing_puts_the_first_pulse_where_the_tape_was_started() {
        let mut tape = tape_of(Tap::builder().block(DATA_FLAG, &[0x00]).build());
        tape.play(1000);
        assert_eq!(tape.next_edge(), Some(1000));

        // The first pilot pulse: the line goes low to high at 1000 and stays
        // until 1000 + 2168.
        tape.advance_to(1000);
        assert!(tape.level());
        assert_eq!(tape.next_edge(), Some(1000 + 2168));
    }

    #[test]
    fn a_late_arrival_does_not_push_the_edges_after_it() {
        // The clock overruns a deadline by up to one instruction, and that
        // error must not accumulate: the pulse after a late edge is still due
        // where the waveform says it is.
        let mut tape = tape_of(Tap::builder().block(DATA_FLAG, &[0x00]).build());
        tape.play(0);
        tape.advance_to(20);
        assert_eq!(tape.next_edge(), Some(2168));
        tape.advance_to(2168 + 23);
        assert_eq!(tape.next_edge(), Some(2 * 2168));
    }

    #[test]
    fn catching_up_crosses_as_many_pulses_as_it_has_to() {
        let mut tape = tape_of(Tap::builder().block(DATA_FLAG, &[0x00]).build());
        tape.play(0);
        // Ten pilot pulses in one go, which is what a single step through an
        // instruction that outlasts them looks like.
        tape.advance_to(10 * 2168);
        assert_eq!(tape.next_edge(), Some(11 * 2168));
        assert!(tape.is_playing());
    }

    #[test]
    fn the_end_of_the_tape_stops_the_deck_and_leaves_the_line_idle() {
        let tap = Tap::builder().block(DATA_FLAG, &[0x00]).build();
        let timing = Timing::rom(CLOCK_HZ).with_pause(0);
        let mut tape = tape_of(tap.clone());
        tape.set_timing(timing);
        tape.play(0);

        let level = tape.advance_to(Player::duration(&tap, &timing) + 1);
        assert!(!tape.is_playing());
        assert!(tape.finished());
        assert!(level, "the line goes back to idle when the tape runs out");
        assert_eq!(tape.next_edge(), None);
    }

    #[test]
    fn a_tzx_that_stops_the_tape_stops_the_deck_and_starting_it_again_carries_on() {
        // The block a game puts between its two levels, or a tape between its
        // two sides: it is not the end of the tape, and the deck has to be
        // able to tell the difference or the second half never plays.
        let tzx = Tzx::builder().tone(1000, 1).pause(0).tone(2000, 1).build();
        let mut tape = tape_of(tzx);
        tape.play(0);

        tape.advance_to(1000);
        assert!(!tape.is_playing());
        assert!(tape.stopped_by_tape());
        assert!(!tape.finished());
        assert_eq!(tape.block(), 2);

        tape.play(5000);
        assert!(tape.is_playing());
        assert!(!tape.stopped_by_tape());
        assert_eq!(tape.next_edge(), Some(5000));
        tape.advance_to(5000);
        assert_eq!(tape.next_edge(), Some(7000), "the second tone, 2000 long");
        tape.advance_to(7000);
        assert!(tape.finished(), "and then the tape has run out");
    }

    #[test]
    fn playing_a_deck_with_no_tape_in_it_does_nothing() {
        let mut tape = Tape::new();
        tape.play(0);
        assert!(!tape.is_playing());
        assert_eq!(tape.next_edge(), None);
    }
}
