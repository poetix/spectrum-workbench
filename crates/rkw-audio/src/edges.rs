//! What the speaker did, and when.
//!
//! Sound out of a Spectrum is one bit of port `0xFE` — bit 4, the speaker —
//! and a program makes a note by flipping it at the right rate. Bit 3, the
//! `MIC` output, is nominally for tape saving, but on a 48K board it feeds the
//! same amplifier at a lower weight, so the port has four output levels rather
//! than two and the beeper engines that care about amplitude use all of them.
//! Both bits are recorded here; what the weighting is belongs to whoever turns
//! levels into a waveform, not to the log.
//!
//! # A level is silent; only a change is a sound
//!
//! Holding the speaker at one makes no more noise than holding it at zero,
//! because a cone that is not moving is not moving air. So the thing worth
//! recording is not the level but the moment it changed, and an [`EdgeLog`] is
//! a list of those moments with the level each one moved to. Between two edges
//! nothing happened, however long the gap, and that is what lets one frame of
//! sound be a few hundred `u32`s instead of 69,888 samples of a bit.
//!
//! # Why it saturates rather than wrapping
//!
//! The log is a fixed array, because it lives inside the `Ula` and therefore
//! inside every checkpoint ticket 0027 will take, and nothing on the emulation
//! thread allocates (ADR-0007). When it fills it stops recording and counts
//! what it missed.
//!
//! Overwrite-oldest — the policy the event ring uses — is not merely worse
//! here, it is unavailable: everything downstream is a single forward walk
//! over ticks that only ever increase, and a wrapped buffer has no such order
//! to walk. Saturating loses the tail of one frame, which is a fifth of a
//! second at worst and audible as a gap; wrapping would deliver that frame's
//! transitions in the wrong order, which is audible as a bang.
//!
//! What saturation must not do is lose the *level*. [`EdgeLog::record`] keeps
//! the levels in force whether or not it had room to write them down, so a
//! truncated frame still hands the next one the right level to start from and
//! the speaker cannot be left stuck at the wrong offset for 20 ms.
//!
//! # Ticks are relative to the frame, and may run past its end
//!
//! An edge is stamped with its T-state offset into the frame being recorded,
//! and [`EdgeLog::roll`] moves the log on at the frame boundary. Offsets past
//! the end of the frame are ordinary and are carried over rather than clamped:
//! a slice ends on the last instruction to *start* before its deadline, so the
//! clock routinely overruns by twenty-odd T-states, and single-stepping in the
//! debugger does not service the frame interrupt at all, so it can carry the
//! clock several whole frames past the last `roll`. Clamping would be
//! inaudible for the first case and silently wrong for the second.

/// Edges one frame can hold before the log starts counting instead of
/// recording.
///
/// A Fluidcore-style engine driving one PWM frame every 152 T-states produces
/// about 920 edges in a Spectrum frame, which is the densest thing real
/// software does; the ceiling an `OUT (n),A` loop could reach is about 6,350
/// and is a 30 kHz tone nobody can hear. This sits above the first and below
/// the second, at 8 KB inside every `Ula`.
pub const MAX_EDGES: usize = 2048;

/// The largest T-state offset an edge can carry, the two low bits of the
/// packed word being the levels. About 300 seconds of Z80 clock, against a
/// frame of 69,888 T-states.
pub const MAX_TICK: u32 = u32::MAX >> 2;

const SPEAKER: u32 = 0b10;
const MIC: u32 = 0b01;

/// The two bits of port `0xFE` that reach the amplifier.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Levels {
    /// Bit 4, the speaker.
    pub speaker: bool,
    /// Bit 3, the `MIC` output, which drives the same amplifier more quietly.
    pub mic: bool,
}

impl Levels {
    /// The audio bits of a byte written to port `0xFE`. The other six —
    /// three of border and three of nothing — are not this module's business.
    pub const fn from_port(value: u8) -> Levels {
        Levels {
            speaker: value & 0x10 != 0,
            mic: value & 0x08 != 0,
        }
    }

