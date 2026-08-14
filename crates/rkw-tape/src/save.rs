//! The waveform read back: `MIC` edges in, blocks out.
//!
//! Saving is the mirror of [`crate::pulse`] and is a harder problem, because
//! what comes out of a saving program is not a list of pulses but a list of
//! *moments* — the T-states at which bit 3 of port `0xFE` moved — and the
//! block structure has to be recovered from the intervals between them. A
//! [`Recorder`] is the state machine that does it: count a pilot, find the
//! sync pair, then read two pulses a bit until the signal stops.
//!
//! It is deliberately a signal decoder rather than a trap on the ROM's `SAVE`
//! routine. A program that writes its own saver — and the fast loaders all
//! come with one — never calls the ROM, and a trap would produce nothing for
//! it while quietly appearing to work for everything else.
//!
//! # Where it runs, and why it is not in the machine
//!
//! On the emulation thread, once a frame, over the `MIC` edges the ULA has
//! already recorded for the beeper (ADR-0021) — but *not* inside the machine.
//! Loading is machine state because the machine's own timing depends on it and
//! ticket 0027 has to be able to checkpoint a load in progress; saving is an
//! output, and a checkpoint restored in the middle of one loses a partial
//! block in exactly the way it loses the beeper's filters. The failure mode is
//! a file that is short, which is visible, rather than a machine that is
//! subtly different, which is not.
//!
//! # Nothing here grows
//!
//! The buffers are sized at construction and never grown, because this runs on
//! the emulation thread and ADR-0007 says nothing there allocates. A block
//! that overruns them is *discarded* and counted rather than written out
//! short: a truncated block that reaches a TAP file is a corrupt tape that
//! looks fine until it is loaded, which is the one outcome worth going out of
//! the way to avoid.

use crate::pulse::Timing;
use crate::tap::Tap;

/// Pilot pulses the recorder wants to see before it believes there is a tape
/// coming. The ROM writes 3223 in front of a data block and 8063 in front of a
/// header, so this is generous by a factor of twenty-five; what it is really
/// defending against is a beeper tune whose period happens to sit near a pilot
/// pulse.
pub const MIN_PILOT_PULSES: u32 = 128;

/// How far from nominal a pulse may be and still be recognised, in percent.
///
/// Wide, because a real recording off tape is wide, and it costs nothing here:
/// within any one state the candidates are far apart — a zero is 855 and a one
/// is 1710, so even at 30% the two windows do not touch — and the states the
/// pilot, the sync and the data are recognised in are separate.
pub const TOLERANCE: u32 = 30;

/// Bytes a recorder will hold before it starts dropping them. Enough for a
/// screen, a 48K program and its header several times over.
pub const CAPACITY: usize = 128 * 1024;

/// Blocks a recorder will hold.
pub const MAX_BLOCKS: usize = 256;

/// Turns `MIC` edges back into blocks.
#[derive(Debug, Clone)]
pub struct Recorder {
    timing: Timing,
    tolerance: u32,
    min_pilot: u32,
    /// The T-state of the last edge, or `None` when the line has been quiet
    /// long enough that the next edge starts a new measurement rather than
    /// ending an enormous pulse.
    last: Option<u64>,
    state: State,
    pilots: u32,
    /// The byte being assembled, most significant bit first.
    byte: u8,
    bits: u8,
    /// The value of the first half of the bit's cycle, waiting for the second
    /// half to agree with it.
    half: Option<bool>,
    bytes: Vec<u8>,
    blocks: Vec<(usize, usize)>,
    /// Where the block in progress starts in `bytes`.
    start: usize,
    /// The block in progress has lost bytes, so it must not be kept.
    spilled: bool,
    dropped: u64,
    lost: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Nothing is coming off the tape, or a pilot is being counted.
    Idle,
    /// A pilot long enough to be believed, waiting for the sync pair.
    Pilot,
    /// The first sync pulse has arrived.
    Sync,
    /// Reading bits.
    Data,
}

impl Recorder {
    /// A recorder expecting the ROM's timings, with room for [`CAPACITY`]
    /// bytes in [`MAX_BLOCKS`] blocks.
    pub fn new(timing: Timing) -> Recorder {
        Recorder::with_capacity(timing, CAPACITY, MAX_BLOCKS)
    }

    pub fn with_capacity(timing: Timing, bytes: usize, blocks: usize) -> Recorder {
        Recorder {
            timing,
            tolerance: TOLERANCE,
            min_pilot: MIN_PILOT_PULSES,
            last: None,
            state: State::Idle,
            pilots: 0,
            byte: 0,
            bits: 0,
            half: None,
            bytes: Vec::with_capacity(bytes),
            blocks: Vec::with_capacity(blocks),
            start: 0,
            spilled: false,
            dropped: 0,
            lost: 0,
        }
    }

