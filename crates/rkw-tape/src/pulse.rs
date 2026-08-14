//! The waveform: what a TAP file sounds like, in T-states.
//!
//! The ROM's loader knows nothing about blocks or bytes. It sits in a loop
//! reading `EAR`, and every time the bit changes it works out how long the
//! last level lasted; that number is the whole of the information coming off
//! the tape. So a block on the tape is:
//!
//! | part | pulse | how many |
//! | --- | --- | --- |
//! | pilot | [`Timing::pilot`] | 8063 before a header, 3223 before data |
//! | sync | [`sync_first`], [`sync_second`] | one each, and both are short |
//! | data | [`zero`] or [`one`] | two a bit, most significant bit first |
//! | tail | [`Timing::tail`] | one, so the last bit has an end |
//! | pause | [`Timing::pause`] | one, and then the next block |
//!
//! [`sync_first`]: Timing::sync_first
//! [`sync_second`]: Timing::sync_second
//! [`zero`]: Timing::zero
//! [`one`]: Timing::one
//!
//! A *pulse* is one level, held for its length: two of them make a cycle, and
//! a bit is one cycle whose half-period says which bit it is. The pilot exists
//! so that the loader can find the tape's timing before it has to decide
//! anything, and the sync pair — both shorter than a pilot pulse — is what
//! tells it the pilot has ended.
//!
//! # Why the player is an index rather than an iterator
//!
//! [`Player`] holds a block number and a phase, and the tape is passed in to
//! every call. The obvious design — an iterator borrowing the `Tap` — cannot
//! be held by the machine that plays it, because that machine is cloned into a
//! checkpoint ring (ADR-0017) and a borrow has nowhere to live across a clone.
//! Plain `Copy` state with the image as an argument is checkpointable, and
//! `Player` is sixteen bytes.
//!
//! # The tail is not decoration
//!
//! A loader reads a bit by timing the interval between two edges, so the last
//! bit of a block is not readable until an edge arrives *after* it. The ROM's
//! `SA-BYTES` duly leaves one — about 945 T-states past the last bit — and a
//! waveform generated without it hands the loader half a bit and a silence,
//! which it waits out and reports as a loading error. It cannot be left to the
//! pause either: the pause is a level, and if the last data pulse was already
//! at that level there is no edge there at all.
//!
//! # Polarity is not observable
//!
//! Every pulse carries the level it puts on the line, and which way round that
//! is has no effect on any loader: they all measure the time between edges.
//! What matters is that there *is* an edge where the waveform says there is,
//! which is why a pause is emitted as a low level and the pilot after it
//! starts high — so the tape restarting is an edge, not a level that quietly
//! stays where it was.

use crate::tap::Tap;

/// One level on the `EAR` line, and how long it lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulse {
    /// T-states this level is held for.
    pub ticks: u32,
    /// The level itself. The tape's absolute polarity means nothing; the
    /// transitions between successive pulses are the signal.
    pub level: bool,
}

/// Pulse lengths in T-states.
///
/// A struct rather than constants because ticket 0017's TZX carries its own
/// timings per block, and because a test that can choose them can work the
/// arithmetic out on paper. [`Timing::rom`] is what the 48K ROM writes and
/// expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub pilot: u32,
    /// Pilot pulses in front of a header block.
    pub header_pilot_pulses: u32,
    /// Pilot pulses in front of a data block. Shorter because by then the
    /// loader has already been running.
    pub data_pilot_pulses: u32,
    pub sync_first: u32,
    pub sync_second: u32,
    /// Half a cycle of a zero bit.
    pub zero: u32,
    /// Half a cycle of a one bit, which is twice a zero: the ROM decides by
    /// comparing against a threshold between them.
    pub one: u32,
    /// The edge after the last bit, without which that bit has no end for a
    /// loader to time against.
    pub tail: u32,
    /// Silence after each block.
    pub pause: u32,
}