    /// The level these bits put on the speaker, in `0.0..=1.0`.
    ///
    /// `mic_level` is `MIC`'s share of it, so `0.2` gives the four levels
    /// `0.0, 0.2, 0.8, 1.0` and `0.0` gives a machine where only bit 4 is
    /// wired up. The real ratio is a resistor network that varies by board
    /// issue and has never, as far as anyone has written down, been measured
    /// properly; it is a parameter for that reason and not because anything
    /// wants to sweep it.
    pub fn amplitude(self, mic_level: f32) -> f32 {
        let speaker = if self.speaker { 1.0 - mic_level } else { 0.0 };
        let mic = if self.mic { mic_level } else { 0.0 };
        speaker + mic
    }
}

/// Pack a T-state offset and the levels it moved to into one word.
///
/// Offsets above [`MAX_TICK`] saturate. Reaching one would mean five minutes
/// of emulated time between frame boundaries.
pub const fn pack(tick: u32, levels: Levels) -> u32 {
    let tick = if tick > MAX_TICK { MAX_TICK } else { tick };
    let speaker = if levels.speaker { SPEAKER } else { 0 };
    let mic = if levels.mic { MIC } else { 0 };
    (tick << 2) | speaker | mic
}

/// The T-state offset of a packed edge.
pub const fn tick_of(edge: u32) -> u32 {
    edge >> 2
}

/// The levels a packed edge moved to.
pub const fn levels_of(edge: u32) -> Levels {
    Levels {
        speaker: edge & SPEAKER != 0,
        mic: edge & MIC != 0,
    }
}

/// One frame's speaker edges, and the level the frame began at.
#[derive(Clone)]
pub struct EdgeLog {
    edges: [u32; MAX_EDGES],
    len: usize,
    /// The levels in force at tick zero of the frame being recorded.
    start: Levels,
    /// The levels in force now. Not derived from the log, so that saturation
    /// cannot lose track of where the speaker actually is.
    now: Levels,
    /// Edges there was no room for, ever. Cumulative, like the event ring's
    /// drop count, and reported for the same reason.
    dropped: u32,
}

impl Default for EdgeLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Equality over the edges that mean something.
///
/// Derived equality would compare the whole array including whatever the last
/// long frame left past `len`, so two logs holding the same sound would differ
/// on their history. Ticket 0027's checkpoint self-check compares whole
/// machines; a derived impl here would make it report divergences that are not
/// there.
impl PartialEq for EdgeLog {
    fn eq(&self, other: &EdgeLog) -> bool {
        self.edges[..self.len] == other.edges[..other.len]
            && self.start == other.start
            && self.now == other.now
            && self.dropped == other.dropped
    }
}

impl Eq for EdgeLog {}