    /// How much pilot to insist on. Lower is for a test with a waveform it
    /// wrote itself; nothing real should need it.
    pub fn with_min_pilot(mut self, pulses: u32) -> Recorder {
        self.min_pilot = pulses;
        self
    }

    /// The `MIC` bit moved at T-state `t`.
    ///
    /// The level it moved *to* is not a parameter because it is not
    /// information: a saving program alternates, and every decision here is
    /// made on the interval between one edge and the next.
    pub fn edge(&mut self, t: u64) {
        if let Some(last) = self.last {
            self.pulse(t.saturating_sub(last));
        }
        self.last = Some(t);
    }

    /// Nothing has happened up to `t`. Called once a frame by whoever is
    /// draining the edges, and it is what ends a block: the end of the last
    /// bit of a block looks exactly like the middle of one until the silence
    /// after it has been waited out.
    pub fn idle(&mut self, t: u64) {
        if let Some(last) = self.last
            && t.saturating_sub(last) > self.gap()
        {
            self.finish();
            self.last = None;
        }
    }

    /// Blocks recorded so far, in order.
    pub fn blocks(&self) -> impl Iterator<Item = &[u8]> {
        self.blocks
            .iter()
            .map(|&(at, len)| &self.bytes[at..at + len])
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// True while a block is being read, which is how a caller knows the tape
    /// is still running and the file is not finished.
    pub fn recording(&self) -> bool {
        self.state != State::Idle
    }

    /// What has been recorded, as a TAP file. Allocates, and is meant to be
    /// called by whoever is writing the file out rather than on the emulation
    /// thread.
    pub fn to_tap(&self) -> Tap {
        self.blocks()
            .fold(Tap::builder(), |builder, body| builder.body(body))
            .build()
    }

    /// Bytes there was no room for. Any block that lost one is discarded, so
    /// this being non-zero means [`Recorder::lost_blocks`] is too.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Blocks that were started and thrown away: an overrun, a partial byte,
    /// or a pulse in the middle of the data that was neither a zero nor a one.
    /// A save that reports any of these produced a file with a hole in it and
    /// has to say so.
    pub fn lost_blocks(&self) -> u64 {
        self.lost
    }

    /// Forget everything, keeping the buffers.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.blocks.clear();
        self.last = None;
        self.state = State::Idle;
        self.pilots = 0;
        self.start = 0;
        self.spilled = false;
        self.half = None;
        self.bits = 0;
        self.byte = 0;
        self.dropped = 0;
        self.lost = 0;
    }

    /// Silence long enough to mean the block has ended. Five times the longest
    /// pulse inside one, which at the ROM's timings is 2.4 ms — far shorter
    /// than the pause between blocks and far longer than anything within one.
    fn gap(&self) -> u64 {
        u64::from(self.timing.one) * 5
    }

    /// One level, held for `ticks`.
    fn pulse(&mut self, ticks: u64) {
        if ticks > self.gap() {
            self.finish();
            self.pilots = 0;
            return;
        }
        match self.state {
            State::Idle => {
                if self.near(ticks, self.timing.pilot) {
                    self.pilots += 1;
                    if self.pilots >= self.min_pilot {
                        self.state = State::Pilot;
                    }
                } else {
                    self.pilots = 0;
                }
            }
            State::Pilot => {
                if self.near(ticks, self.timing.pilot) {
                    // Still in the pilot tone.
                } else if self.near(ticks, self.timing.sync_first) {
                    self.state = State::Sync;
                } else {
                    self.abandon();
                }
            }
            State::Sync => {
                if self.near(ticks, self.timing.sync_second) {
                    self.state = State::Data;
                    self.start = self.bytes.len();
                    self.spilled = false;
                    self.bits = 0;
                    self.byte = 0;
                    self.half = None;
                } else {
                    self.abandon();
                }
            }
            State::Data => self.data(ticks),
        }
    }

    /// A pulse inside the data, which is half of a bit.
    fn data(&mut self, ticks: u64) {
        let bit = if self.near(ticks, self.timing.zero) {
            false
        } else if self.near(ticks, self.timing.one) {
            true
        } else if self.bits == 0 && self.half.is_none() && self.bytes.len() > self.start {
            // Neither a zero nor a one, arriving exactly where a block is
            // entitled to end: this is the tail, and the block before it is
            // whole. Nominally the tail is 945 T-states and reads as half of a
            // zero, but it sits between the two windows as soon as the
            // recording runs slow, and a block thrown away for its own last
            // edge would be a maddening thing to debug.
            self.finish();
            return;
        } else {
            // Neither, in the middle of a byte: the signal is not what it
            // claims to be, and a block read past this point would be wrong
            // rather than short.
            self.abandon();
            return;
        };
        match self.half {
            None => self.half = Some(bit),
            Some(first) => {
                self.half = None;
                if first != bit {
                    // Half a zero and half a one is not a bit.
                    self.abandon();
                    return;
                }
                self.byte = (self.byte << 1) | u8::from(bit);
                self.bits += 1;
                if self.bits == 8 {
                    self.push(self.byte);
                    self.bits = 0;
                    self.byte = 0;
                }
            }
        }
    }

