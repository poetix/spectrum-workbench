//! The waveform: what a tape image sounds like, in T-states.
//!
//! The ROM's loader knows nothing about blocks or bytes. It sits in a loop
//! reading `EAR`, and every time the bit changes it works out how long the
//! last level lasted; that number is the whole of the information coming off
//! the tape. So a block on the tape is:
//!
//! | part | pulse | how many |
//! | --- | --- | --- |
//! | pilot | [`Data::pilot`] | 8063 before a header, 3223 before data |
//! | sync | [`Data::sync_first`], [`Data::sync_second`] | one each, and both are short |
//! | data | [`Data::zero`] or [`Data::one`] | two a bit, most significant bit first |
//! | tail | [`Data::tail`] | one, so the last bit has an end |
//! | pause | [`Data::pause`] | one, and then the next block |
//!
//! A *pulse* is one level, held for its length: two of them make a cycle, and
//! a bit is one cycle whose half-period says which bit it is. The pilot exists
//! so that the loader can find the tape's timing before it has to decide
//! anything, and the sync pair — both shorter than a pilot pulse — is what
//! tells it the pilot has ended.
//!
//! # Where the numbers come from
//!
//! [`Player`] does not know what a TAP or a TZX file is. It asks the image for
//! a [`Plan`] for the block it is on — a data block and its lengths, a run of
//! identical pulses, a list of pulse lengths, a recording of the line, or an
//! instruction to jump, loop or stop — and plays it. That is the whole of the
//! difference between the two formats: TAP has one plan, `Plan::Data` with the
//! ROM's timings, and TZX has all of them.
//!
//! A plan is fetched per pulse rather than held, so [`Player`] stays small
//! enough to sit in a machine that gets cloned into a checkpoint ring
//! (ADR-0017) and cannot go stale against an image swapped under it.
//!
//! # Why the player is an index rather than an iterator
//!
//! [`Player`] holds a block number and a phase, and the tape is passed in to
//! every call. The obvious design — an iterator borrowing the image — cannot
//! be held by the machine that plays it, because that machine is cloned into a
//! checkpoint (ADR-0017) and a borrow has nowhere to live across a clone.
//! Plain `Copy` state with the image as an argument is checkpointable, and
//! `Player` is forty bytes.
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
//! TZX has the same problem and solves it the same way: the first millisecond
//! of a block's pause is an edge and the rest is silence, which is why
//! [`Timing::tail_for`] and [`Timing::pause_for`] split a pause in
//! milliseconds into the two of them.
//!
//! # Polarity is not observable
//!
//! Every pulse carries the level it puts on the line, and which way round that
//! is has no effect on any loader: they all measure the time between edges.
//! What matters is that there *is* an edge where the waveform says there is,
//! which is why a pause is emitted as a low level and the pilot after it
//! starts high — so the tape restarting is an edge, not a level that quietly
//! stays where it was.
//!
//! The exception is a direct recording, which is a sampled line rather than an
//! encoding: its levels are what was on the tape, and it says so absolutely
//! rather than by flipping.

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
/// These are the ROM's, and they are what a TAP file's blocks are played at —
/// a TAP file records no timings, so there is nothing else it could mean. A
/// TZX file carries its own per block and uses this only for what it defers to
/// the ROM: the standard speed block, and [`Timing::ms`], which is the one
/// thing a tape image cannot know because it depends on the clock.
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
    /// T-states in a millisecond, which is the unit TZX keeps its pauses in.
    pub ms: u32,
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
        let clock = if clock_hz > u32::MAX as u64 {
            u32::MAX
        } else {
            clock_hz as u32
        };
        Timing {
            pilot: 2168,
            header_pilot_pulses: 8063,
            data_pilot_pulses: 3223,
            sync_first: 667,
            sync_second: 735,
            zero: 855,
            one: 1710,
            tail: 945,
            pause: clock,
            ms: clock / 1000,
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

    /// The edge that ends a block whose TZX pause is `pause_ms`: one
    /// millisecond of it, or nothing at all if the block asked for no pause.
    ///
    /// A block with no pause runs straight into the next one, which is how a
    /// loader that splits its data across several blocks keeps them a single
    /// waveform. The block after it supplies the edge.
    pub const fn tail_for(&self, pause_ms: u16) -> u32 {
        if pause_ms == 0 { 0 } else { self.ms }
    }

    /// The silence after that edge: the rest of the pause.
    pub const fn pause_for(&self, pause_ms: u16) -> u32 {
        (pause_ms as u32).saturating_sub(1) * self.ms
    }
}