impl std::fmt::Debug for EdgeLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeLog")
            .field("edges", &self.len)
            .field("start", &self.start)
            .field("now", &self.now)
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl EdgeLog {
    /// An empty log with the speaker low, which is where a machine comes up.
    pub const fn new() -> EdgeLog {
        EdgeLog {
            edges: [0; MAX_EDGES],
            len: 0,
            start: Levels {
                speaker: false,
                mic: false,
            },
            now: Levels {
                speaker: false,
                mic: false,
            },
            dropped: 0,
        }
    }

    /// Note that port `0xFE` now reads `levels`, at `tick` T-states into the
    /// frame.
    ///
    /// A write that leaves both audio bits where they were records nothing,
    /// which is what keeps a border-striping program — thousands of writes a
    /// frame, none of them a sound — from filling the log.
    pub fn record(&mut self, tick: u32, levels: Levels) {
        if levels == self.now {
            return;
        }
        // Before the room check, so that a full log still knows where the
        // speaker is.
        self.now = levels;

        if self.len == MAX_EDGES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.edges[self.len] = pack(tick, levels);
        self.len += 1;
    }

    /// The frame so far: every edge recorded since the last [`roll`](Self::roll),
    /// in increasing T-state order, and the levels the frame started at.
    ///
    /// Edges past the end of the frame may be in here; see the module note.
    /// A consumer bounds its own window and simply never reaches them.
    pub fn frame(&self) -> (&[u32], Levels) {
        (&self.edges[..self.len], self.start)
    }

    /// Move on to the next frame: drop everything inside this one, and rebase
    /// what overran it onto the frame that is starting.
    ///
    /// Idempotent in the sense that matters — calling it repeatedly against a
    /// clock several frames late peels one frame each time and converges.
    pub fn roll(&mut self, frame_ticks: u32) {
        let split = self.edges[..self.len]
            .iter()
            .position(|&edge| tick_of(edge) >= frame_ticks)
            .unwrap_or(self.len);

        // Where the speaker is as the new frame opens. When the whole log
        // belongs to the frame being closed, that is simply where it is now —
        // which is also the only answer that survives saturation, since the
        // last edge in a truncated log is not the last edge that happened.
        // When edges overran, the truth is the one in force at the boundary.
        self.start = if split == self.len {
            self.now
        } else if split == 0 {
            self.start
        } else {
            levels_of(self.edges[split - 1])
        };

        self.edges.copy_within(split..self.len, 0);
        self.len -= split;
        for edge in &mut self.edges[..self.len] {
            *edge = pack(tick_of(*edge) - frame_ticks, levels_of(*edge));
        }
    }

    /// The levels in force now, which is what port `0xFE` last put there
    /// whether or not there was room to record it.
    pub fn levels(&self) -> Levels {
        self.now
    }

    /// Edges there was no room for, over the life of the machine.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIGH: Levels = Levels {
        speaker: true,
        mic: false,
    };
    const LOW: Levels = Levels {
        speaker: false,
        mic: false,
    };
    const BOTH: Levels = Levels {
        speaker: true,
        mic: true,
    };

    const FRAME: u32 = 69_888;

    fn ticks(log: &EdgeLog) -> Vec<u32> {
        log.frame().0.iter().copied().map(tick_of).collect()
    }

    #[test]
    fn a_packed_edge_round_trips_its_tick_and_its_levels() {
        for levels in [LOW, HIGH, BOTH, Levels { speaker: false, mic: true }] {
            for tick in [0, 1, 3, FRAME - 1, FRAME, MAX_TICK] {
                let edge = pack(tick, levels);
                assert_eq!(tick_of(edge), tick, "{tick} {levels:?}");
                assert_eq!(levels_of(edge), levels, "{tick} {levels:?}");
            }
        }
    }

    #[test]
    fn the_audio_bits_of_port_fe_are_bits_four_and_three() {
        assert_eq!(Levels::from_port(0x00), LOW);
        assert_eq!(Levels::from_port(0x10), HIGH);
        assert_eq!(Levels::from_port(0x18), BOTH);
        // The border is three bits of the same byte and none of this module's
        // business.
        assert_eq!(Levels::from_port(0xE7), LOW);
        assert_eq!(Levels::from_port(0xFF), BOTH);
    }

    #[test]
    fn mic_takes_its_share_of_the_amplitude_and_leaves_the_rest_to_the_speaker() {
        assert_eq!(LOW.amplitude(0.2), 0.0);
        assert_eq!(HIGH.amplitude(0.2), 0.8);
        assert_eq!(Levels { speaker: false, mic: true }.amplitude(0.2), 0.2);
        assert_eq!(BOTH.amplitude(0.2), 1.0);
        // A machine where only bit 4 is wired up.
        assert_eq!(BOTH.amplitude(0.0), 1.0);
        assert_eq!(Levels { speaker: false, mic: true }.amplitude(0.0), 0.0);
    }

    #[test]
    fn a_write_that_changes_neither_audio_bit_records_nothing() {
        let mut log = EdgeLog::new();
        // A border striper: every colour in turn, the speaker untouched.
        for line in 0..312u32 {
            log.record(line * 224, Levels::from_port((line % 8) as u8));
        }
        assert_eq!(ticks(&log), Vec::<u32>::new());
        assert_eq!(log.dropped(), 0);

        // And the first write that does move bit 4 is recorded.
        log.record(1000, HIGH);
        assert_eq!(ticks(&log), vec![1000]);
    }

    #[test]
    fn the_frame_begins_at_the_level_the_last_one_left() {
        let mut log = EdgeLog::new();
        assert_eq!(log.frame().1, LOW);

        log.record(100, HIGH);
        log.roll(FRAME);
        assert_eq!(log.frame().1, HIGH);
        assert_eq!(ticks(&log), Vec::<u32>::new());

        // Nothing happens for a frame, and the level carries on carrying.
        log.roll(FRAME);
        assert_eq!(log.frame().1, HIGH);
    }

    #[test]
    fn an_edge_past_the_end_of_the_frame_belongs_to_the_next_one() {
        let mut log = EdgeLog::new();
        log.record(100, HIGH);
        log.record(FRAME + 17, LOW);
        log.roll(FRAME);

        assert_eq!(ticks(&log), vec![17]);
        assert_eq!(levels_of(log.frame().0[0]), LOW);
        // The frame opens at the level in force at the boundary, which is the
        // one the overrunning edge moved away from.
        assert_eq!(log.frame().1, HIGH);
    }

    #[test]
    fn rolling_twice_peels_two_frames_off_a_clock_that_is_two_frames_late() {
        let mut log = EdgeLog::new();
        log.record(50, HIGH);
        log.record(FRAME + 50, LOW);
        log.record(2 * FRAME + 50, HIGH);

        log.roll(FRAME);
        assert_eq!(ticks(&log), vec![50, FRAME + 50]);
        assert_eq!(log.frame().1, HIGH);

        log.roll(FRAME);
        assert_eq!(ticks(&log), vec![50]);
        assert_eq!(log.frame().1, LOW);

        log.roll(FRAME);
        assert_eq!(ticks(&log), Vec::<u32>::new());
        assert_eq!(log.frame().1, HIGH);
    }

    #[test]
    fn a_full_log_counts_what_it_missed_and_still_knows_where_the_speaker_is() {
        let mut log = EdgeLog::new();
        // Twice the capacity of alternating edges and one more, so that half
        // are lost and the last one lost moves the speaker somewhere the log
        // has no room to say.
        for i in 0..(2 * MAX_EDGES + 1) as u32 {
            log.record(i * 16, if i % 2 == 0 { HIGH } else { LOW });
        }
        assert_eq!(log.frame().0.len(), MAX_EDGES);
        assert_eq!(log.dropped(), MAX_EDGES as u32 + 1);

        // The last edge written down was an odd one, and so low; the last edge
        // that happened was an even one, and so high. The log knows the
        // difference.
        assert_eq!(levels_of(log.frame().0[MAX_EDGES - 1]), LOW);
        assert_eq!(log.levels(), HIGH);

        // Which is what the next frame has to start from — the truth, not the
        // last thing there was room to record.
        log.roll(FRAME);
        assert_eq!(log.frame().1, HIGH);
        assert_eq!(ticks(&log), Vec::<u32>::new());
    }

    #[test]
    fn two_logs_holding_the_same_sound_are_equal_whatever_they_held_before() {
        let mut busy = EdgeLog::new();
        for i in 0..500u32 {
            busy.record(i * 100, if i % 2 == 0 { HIGH } else { LOW });
        }
        busy.roll(FRAME);
        busy.record(1234, HIGH);

        let mut quiet = EdgeLog::new();
        // Same state, reached without ever filling the array past one entry.
        quiet.record(0, LOW);
        quiet.record(1234, HIGH);

        assert_eq!(busy.frame().0.len(), 1);
        assert_eq!(busy, quiet);
    }
}