impl Timing {
    /// What the 48K ROM's `SAVE` writes and its `LOAD` expects, at a clock of
    /// `clock_hz`.
    ///
    /// The pulse lengths are counts of Z80 cycles in the ROM's own loops and
    /// do not scale with the clock — they are what they are because of the
    /// instructions that produce them. Only the pause is a duration, and it is
    /// one second.
    pub const fn rom(clock_hz: u64) -> Timing {
        Timing {
            pilot: 2168,
            header_pilot_pulses: 8063,
            data_pilot_pulses: 3223,
            sync_first: 667,
            sync_second: 735,
            zero: 855,
            one: 1710,
            tail: 945,
            pause: if clock_hz > u32::MAX as u64 {
                u32::MAX
            } else {
                clock_hz as u32
            },
        }
    }

    /// The same timings with a different gap between blocks. A shorter pause
    /// is what a test uses to stop a two-block tape taking two seconds of
    /// emulated time it has no use for.
    pub const fn with_pause(self, ticks: u32) -> Timing {
        Timing {
            pause: ticks,
            ..self
        }
    }

    /// Pilot pulses in front of a block with this flag byte.
    pub const fn pilot_pulses(&self, header: bool) -> u32 {
        if header {
            self.header_pilot_pulses
        } else {
            self.data_pilot_pulses
        }
    }
}

/// Where the tape has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    block: usize,
    phase: Phase,
    level: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// About to start a block: the next call works out how long its pilot is.
    Start,
    Pilot(u32),
    SyncFirst,
    SyncSecond,
    Bit {
        byte: usize,
        /// 0 is the most significant bit, which is the one that goes first.
        bit: u8,
        /// The second half of the bit's cycle, which is the same length as the
        /// first.
        second: bool,
    },
    /// The edge that ends the last bit.
    Tail,
    Pause,
    /// Past the last block.
    Done,
}

impl Default for Player {
    fn default() -> Player {
        Player::new()
    }
}

impl Player {
    /// At the start of the first block, with the line low, so that the first
    /// pilot pulse is an edge upwards.
    pub const fn new() -> Player {
        Player {
            block: 0,
            phase: Phase::Start,
            level: false,
        }
    }

    /// The next pulse, or `None` at the end of the tape.
    ///
    /// The tape is an argument rather than a field, so nothing stops a caller
    /// passing a different one; what it gets if it does is the position it had
    /// applied to the new tape, which is what a physical machine would also do
    /// and is why [`Player::seek`] exists.
    pub fn next_pulse(&mut self, tap: &Tap, timing: &Timing) -> Option<Pulse> {
        loop {
            match self.phase {
                Phase::Done => return None,
                Phase::Start => match tap.block(self.block) {
                    None => self.phase = Phase::Done,
                    Some(block) => {
                        let pulses = timing.pilot_pulses(block.is_header());
                        self.phase = match pulses {
                            0 => Phase::SyncFirst,
                            n => Phase::Pilot(n),
                        };
                    }
                },
                Phase::Pilot(left) => {
                    self.phase = match left {
                        0 | 1 => Phase::SyncFirst,
                        n => Phase::Pilot(n - 1),
                    };
                    return Some(self.flip(timing.pilot));
                }
                Phase::SyncFirst => {
                    self.phase = Phase::SyncSecond;
                    return Some(self.flip(timing.sync_first));
                }
                Phase::SyncSecond => {
                    self.phase = Phase::Bit {
                        byte: 0,
                        bit: 0,
                        second: false,
                    };
                    return Some(self.flip(timing.sync_second));
                }
                Phase::Bit { byte, bit, second } => {
                    let body = tap.block(self.block).map(|b| b.body()).unwrap_or(&[]);
                    let Some(&value) = body.get(byte) else {
                        self.phase = Phase::Tail;
                        continue;
                    };
                    let set = value & (0x80 >> bit) != 0;
                    self.phase = match (second, bit) {
                        (false, _) => Phase::Bit {
                            byte,
                            bit,
                            second: true,
                        },
                        (true, 7) => Phase::Bit {
                            byte: byte + 1,
                            bit: 0,
                            second: false,
                        },
                        (true, _) => Phase::Bit {
                            byte,
                            bit: bit + 1,
                            second: false,
                        },
                    };
                    return Some(self.flip(if set { timing.one } else { timing.zero }));
                }
                Phase::Tail => {
                    self.phase = Phase::Pause;
                    if timing.tail > 0 {
                        return Some(self.flip(timing.tail));
                    }
                }
                Phase::Pause => {
                    self.block += 1;
                    self.phase = Phase::Start;
                    // A pause of nothing is not a pulse. Emitting one would be
                    // a zero-length level, which every consumer of this would
                    // then have to defend itself against.
                    if timing.pause > 0 {
                        self.level = false;
                        return Some(Pulse {
                            ticks: timing.pause,
                            level: false,
                        });
                    }
                }
            }
        }
    }