    fn push(&mut self, byte: u8) {
        if self.bytes.len() == self.bytes.capacity() {
            self.dropped += 1;
            self.spilled = true;
        } else {
            self.bytes.push(byte);
        }
    }

    /// The block in progress is not usable. Throw away what was read of it and
    /// go back to looking for a pilot.
    fn abandon(&mut self) {
        if self.state == State::Data && self.bytes.len() > self.start {
            self.bytes.truncate(self.start);
            self.lost += 1;
        }
        self.state = State::Idle;
        self.pilots = 0;
        self.half = None;
        self.bits = 0;
        self.byte = 0;
        self.spilled = false;
    }

    /// The signal has stopped. Keep the block if it is whole.
    fn finish(&mut self) {
        if self.state != State::Data {
            self.state = State::Idle;
            self.pilots = 0;
            return;
        }
        let len = self.bytes.len() - self.start;
        // A dangling half-pulse at a byte boundary is the tail the ROM leaves
        // after the last bit (see `pulse`), so it is not evidence of anything
        // wrong. A dangling *bit* is: the signal stopped in the middle of a
        // byte, and what was read of it cannot be written out as though it
        // were whole.
        let whole = self.bits == 0 && !self.spilled && len > 0;
        if !whole {
            self.bytes.truncate(self.start);
            if len > 0 || self.spilled {
                self.lost += 1;
            }
        } else if self.blocks.len() == self.blocks.capacity() {
            // Room for the bytes but not for another block. Same rule: lose it
            // loudly rather than write a file that is missing a block in the
            // middle without saying so.
            self.bytes.truncate(self.start);
            self.lost += 1;
        } else {
            self.blocks.push((self.start, len));
        }
        self.state = State::Idle;
        self.pilots = 0;
        self.half = None;
        self.bits = 0;
        self.byte = 0;
        self.spilled = false;
    }