/// A data block and every length needed to play it.
///
/// One struct for what TAP and all three of TZX's data blocks have in common,
/// which is everything: a pure data block is one with no pilot and no sync,
/// and a pause block is one with no data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Data<'a> {
    pub pilot: u32,
    pub pilot_pulses: u32,
    /// Zero for a block with no sync pulse, which is what a pure data block is.
    pub sync_first: u32,
    pub sync_second: u32,
    pub zero: u32,
    pub one: u32,
    /// Bits of the last byte that are data, 1 to 8. Always 8 for TAP, which
    /// cannot say otherwise.
    pub last_bits: u8,
    /// The edge that ends the last bit. See the module docs.
    pub tail: u32,
    /// Silence after that edge.
    pub pause: u32,
    /// Flag, data and checksum — everything the tape carried, played as bits.
    pub bytes: &'a [u8],
}

/// A recording of the line itself, one bit a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Direct<'a> {
    /// T-states each sample lasts.
    pub each: u32,
    /// Bits of the last byte that are samples, 1 to 8.
    pub last_bits: u8,
    /// Silence after the recording.
    pub pause: u32,
    /// The samples, most significant bit first, each bit a level.
    pub samples: &'a [u8],
}

/// What one block of a tape does.
///
/// The player's whole vocabulary, and the only thing a tape image has to be
/// able to say about itself. TAP speaks one word of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan<'a> {
    /// Pilot, sync, bits, an edge and a pause.
    Data(Data<'a>),
    /// `pulses` identical pulses of `length`, which is a pilot tone for a
    /// loader assembling its own block header.
    Tone {
        length: u32,
        pulses: u32,
    },
    /// A handful of pulses of stated lengths, as little-endian words.
    Pulses {
        lengths: &'a [u8],
    },
    /// The line as it was sampled.
    Direct(Direct<'a>),
    /// Play block `to` next. Out of range is the end of the tape, and a jump
    /// to this same block is the format's way of saying "loop forever".
    Jump {
        to: isize,
    },
    /// Play from the next block to the matching [`Plan::LoopEnd`] this many
    /// times in total.
    Loop {
        count: u16,
    },
    LoopEnd,
    /// Put the line here before the next pulse, without an edge of its own.
    Level {
        level: bool,
    },
    /// Stop the tape and wait to be started again. What a real tape does when
    /// the loader wants a second side, or a person to press a key.
    Stop,
    /// Nothing to play: text, a group marker, a block this player has no
    /// waveform for. The tape moves on.
    Nothing,
}

/// A tape image the player can play: something that can say what each of its
/// blocks does.
pub trait Playable {
    /// What block `index` does, or `None` past the end of the tape.
    fn plan(&self, index: usize, timing: &Timing) -> Option<Plan<'_>>;
}

/// Control blocks crossed in one call before the tape is declared over.
///
/// A jump to itself is the format's "loop forever", and a loop whose body is
/// empty is the same thing written differently: both would spin here without
/// ever producing a pulse. A tape that has made no sound in this many blocks
/// is not going to.
const CONTROL_LIMIT: usize = 1 << 16;

/// Pulses [`Player::duration`] will walk before giving up. A tape that loops
/// forever has no duration, and a caller sizing a buffer wants an answer
/// rather than a hang. Sixteen million is fifty times a full-length tape.
const DURATION_LIMIT: usize = 1 << 24;

/// Where the tape has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    block: usize,
    phase: Phase,
    level: bool,
    /// The block a [`Plan::LoopEnd`] goes back to, and how many times round
    /// there is left to go. TZX's loops do not nest — the format says so —
    /// which is why this is a pair of fields and not a stack.
    loop_start: usize,
    loop_left: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// About to start a block: the next call asks what it is.
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
    /// Pulses left in a pure tone.
    Tone(u32),
    /// The next pulse of a pulse sequence, counted in pulses.
    Sequence(usize),
    /// The next sample of a direct recording.
    Sample {
        byte: usize,
        bit: u8,
    },
    /// Stopped by the tape rather than by the person listening to it: the
    /// block after the one that asked. Playing again carries on from there.
    Stopped,
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
            loop_start: 0,
            loop_left: 0,
        }
    }

    /// The next pulse, or `None` at the end of the tape and at a block that
    /// asked for it to stop.
    ///
    /// The tape is an argument rather than a field, so nothing stops a caller
    /// passing a different one; what it gets if it does is the position it had
    /// applied to the new tape — and, if that position is in the middle of a
    /// block of a different shape, the start of that block again.
    pub fn next_pulse<I: Playable + ?Sized>(
        &mut self,
        image: &I,
        timing: &Timing,
    ) -> Option<Pulse> {
        let mut steps = 0;
        loop {
            if matches!(self.phase, Phase::Done | Phase::Stopped) {
                return None;
            }
            let Some(plan) = image.plan(self.block, timing) else {
                self.phase = Phase::Done;
                return None;
            };
            steps += 1;
            if steps > CONTROL_LIMIT {
                self.phase = Phase::Done;
                return None;
            }
            match (self.phase, plan) {
                (Phase::Start, plan) => self.begin(plan),

                (Phase::Pilot(left), Plan::Data(data)) => {
                    self.phase = match left {
                        0 | 1 => after_pilot(&data),
                        n => Phase::Pilot(n - 1),
                    };
                    return Some(self.flip(data.pilot));
                }
                (Phase::SyncFirst, Plan::Data(data)) => {
                    self.phase = after_sync_first(&data);
                    return Some(self.flip(data.sync_first));
                }
                (Phase::SyncSecond, Plan::Data(data)) => {
                    self.phase = first_bit(&data);
                    return Some(self.flip(data.sync_second));
                }
                (Phase::Bit { byte, bit, second }, Plan::Data(data)) => {
                    let Some(&value) = data.bytes.get(byte) else {
                        self.phase = Phase::Tail;
                        continue;
                    };
                    let set = value & (0x80 >> bit) != 0;
                    let last = bit + 1 >= bits_in(&data, byte);
                    self.phase = match (second, last) {
                        (false, _) => Phase::Bit {
                            byte,
                            bit,
                            second: true,
                        },
                        (true, false) => Phase::Bit {
                            byte,
                            bit: bit + 1,
                            second: false,
                        },
                        (true, true) => Phase::Bit {
                            byte: byte + 1,
                            bit: 0,
                            second: false,
                        },
                    };
                    return Some(self.flip(if set { data.one } else { data.zero }));
                }
                (Phase::Tail, Plan::Data(data)) => {
                    self.phase = Phase::Pause;
                    if data.tail > 0 {
                        return Some(self.flip(data.tail));
                    }
                }
                (Phase::Pause, plan) => {
                    let pause = match plan {
                        Plan::Data(data) => data.pause,
                        Plan::Direct(direct) => direct.pause,
                        _ => 0,
                    };
                    self.next_block();
                    // A pause of nothing is not a pulse. Emitting one would be
                    // a zero-length level, which every consumer of this would
                    // then have to defend itself against.
                    if pause > 0 {
                        self.level = false;
                        return Some(Pulse {
                            ticks: pause,
                            level: false,
                        });
                    }
                }

                (Phase::Tone(left), Plan::Tone { length, .. }) => {
                    match left {
                        0 | 1 => self.next_block(),
                        n => self.phase = Phase::Tone(n - 1),
                    }
                    return Some(self.flip(length));
                }
                (Phase::Sequence(index), Plan::Pulses { lengths }) => {
                    let at = 2 * index;
                    let Some(pair) = lengths.get(at..at + 2) else {
                        self.next_block();
                        continue;
                    };
                    self.phase = Phase::Sequence(index + 1);
                    let ticks = u32::from(u16::from_le_bytes([pair[0], pair[1]]));
                    if ticks > 0 {
                        return Some(self.flip(ticks));
                    }
                }
                (Phase::Sample { byte, bit }, Plan::Direct(direct)) => {
                    match run_of_samples(&direct, byte, bit) {
                        None => self.phase = Phase::Pause,
                        Some(run) => {
                            self.phase = Phase::Sample {
                                byte: run.byte,
                                bit: run.bit,
                            };
                            // A recording says what the level was rather than
                            // that it changed, so this does not flip.
                            self.level = run.level;
                            return Some(Pulse {
                                ticks: run.ticks,
                                level: run.level,
                            });
                        }
                    }
                }

                // The phase does not fit the block, which means the image
                // under the player changed. Start the block it is on.
                _ => self.phase = Phase::Start,
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

    /// True when the tape stopped itself: a TZX block asked for it, and the
    /// next block is where playing again picks up. Distinct from
    /// [`Player::finished`], which is the end of the tape and not a pause in
    /// the middle of one.
    pub const fn stopped(&self) -> bool {
        matches!(self.phase, Phase::Stopped)
    }

    /// Carry on from a stop, at the block after the one that asked for it.
    /// Does nothing to a tape that is merely between pulses.
    pub fn resume(&mut self) {
        if self.stopped() {
            self.phase = Phase::Start;
        }
    }

    /// Wind to the start of a block, which is where a tape counter would put
    /// it. Past the end is not an error: the tape has simply run out, and
    /// [`Player::next_pulse`] says so.
    pub fn seek(&mut self, block: usize) {
        self.block = block;
        self.phase = Phase::Start;
        self.level = false;
        self.loop_left = 0;
        self.loop_start = 0;
    }

    /// Back to the beginning.
    pub fn rewind(&mut self) {
        self.seek(0);
    }

    /// How long the whole tape takes to play, in T-states.
    ///
    /// Walks it, because that is the only way to know: the answer depends on
    /// how many bits are set in every byte of it. A tape that loops forever
    /// has no answer, and this gives up after sixteen million pulses — fifty
    /// times a full-length tape — rather than hanging.
    pub fn duration<I: Playable + ?Sized>(image: &I, timing: &Timing) -> u64 {
        let mut player = Player::new();
        let mut total = 0;
        for _ in 0..DURATION_LIMIT {
            let Some(pulse) = player.next_pulse(image, timing) else {
                break;
            };
            total += u64::from(pulse.ticks);
        }
        total
    }

    /// Take up a block: work out where its first pulse is, or — for the blocks
    /// that are not waveform at all — do what they say and move on.
    fn begin(&mut self, plan: Plan<'_>) {
        match plan {
            Plan::Data(data) => self.phase = first_pilot(&data),
            Plan::Tone { length: 0, .. } | Plan::Tone { pulses: 0, .. } => self.next_block(),
            Plan::Tone { pulses, .. } => self.phase = Phase::Tone(pulses),
            Plan::Pulses { lengths } if lengths.len() < 2 => self.next_block(),
            Plan::Pulses { .. } => self.phase = Phase::Sequence(0),
            Plan::Direct(direct) if direct.each == 0 || direct.samples.is_empty() => {
                self.next_block();
            }
            Plan::Direct(_) => self.phase = Phase::Sample { byte: 0, bit: 0 },
            Plan::Jump { to } => match usize::try_from(to) {
                // Off the front of the tape is off the tape: a file that jumps
                // to block -3 is corrupt, and the alternative to stopping is
                // playing something it did not mean.
                Err(_) => self.phase = Phase::Done,
                Ok(to) => {
                    self.block = to;
                    self.phase = Phase::Start;
                }
            },
            Plan::Loop { count } => {
                self.loop_start = self.block + 1;
                self.loop_left = count;
                self.next_block();
            }
            Plan::LoopEnd => {
                if self.loop_left > 1 {
                    self.loop_left -= 1;
                    self.block = self.loop_start;
                    self.phase = Phase::Start;
                } else {
                    self.loop_left = 0;
                    self.next_block();
                }
            }
            Plan::Level { level } => {
                self.level = level;
                self.next_block();
            }
            Plan::Nothing => self.next_block(),
            Plan::Stop => {
                self.block += 1;
                self.phase = Phase::Stopped;
            }
        }
    }

    fn next_block(&mut self) {
        self.block += 1;
        self.phase = Phase::Start;
    }

    fn flip(&mut self, ticks: u32) -> Pulse {
        self.level = !self.level;
        Pulse {
            ticks,
            level: self.level,
        }
    }
}

/// Bits of byte `byte` that are data: all of them, except in the last byte of
/// a block that said otherwise.
fn bits_in(data: &Data<'_>, byte: usize) -> u8 {
    if byte + 1 == data.bytes.len() {
        data.last_bits.clamp(1, 8)
    } else {
        8
    }
}

fn first_pilot(data: &Data<'_>) -> Phase {
    if data.pilot_pulses > 0 && data.pilot > 0 {
        Phase::Pilot(data.pilot_pulses)
    } else {
        after_pilot(data)
    }
}

fn after_pilot(data: &Data<'_>) -> Phase {
    if data.sync_first > 0 {
        Phase::SyncFirst
    } else {
        after_sync_first(data)
    }
}

fn after_sync_first(data: &Data<'_>) -> Phase {
    if data.sync_second > 0 {
        Phase::SyncSecond
    } else {
        first_bit(data)
    }
}

fn first_bit(data: &Data<'_>) -> Phase {
    if data.bytes.is_empty() {
        Phase::Tail
    } else {
        Phase::Bit {
            byte: 0,
            bit: 0,
            second: false,
        }
    }
}

/// A run of samples at the same level, which is one pulse.
struct Run {
    level: bool,
    ticks: u32,
    /// Where the recording has got to after it.
    byte: usize,
    bit: u8,
}

/// The pulse starting at sample (`byte`, `bit`): its level, and how long the
/// line stays there.
///
/// Samples are merged rather than emitted one at a time because a recording is
/// sampled at tens of T-states and a level that holds for a millisecond is
/// forty of them — forty scheduled events, forty wakeups of the emulation
/// thread, for one edge.
fn run_of_samples(direct: &Direct<'_>, byte: usize, bit: u8) -> Option<Run> {
    let sample = |byte: usize, bit: u8| -> Option<bool> {
        let &value = direct.samples.get(byte)?;
        let bits = if byte + 1 == direct.samples.len() {
            direct.last_bits.clamp(1, 8)
        } else {
            8
        };
        (bit < bits).then(|| value & (0x80 >> bit) != 0)
    };

    let level = sample(byte, bit)?;
    let (mut byte, mut bit, mut ticks) = (byte, bit, direct.each);
    loop {
        let (next_byte, next_bit) = if bit == 7 {
            (byte + 1, 0)
        } else {
            (byte, bit + 1)
        };
        match sample(next_byte, next_bit) {
            Some(next) if next == level => {
                let Some(longer) = ticks.checked_add(direct.each) else {
                    // Nearly four billion T-states of one level is a tape
                    // nobody wrote; breaking the run here costs an edge that
                    // changes nothing and keeps the arithmetic honest.
                    break;
                };
                ticks = longer;
                byte = next_byte;
                bit = next_bit;
            }
            _ => break,
        }
    }
    let (byte, bit) = if bit == 7 {
        (byte + 1, 0)
    } else {
        (byte, bit + 1)
    };
    Some(Run {
        level,
        ticks,
        byte,
        bit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tap;
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
        ms: 7,
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
        // A millisecond, which is what a TZX pause is counted in.
        assert_eq!(timing.ms, 3500);
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