    /// Which block the tape is in the middle of.
    pub const fn block(&self) -> usize {
        self.block
    }

    /// The level the last pulse put on the line.
    pub const fn level(&self) -> bool {
        self.level
    }

    /// True once the tape has run out.
    pub const fn finished(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// Wind to the start of a block, which is where a tape counter would put
    /// it. Past the end is not an error: the tape has simply run out, and
    /// [`Player::next_pulse`] says so.
    pub fn seek(&mut self, block: usize) {
        self.block = block;
        self.phase = Phase::Start;
        self.level = false;
    }

    /// Back to the beginning.
    pub fn rewind(&mut self) {
        self.seek(0);
    }

    /// How long the whole tape takes to play, in T-states.
    ///
    /// Walks it, because that is the only way to know: the answer depends on
    /// how many bits are set in every byte of it.
    pub fn duration(tap: &Tap, timing: &Timing) -> u64 {
        let mut player = Player::new();
        let mut total = 0;
        while let Some(pulse) = player.next_pulse(tap, timing) {
            total += u64::from(pulse.ticks);
        }
        total
    }

    fn flip(&mut self, ticks: u32) -> Pulse {
        self.level = !self.level;
        Pulse {
            ticks,
            level: self.level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tap::{DATA_FLAG, HEADER_FLAG};

    /// Small round numbers, so that every assertion below can be worked out on
    /// paper: two pilot pulses, and a bit that is one tick or two.
    const TINY: Timing = Timing {
        pilot: 100,
        header_pilot_pulses: 3,
        data_pilot_pulses: 2,
        sync_first: 10,
        sync_second: 20,
        zero: 1,
        one: 2,
        tail: 5,
        pause: 1000,
    };

    /// Pulses in one block of `bytes` bytes at [`TINY`], whichever way its
    /// bits fall: the pilot, the sync pair, two a bit, the tail and the pause.
    fn tiny_pulses(header: bool, bytes: usize) -> usize {
        TINY.pilot_pulses(header) as usize + 2 + 16 * bytes + 2
    }

    fn pulses(tap: &Tap, timing: &Timing) -> Vec<Pulse> {
        let mut player = Player::new();
        let mut out = Vec::new();
        while let Some(pulse) = player.next_pulse(tap, timing) {
            out.push(pulse);
        }
        out
    }

    fn ticks(tap: &Tap, timing: &Timing) -> Vec<u32> {
        pulses(tap, timing).iter().map(|p| p.ticks).collect()
    }

    #[test]
    fn a_block_is_pilot_sync_two_pulses_a_bit_a_tail_and_a_pause() {
        // One block whose body is a single byte, 0b1000_0001. The flag byte is
        // what decides the pilot length and here it is the data as well, and
        // 0x81 is over 0x80, so this is a data block: two pilot pulses.
        let tap = Tap::builder().body(&[0x81]).build();
        assert_eq!(
            ticks(&tap, &TINY),
            vec![
                100, 100, // the data pilot
                10, 20, // sync
                2, 2, // the top bit, set
                1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // six zeroes
                2, 2,    // the bottom bit, set
                5,    // the tail, which is what ends that last bit
                1000, // the pause
            ]
        );
    }

    #[test]
    fn a_header_block_gets_the_longer_pilot() {
        let header = Tap::builder().body(&[HEADER_FLAG]).build();
        let data = Tap::builder().body(&[DATA_FLAG]).build();

        assert_eq!(ticks(&header, &TINY)[..3], [100, 100, 100]);
        assert_eq!(ticks(&data, &TINY)[..3], [100, 100, 10]);
    }

    #[test]
    fn every_pulse_is_an_edge_and_the_tape_starts_by_going_high() {
        let tap = Tap::builder().body(&[0x55]).build();
        let pulses = pulses(&tap, &TINY);

        assert!(pulses[0].level, "the first pulse goes high");
        // Consecutive pulses alternate, up to the pause, which is low by
        // construction so that the block after it starts with an edge.
        let pause = pulses.len() - 1;
        for (i, pair) in pulses[..=pause].windows(2).enumerate() {
            if i + 1 == pause {
                assert!(!pair[1].level, "the pause is low");
            } else {
                assert_ne!(pair[0].level, pair[1].level, "pulse {i} is not an edge");
            }
        }
    }

    #[test]
    fn a_pause_of_nothing_is_not_a_pulse() {
        // A zero-length level would be an edge that arrives at the same
        // T-state as the one before it, which every consumer downstream would
        // then have to defend against.
        let tap = Tap::builder().body(&[0x00]).body(&[0x00]).build();
        let timing = TINY.with_pause(0);
        assert!(ticks(&tap, &timing).iter().all(|&t| t > 0));
        // Two header blocks, each less its pause.
        assert_eq!(ticks(&tap, &timing).len(), 2 * (tiny_pulses(true, 1) - 1));
    }

    #[test]
    fn the_rom_timings_are_the_ones_the_rom_uses() {
        let timing = Timing::rom(3_500_000);
        let tap = Tap::builder().block(DATA_FLAG, &[0x00]).build();
        let ticks = ticks(&tap, &timing);

        assert_eq!(ticks.len(), 3223 + 2 + 3 * 16 + 2);
        assert!(ticks[..3223].iter().all(|&t| t == 2168));
        assert_eq!(ticks[3223..3225], [667, 735]);
        // Flag 0xFF is eight one bits; the byte after it is eight zeroes.
        assert!(ticks[3225..3241].iter().all(|&t| t == 1710));
        assert!(ticks[3241..3257].iter().all(|&t| t == 855));
        assert_eq!(ticks[ticks.len() - 2], 945);
        assert_eq!(ticks[ticks.len() - 1], 3_500_000);
    }

    #[test]
    fn the_duration_of_a_tape_is_the_sum_of_its_pulses() {
        let tap = Tap::builder().block(DATA_FLAG, &[0xFF; 10]).build();
        let timing = Timing::rom(3_500_000);
        let total: u64 = ticks(&tap, &timing).iter().map(|&t| u64::from(t)).sum();

        assert_eq!(Player::duration(&tap, &timing), total);
        // Every byte is 0xFF, so every bit is a one: pilot, sync, 12 bytes of
        // one bits, and the pause.
        assert_eq!(
            total,
            3223 * 2168 + 667 + 735 + 12 * 16 * 1710 + 945 + 3_500_000
        );
    }

    #[test]
    fn seeking_starts_the_block_again_from_its_pilot() {
        let tap = Tap::builder().body(&[0x00]).body(&[0xFF]).build();
        let mut player = Player::new();
        player.seek(1);

        assert_eq!(player.block(), 1);
        let mut out = Vec::new();
        while let Some(pulse) = player.next_pulse(&tap, &TINY) {
            out.push(pulse.ticks);
        }
        // The second block only, whose flag of 0xFF makes it a data block:
        // two pilot pulses, sync, eight one bits, tail, pause.
        assert_eq!(out[..4], [100, 100, 10, 20]);
        assert_eq!(out.len(), tiny_pulses(false, 1));
        assert!(player.finished());
    }

    #[test]
    fn an_empty_tape_plays_nothing() {
        let mut player = Player::new();
        assert_eq!(player.next_pulse(&Tap::empty(), &TINY), None);
        assert!(player.finished());
    }
}