    fn near(&self, ticks: u64, target: u32) -> bool {
        let target = u64::from(target);
        ticks.abs_diff(target) * 100 <= target * u64::from(self.tolerance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::Player;
    use crate::tap::{DATA_FLAG, HEADER_FLAG};

    const ROM: Timing = Timing::rom(3_500_000);

    /// Play `tap` into a recorder, edge by edge, and let the tape run out.
    fn record(tap: &Tap, timing: &Timing) -> Recorder {
        let mut recorder = Recorder::new(*timing);
        let mut player = Player::new();
        let mut t = 1_000_000;
        recorder.edge(t);
        while let Some(pulse) = player.next_pulse(tap, timing) {
            t += u64::from(pulse.ticks);
            recorder.edge(t);
        }
        recorder.idle(t + u64::from(timing.one) * 10);
        recorder
    }

    #[test]
    fn what_was_played_is_what_comes_back() {
        let tap = Tap::builder()
            .code("test", 32768, &[0xDE, 0xAD, 0xBE, 0xEF])
            .build();
        let recorder = record(&tap, &ROM);

        assert_eq!(recorder.len(), 2);
        assert_eq!(recorder.to_tap(), tap);
        assert_eq!(recorder.dropped(), 0);
        assert_eq!(recorder.lost_blocks(), 0);
    }

    #[test]
    fn a_block_is_kept_only_once_the_silence_after_it_has_been_waited_out() {
        // Until the gap, the end of the last bit is indistinguishable from the
        // middle of the block, so nothing may be published.
        let tap = Tap::builder().block(DATA_FLAG, &[0x42]).build();
        // No pause, so that the run ends where the block does rather than a
        // second of emulated silence later.
        let timing = ROM.with_pause(0);
        let mut recorder = Recorder::new(timing);
        let mut player = Player::new();
        let mut t = 0;
        while let Some(pulse) = player.next_pulse(&tap, &timing) {
            recorder.edge(t);
            t += u64::from(pulse.ticks);
        }
        assert_eq!(recorder.len(), 0);
        assert!(recorder.recording());

        recorder.idle(t + 100);
        assert_eq!(recorder.len(), 0, "100 T-states is not a gap");

        recorder.idle(t + 10 * u64::from(ROM.one));
        assert_eq!(recorder.len(), 1);
        assert!(!recorder.recording());
    }

    #[test]
    fn a_tape_with_a_bad_checksum_is_recorded_as_it_was_played() {
        // The recorder is a signal decoder and has no opinion about the
        // contents: whether the block is any good is the loader's question.
        let mut body = vec![DATA_FLAG, 0x01];
        body.push(checksum_of(&body) ^ 0xFF);
        let tap = Tap::builder().body(&body).build();

        let recorded = record(&tap, &ROM).to_tap();
        assert_eq!(recorded, tap);
        assert!(!recorded.block(0).expect("one block").checksum_ok());
    }

    fn checksum_of(bytes: &[u8]) -> u8 {
        crate::tap::checksum(bytes)
    }

    #[test]
    fn a_beeper_tune_is_not_a_tape() {
        // A square wave near the pilot frequency, which is what a program
        // playing a note produces, must not be mistaken for a pilot tone that
        // then swallows the first block after it.
        let mut recorder = Recorder::new(ROM);
        let mut t = 0;
        for _ in 0..10_000 {
            t += u64::from(ROM.pilot);
            recorder.edge(t);
        }
        recorder.idle(t + 10 * u64::from(ROM.one));

        assert_eq!(recorder.len(), 0);
        assert_eq!(
            recorder.lost_blocks(),
            0,
            "a pilot with no data is not a loss"
        );
    }

    #[test]
    fn a_partial_byte_is_thrown_away_rather_than_written_out_short() {
        let tap = Tap::builder().block(DATA_FLAG, &[0x01, 0x02, 0x03]).build();
        let mut recorder = Recorder::new(ROM);
        let mut player = Player::new();
        let mut t = 0;
        let mut edges = 0;
        while let Some(pulse) = player.next_pulse(&tap, &ROM) {
            recorder.edge(t);
            t += u64::from(pulse.ticks);
            edges += 1;
            // Stop three bits into the last byte.
            if edges == 3223 + 2 + 4 * 16 + 6 {
                break;
            }
        }
        recorder.idle(t + 10 * u64::from(ROM.one));

        assert_eq!(recorder.len(), 0);
        assert_eq!(recorder.lost_blocks(), 1);
    }

    #[test]
    fn a_pulse_that_is_neither_a_zero_nor_a_one_ends_the_block() {
        let tap = Tap::builder().block(DATA_FLAG, &[0x00; 4]).build();
        let mut recorder = Recorder::new(ROM);
        let mut player = Player::new();
        let mut t = 0;
        let mut edges = 0;
        while let Some(pulse) = player.next_pulse(&tap, &ROM) {
            recorder.edge(t);
            edges += 1;
            // A dropout in the middle of the data: a pulse half again as long
            // as a one, which is not a bit and not long enough to be a gap.
            t += if edges == 3223 + 2 + 20 {
                u64::from(ROM.one) * 3 / 2
            } else {
                u64::from(pulse.ticks)
            };
        }
        recorder.idle(t + 10 * u64::from(ROM.one));

        assert_eq!(recorder.len(), 0);
        assert_eq!(recorder.lost_blocks(), 1);
    }

    #[test]
    fn two_blocks_in_a_row_are_two_blocks() {
        let tap = Tap::builder()
            .block(HEADER_FLAG, &[0x03; 17])
            .block(DATA_FLAG, &[0xFF; 32])
            .build();
        let recorder = record(&tap, &ROM);

        assert_eq!(recorder.len(), 2);
        let blocks: Vec<&[u8]> = recorder.blocks().collect();
        assert_eq!(blocks[0][0], HEADER_FLAG);
        assert_eq!(blocks[1][0], DATA_FLAG);
        assert_eq!(blocks[1].len(), 34);
    }

    #[test]
    fn a_block_too_big_for_the_buffer_is_dropped_and_counted() {
        let tap = Tap::builder().block(DATA_FLAG, &[0x00; 64]).build();
        let mut recorder = Recorder::with_capacity(ROM, 16, 4);
        let mut player = Player::new();
        let mut t = 0;
        while let Some(pulse) = player.next_pulse(&tap, &ROM) {
            recorder.edge(t);
            t += u64::from(pulse.ticks);
        }
        recorder.idle(t + 10 * u64::from(ROM.one));

        assert_eq!(recorder.len(), 0);
        assert_eq!(recorder.dropped(), 66 - 16);
        assert_eq!(recorder.lost_blocks(), 1);
    }

    #[test]
    fn timings_a_third_out_are_still_read() {
        // A real recording is not exact. Every pulse 20% long, which is well
        // outside anything an emulator produces and well inside what a tape
        // recorder does.
        let tap = Tap::builder().block(DATA_FLAG, &[0x5A; 8]).build();
        let mut recorder = Recorder::new(ROM);
        let mut player = Player::new();
        let mut t = 0;
        while let Some(pulse) = player.next_pulse(&tap, &ROM) {
            recorder.edge(t);
            t += u64::from(pulse.ticks) * 6 / 5;
        }
        recorder.idle(t + 20 * u64::from(ROM.one));

        assert_eq!(recorder.to_tap(), tap);
    }
}
