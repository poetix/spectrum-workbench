//! TZX: the same tape, with the timings written down.
//!
//! A TAP file says what the bytes were and leaves the waveform to be guessed
//! from the ROM's constants, which is why it can only hold tapes the ROM could
//! have written. TZX says what the *signal* was: how long a pilot pulse is, how
//! long a one is, how many bits of the last byte count, and — for the loaders
//! that gave up on bit-per-cycle encoding altogether — a list of pulse lengths
//! or a recording of the line itself.
//!
//! So a TZX file is a header and then a sequence of blocks, each an id byte and
//! a payload whose shape the id decides. Most of them are waveform; the rest
//! are either control flow (jump, loop, pause, stop) or text for a person
//! (title, publisher, description).
//!
//! # What parsing does and does not do
//!
//! [`Tzx::parse`] keeps the file's bytes and records, per block, the numbers in
//! its header and where its payload is. It does not turn anything into pulses:
//! that is [`crate::pulse::Player`]'s business, and it works from a [`Plan`]
//! this module hands it a block at a time. The split is the same one [`crate::tap`]
//! makes, and for the same reason — the index has to be walkable by block
//! number from inside an emulated machine that gets cloned into checkpoints.
//!
//! [`Plan`]: crate::pulse::Plan
//!
//! # Unknown blocks are skipped, not refused
//!
//! The format has been extended repeatedly and files in the wild carry blocks
//! from versions this code has never heard of. The specification's own rule for
//! that case is that the four bytes after an unrecognised id are its length, so
//! an unknown block costs a seek and a note in the index rather than a refusal
//! to open the file. The deprecated C64 blocks, the CSW recording and the
//! generalized data block are all skipped by exactly that rule: they are known
//! to exist, they are not waveform this player can produce, and their length
//! field is in the same place.
//!
//! ```
//! use rkw_tape::{Timing, Tzx};
//!
//! // A turbo block: the loader's own pulse lengths, not the ROM's.
//! let tzx = Tzx::builder()
//!     .turbo(&rkw_tape::tzx::Turbo {
//!         pilot: 1000,
//!         pilot_pulses: 2,
//!         sync_first: 100,
//!         sync_second: 100,
//!         zero: 200,
//!         one: 400,
//!         last_bits: 8,
//!         pause_ms: 0,
//!     }, &[0xFF])
//!     .build();
//!
//! assert_eq!(tzx.len(), 1);
//! assert_eq!(tzx.version(), (1, 20));
//! ```

use std::fmt;

use crate::pulse::{Data, Direct, Plan, Playable, Timing};

/// The eight bytes every TZX file starts with: `ZXTape!` and an end-of-file
/// character, which is there so that `TYPE`ing the file on a DOS machine
/// stopped at the signature instead of filling the screen.
pub const MAGIC: &[u8; 8] = b"ZXTape!\x1a";

/// The version this code writes and understands the blocks of. Files claiming
/// a later minor version are read anyway: the format's own extension rule
/// makes a block it has never seen skippable.
pub const VERSION: (u8, u8) = (1, 20);

/// Where a block's payload is in the file.
///
/// Offsets rather than slices, because [`Tzx`] owns its bytes and a block that
/// borrowed from them could not live in the same struct.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// Offset of the first byte, from the start of the file.
    pub at: usize,
    pub len: usize,
}

impl Span {
    const EMPTY: Span = Span { at: 0, len: 0 };
}

/// One block of a TZX file: what its header said, and where its payload is.
///
/// The waveform variants carry their timings as they were written, in
/// T-states, except for pauses, which the format keeps in milliseconds and
/// which therefore cannot be turned into T-states without knowing the clock —
/// [`Timing::ms`] is where that arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TzxBlock {
    /// `0x10`. A TAP block with a pause: the ROM's own timings, and the pilot
    /// length chosen by the flag byte as the ROM chooses it.
    StandardData { pause_ms: u16, data: Span },
    /// `0x11`. The same shape with every length spelled out, which is what a
    /// commercial loader needs: it is the pulse lengths that make it fast.
    TurboData {
        pilot: u16,
        sync_first: u16,
        sync_second: u16,
        zero: u16,
        one: u16,
        pilot_pulses: u16,
        /// Bits of the last byte that are part of the data, 1 to 8. A loader
        /// that ends mid-byte is not an accident; some of them encode a
        /// checksum in the tail of the last one.
        last_bits: u8,
        pause_ms: u16,
        data: Span,
    },
    /// `0x12`. A run of identical pulses, which is a pilot tone for a loader
    /// that wants to build its block header out of parts.
    PureTone { length: u16, pulses: u16 },
    /// `0x13`. A handful of pulses of stated lengths, `pulses` being little-
    /// endian words in the file.
    PulseSequence { pulses: Span },
    /// `0x14`. Data with no pilot and no sync, for a loader that has already
    /// found its own timing.
    PureData {
        zero: u16,
        one: u16,
        last_bits: u8,
        pause_ms: u16,
        data: Span,
    },
    /// `0x15`. The line itself, sampled: one bit per sample, most significant
    /// first, each held for `each` T-states. What a tape that no encoding
    /// describes gets written down as.
    DirectRecording {
        each: u16,
        pause_ms: u16,
        last_bits: u8,
        samples: Span,
    },
    /// `0x20`. Silence, or — at zero milliseconds — a request to stop the tape
    /// and wait for the person at the keyboard.
    Pause { ms: u16 },
    /// `0x21`. The name of a group of blocks, for a listing.
    GroupStart { name: Span },
    /// `0x22`.
    GroupEnd,
    /// `0x23`. Relative to this block, so `1` is the next one and `-1` is the
    /// one before. Zero means jump to itself, which the format defines as
    /// looping forever.
    Jump { offset: i16 },
    /// `0x24`. Repeat the blocks up to the matching [`TzxBlock::LoopEnd`] this
    /// many times in total.
    LoopStart { count: u16 },
    /// `0x25`.
    LoopEnd,
    /// `0x2A`. Stop the tape if this is a 48K machine, which it is.
    StopIf48k,
    /// `0x2B`. Where the line sits before the next pulse.
    SetLevel { level: bool },
    /// `0x30`, and `0x31` with the number of seconds it asks to be shown for.
    Text { seconds: Option<u8>, text: Span },
    /// `0x32`. Title, publisher, author and the rest, as `(id, text)` pairs in
    /// the payload rather than parsed here: see [`Tzx::archive_info`].
    ArchiveInfo { body: Span },
    /// `0x33`. Machines the tape is known to run on, three bytes an entry.
    Hardware { body: Span },
    /// `0x35`. Somebody else's data under a ten-character name.
    Custom { id: Span, body: Span },
    /// `0x5A`. Ninety bytes of nothing, put between two files concatenated with
    /// `copy /b` so that a reader can find its footing again.
    Glue,
    /// A block this player has no waveform for, skipped by its length field:
    /// the deprecated C64 blocks, CSW and generalized data, anything from a
    /// later version of the format, and the select-block and call-sequence
    /// blocks, which ask a person or a stack to decide what plays next.
    Skipped { id: u8, body: Span },
}

impl TzxBlock {
    /// The block's id byte, as it is written in the file.
    pub const fn id(&self) -> u8 {
        match self {
            TzxBlock::StandardData { .. } => 0x10,
            TzxBlock::TurboData { .. } => 0x11,
            TzxBlock::PureTone { .. } => 0x12,
            TzxBlock::PulseSequence { .. } => 0x13,
            TzxBlock::PureData { .. } => 0x14,
            TzxBlock::DirectRecording { .. } => 0x15,
            TzxBlock::Pause { .. } => 0x20,
            TzxBlock::GroupStart { .. } => 0x21,
            TzxBlock::GroupEnd => 0x22,
            TzxBlock::Jump { .. } => 0x23,
            TzxBlock::LoopStart { .. } => 0x24,
            TzxBlock::LoopEnd => 0x25,
            TzxBlock::StopIf48k => 0x2A,
            TzxBlock::SetLevel { .. } => 0x2B,
            TzxBlock::Text { seconds: None, .. } => 0x30,
            TzxBlock::Text { .. } => 0x31,
            TzxBlock::ArchiveInfo { .. } => 0x32,
            TzxBlock::Hardware { .. } => 0x33,
            TzxBlock::Custom { .. } => 0x35,
            TzxBlock::Glue => 0x5A,
            TzxBlock::Skipped { id, .. } => *id,
        }
    }

    /// What kind of thing this is, in a word, for a listing.
    pub const fn kind(&self) -> &'static str {
        match self {
            TzxBlock::StandardData { .. } => "standard data",
            TzxBlock::TurboData { .. } => "turbo data",
            TzxBlock::PureTone { .. } => "pure tone",
            TzxBlock::PulseSequence { .. } => "pulse sequence",
            TzxBlock::PureData { .. } => "pure data",
            TzxBlock::DirectRecording { .. } => "direct recording",
            TzxBlock::Pause { ms: 0 } => "stop the tape",
            TzxBlock::Pause { .. } => "pause",
            TzxBlock::GroupStart { .. } => "group start",
            TzxBlock::GroupEnd => "group end",
            TzxBlock::Jump { .. } => "jump",
            TzxBlock::LoopStart { .. } => "loop start",
            TzxBlock::LoopEnd => "loop end",
            TzxBlock::StopIf48k => "stop if 48K",
            TzxBlock::SetLevel { .. } => "set level",
            TzxBlock::Text { .. } => "text",
            TzxBlock::ArchiveInfo { .. } => "archive info",
            TzxBlock::Hardware { .. } => "hardware type",
            TzxBlock::Custom { .. } => "custom info",
            TzxBlock::Glue => "glue",
            TzxBlock::Skipped { .. } => "skipped",
        }
    }
}

/// The turbo block's timings, which is the whole of what makes a loader fast.
///
/// A struct because there are eight of them and a call taking eight bare
/// integers is a bug waiting for somebody to swap two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Turbo {
    pub pilot: u16,
    pub pilot_pulses: u16,
    pub sync_first: u16,
    pub sync_second: u16,
    pub zero: u16,
    pub one: u16,
    /// Bits of the last byte that count, 1 to 8.
    pub last_bits: u8,
    pub pause_ms: u16,
}

impl Default for Turbo {
    /// The ROM's own numbers, which is what a turbo block that has not been
    /// told otherwise should sound like.
    fn default() -> Turbo {
        Turbo {
            pilot: 2168,
            pilot_pulses: 3223,
            sync_first: 667,
            sync_second: 735,
            zero: 855,
            one: 1710,
            last_bits: 8,
            pause_ms: 1000,
        }
    }
}

/// A TZX file: its bytes, its version, and the blocks in it.
#[derive(Clone, PartialEq, Eq)]
pub struct Tzx {
    bytes: Vec<u8>,
    version: (u8, u8),
    blocks: Vec<TzxBlock>,
}

impl Tzx {
    /// Read a TZX file.
    ///
    /// Refuses a file that does not start with [`MAGIC`], and one whose blocks
    /// run off the end. Everything else is accepted, including block types this
    /// player cannot make a sound out of and data that no loader would agree
    /// with: what a tape image says is the tape's business, and the machine's
    /// response to a bad one is a thing worth being able to test.
    pub fn parse(bytes: &[u8]) -> Result<Tzx, TzxError> {
        if bytes.len() < 10 || &bytes[..8] != MAGIC {
            return Err(TzxError::NotATzx);
        }
        let version = (bytes[8], bytes[9]);
        let mut cursor = Cursor { bytes, at: 10 };
        let mut blocks = Vec::new();
        while cursor.at < bytes.len() {
            blocks.push(cursor.block()?);
        }
        Ok(Tzx {
            bytes: bytes.to_vec(),
            version,
            blocks,
        })
    }

    /// A file with a header and no blocks.
    pub fn empty() -> Tzx {
        Tzx {
            bytes: header_bytes(),
            version: VERSION,
            blocks: Vec::new(),
        }
    }

    /// Assemble a file a block at a time.
    pub fn builder() -> Builder {
        Builder {
            bytes: header_bytes(),
        }
    }

    /// The version the file claims, as `(major, minor)`.
    pub const fn version(&self) -> (u8, u8) {
        self.version
    }

    /// Blocks in the file, of every kind — waveform, control and text alike.
    /// Block numbers are indexes into this, which is what a jump is relative
    /// to and what a tape counter counts.
    pub fn blocks(&self) -> &[TzxBlock] {
        &self.blocks
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Block `index`, or `None` past the end.
    pub fn block(&self, index: usize) -> Option<TzxBlock> {
        self.blocks.get(index).copied()
    }

    /// The file, as it would be written out.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes a span covers.
    pub fn span(&self, span: Span) -> &[u8] {
        &self.bytes[span.at..span.at + span.len]
    }

    /// FNV-1a over the file, which is what ADR-0017 asks a recorded session to
    /// name its tape by. The same function [`crate::Tap::hash`] uses, over the
    /// same kind of thing: a file that changed has to be visible as a refusal
    /// rather than as a divergence with no cause.
    pub fn hash(&self) -> u64 {
        crate::tap::fnv1a(&self.bytes)
    }

    /// The text of a text or message block, and of a group start.
    ///
    /// The format's character set is the Spectrum's, which above `0x7F`
    /// disagrees with everything; those bytes come back as `?` rather than
    /// guessed at, exactly as a header name does.
    pub fn text(&self, span: Span) -> String {
        self.span(span)
            .iter()
            .map(|&b| {
                if (32..127).contains(&b) {
                    b as char
                } else {
                    '?'
                }
            })
            .collect()
    }

    /// The archive information: title, publisher, author, year and the rest,
    /// in the order the file gives them.
    ///
    /// A malformed entry — one whose length runs past the end of the block —
    /// ends the list rather than failing, because by the time anybody reads
    /// this the tape is already mounted and the text is the least of what it
    /// is for.
    pub fn archive_info(&self) -> Vec<(ArchiveId, String)> {
        let mut out = Vec::new();
        for block in &self.blocks {
            let TzxBlock::ArchiveInfo { body } = *block else {
                continue;
            };
            let end = body.at + body.len;
            let Some(&count) = self.bytes.get(body.at) else {
                continue;
            };
            // Offsets into the file rather than a slice walked with pattern
            // matching, because what comes out is a `Span` and a span is where
            // a thing is, not what it is.
            let mut at = body.at + 1;
            for _ in 0..count {
                if at + 2 > end {
                    break;
                }
                let id = self.bytes[at];
                let len = usize::from(self.bytes[at + 1]);
                if at + 2 + len > end {
                    break;
                }
                out.push((ArchiveId::of(id), self.text(Span { at: at + 2, len })));
                at += 2 + len;
            }
        }
        out
    }

    /// Every text block and message in the file, in order.
    pub fn descriptions(&self) -> Vec<String> {
        self.blocks
            .iter()
            .filter_map(|block| match *block {
                TzxBlock::Text { text, .. } => Some(self.text(text)),
                _ => None,
            })
            .collect()
    }

    /// What block `index` plays, in the terms [`crate::pulse::Player`] works
    /// in. `None` past the end of the tape.
    ///
    /// This is where the format's numbers become a waveform, and where the two
    /// things TZX keeps that TAP does not — a pause in milliseconds, and a
    /// last byte that is not eight bits long — are turned into T-states and
    /// into a bit count. The pause is split into the edge that ends the last
    /// bit and the silence after it, for the reason [`Data::tail`] gives.
    pub fn plan(&self, index: usize, timing: &Timing) -> Option<Plan<'_>> {
        let block = self.blocks.get(index)?;
        Some(match *block {
            TzxBlock::StandardData { pause_ms, data } => {
                let bytes = self.span(data);
                let header = bytes.first().is_some_and(|&flag| flag < 0x80);
                Plan::Data(Data {
                    pilot: timing.pilot,
                    pilot_pulses: timing.pilot_pulses(header),
                    sync_first: timing.sync_first,
                    sync_second: timing.sync_second,
                    zero: timing.zero,
                    one: timing.one,
                    last_bits: 8,
                    tail: timing.tail_for(pause_ms),
                    pause: timing.pause_for(pause_ms),
                    bytes,
                })
            }
            TzxBlock::TurboData {
                pilot,
                sync_first,
                sync_second,
                zero,
                one,
                pilot_pulses,
                last_bits,
                pause_ms,
                data,
            } => Plan::Data(Data {
                pilot: u32::from(pilot),
                pilot_pulses: u32::from(pilot_pulses),
                sync_first: u32::from(sync_first),
                sync_second: u32::from(sync_second),
                zero: u32::from(zero),
                one: u32::from(one),
                last_bits,
                tail: timing.tail_for(pause_ms),
                pause: timing.pause_for(pause_ms),
                bytes: self.span(data),
            }),
            TzxBlock::PureData {
                zero,
                one,
                last_bits,
                pause_ms,
                data,
            } => Plan::Data(Data {
                pilot: 0,
                pilot_pulses: 0,
                sync_first: 0,
                sync_second: 0,
                zero: u32::from(zero),
                one: u32::from(one),
                last_bits,
                tail: timing.tail_for(pause_ms),
                pause: timing.pause_for(pause_ms),
                bytes: self.span(data),
            }),
            TzxBlock::PureTone { length, pulses } => Plan::Tone {
                length: u32::from(length),
                pulses: u32::from(pulses),
            },
            TzxBlock::PulseSequence { pulses } => Plan::Pulses {
                lengths: self.span(pulses),
            },
            TzxBlock::DirectRecording {
                each,
                pause_ms,
                last_bits,
                samples,
            } => Plan::Direct(Direct {
                each: u32::from(each),
                last_bits,
                // A recording's levels are absolute, so there is no edge to
                // manufacture at the end of one: the pause is silence and
                // nothing else.
                pause: timing.tail_for(pause_ms) + timing.pause_for(pause_ms),
                samples: self.span(samples),
            }),
            // A pause is a data block with no data: the same terminating edge,
            // the same silence, and nothing in between. That is not a trick —
            // it is what a pause block is for, which is to end the block before
            // it in a file that wrote that block with no pause of its own.
            TzxBlock::Pause { ms: 0 } => Plan::Stop,
            TzxBlock::Pause { ms } => Plan::Data(Data {
                pilot: 0,
                pilot_pulses: 0,
                sync_first: 0,
                sync_second: 0,
                zero: 0,
                one: 0,
                last_bits: 8,
                tail: timing.tail_for(ms),
                pause: timing.pause_for(ms),
                bytes: &[],
            }),
            TzxBlock::StopIf48k => Plan::Stop,
            TzxBlock::Jump { offset } => Plan::Jump {
                to: index as isize + isize::from(offset),
            },
            TzxBlock::LoopStart { count } => Plan::Loop { count },
            TzxBlock::LoopEnd => Plan::LoopEnd,
            TzxBlock::SetLevel { level } => Plan::Level { level },
            // Text, group markers, hardware types, custom and unknown data:
            // nothing to play, and the tape moves on.
            _ => Plan::Nothing,
        })
    }
}

impl Playable for Tzx {
    fn plan(&self, index: usize, timing: &Timing) -> Option<Plan<'_>> {
        Tzx::plan(self, index, timing)
    }
}

impl fmt::Debug for Tzx {
    /// The shape of the file rather than its contents, as [`crate::Tap`]'s
    /// does: a tape is megabytes and a debug line is a line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tzx")
            .field(
                "version",
                &format_args!("{}.{}", self.version.0, self.version.1),
            )
            .field("blocks", &self.len())
            .field("bytes", &self.bytes.len())
            .field("hash", &format_args!("{:#018x}", self.hash()))
            .finish()
    }
}

impl fmt::Display for Tzx {
    /// A listing: what the file says about itself, and then its blocks.
    ///
    /// This is the "displayed" half of the text and archive info blocks. They
    /// exist for a person — the title of the game, who published it, and the
    /// loading instructions — and a tape image that shows a person nothing is
    /// where the wrong tape gets loaded for ten minutes before anybody notices.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "TZX {}.{}, {} block{}",
            self.version.0,
            self.version.1,
            self.len(),
            if self.len() == 1 { "" } else { "s" }
        )?;
        for (id, text) in self.archive_info() {
            writeln!(f, "  {}: {}", id.label(), text.trim_end())?;
        }
        for text in self.descriptions() {
            writeln!(f, "  {}", text.trim_end())?;
        }
        for (index, block) in self.blocks.iter().enumerate() {
            write!(f, "  {index:3}  {:<16}", block.kind())?;
            match *block {
                TzxBlock::StandardData { data, pause_ms } => {
                    write!(f, "{} bytes, pause {pause_ms} ms", data.len)?;
                }
                TzxBlock::TurboData {
                    data,
                    pause_ms,
                    zero,
                    one,
                    ..
                } => write!(
                    f,
                    "{} bytes, 0/1 = {zero}/{one} T, pause {pause_ms} ms",
                    data.len
                )?,
                TzxBlock::PureData { data, pause_ms, .. } => {
                    write!(f, "{} bytes, pause {pause_ms} ms", data.len)?;
                }
                TzxBlock::PureTone { length, pulses } => {
                    write!(f, "{pulses} pulses of {length} T")?;
                }
                TzxBlock::PulseSequence { pulses } => write!(f, "{} pulses", pulses.len / 2)?,
                TzxBlock::DirectRecording { each, samples, .. } => {
                    write!(f, "{} samples at {each} T", samples.len)?;
                }
                TzxBlock::Pause { ms: 0 } => {}
                TzxBlock::Pause { ms } => write!(f, "{ms} ms")?,
                TzxBlock::GroupStart { name } => write!(f, "{}", self.text(name))?,
                TzxBlock::Jump { offset } => write!(f, "{offset:+}")?,
                TzxBlock::LoopStart { count } => write!(f, "{count} times")?,
                TzxBlock::Text { text, .. } => write!(f, "{}", self.text(text).trim_end())?,
                TzxBlock::Custom { id, body } => {
                    write!(f, "{}, {} bytes", self.text(id).trim_end(), body.len)?;
                }
                TzxBlock::Skipped { id, body } => write!(f, "id {id:#04X}, {} bytes", body.len)?,
                _ => {}
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

/// What an archive info entry is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArchiveId {
    Title,
    Publisher,
    Author,
    Year,
    Language,
    Kind,
    Price,
    Protection,
    Origin,
    Comment,
    Other(u8),
}

impl ArchiveId {
    const fn of(byte: u8) -> ArchiveId {
        match byte {
            0x00 => ArchiveId::Title,
            0x01 => ArchiveId::Publisher,
            0x02 => ArchiveId::Author,
            0x03 => ArchiveId::Year,
            0x04 => ArchiveId::Language,
            0x05 => ArchiveId::Kind,
            0x06 => ArchiveId::Price,
            0x07 => ArchiveId::Protection,
            0x08 => ArchiveId::Origin,
            0xFF => ArchiveId::Comment,
            other => ArchiveId::Other(other),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            ArchiveId::Title => "title",
            ArchiveId::Publisher => "publisher",
            ArchiveId::Author => "author",
            ArchiveId::Year => "year",
            ArchiveId::Language => "language",
            ArchiveId::Kind => "type",
            ArchiveId::Price => "price",
            ArchiveId::Protection => "protection",
            ArchiveId::Origin => "origin",
            ArchiveId::Comment => "comment",
            ArchiveId::Other(_) => "info",
        }
    }
}

/// What is wrong with a file that is not a TZX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TzxError {
    /// No `ZXTape!` signature, which is the one thing every TZX has.
    NotATzx,
    /// A block wants more bytes than the file has left. The id is reported
    /// because the block that ran off the end is usually the one before the
    /// download was cut short, and knowing which it was is the difference
    /// between a truncated file and a misparsed one.
    Truncated {
        at: usize,
        id: u8,
        want: usize,
        have: usize,
    },
}

impl fmt::Display for TzxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TzxError::NotATzx => write!(f, "not a TZX file: no ZXTape! signature"),
            TzxError::Truncated { at, id, want, have } => write!(
                f,
                "truncated TZX: the {id:#04X} block at {at} wants {want} bytes and {have} are left"
            ),
        }
    }
}

impl std::error::Error for TzxError {}

/// Reads the file forwards, one field at a time.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    /// The next block, whatever it is. The id has already been consumed by the
    /// time anything can go wrong, so every error can name it.
    fn block(&mut self) -> Result<TzxBlock, TzxError> {
        let id = self.byte(0)?;
        Ok(match id {
            0x10 => {
                let pause_ms = self.word(id)?;
                let len = usize::from(self.word(id)?);
                TzxBlock::StandardData {
                    pause_ms,
                    data: self.take(id, len)?,
                }
            }
            0x11 => {
                let pilot = self.word(id)?;
                let sync_first = self.word(id)?;
                let sync_second = self.word(id)?;
                let zero = self.word(id)?;
                let one = self.word(id)?;
                let pilot_pulses = self.word(id)?;
                let last_bits = self.bits(id)?;
                let pause_ms = self.word(id)?;
                let len = self.triple(id)?;
                TzxBlock::TurboData {
                    pilot,
                    sync_first,
                    sync_second,
                    zero,
                    one,
                    pilot_pulses,
                    last_bits,
                    pause_ms,
                    data: self.take(id, len)?,
                }
            }
            0x12 => TzxBlock::PureTone {
                length: self.word(id)?,
                pulses: self.word(id)?,
            },
            0x13 => {
                let count = usize::from(self.byte(id)?);
                TzxBlock::PulseSequence {
                    pulses: self.take(id, 2 * count)?,
                }
            }
            0x14 => {
                let zero = self.word(id)?;
                let one = self.word(id)?;
                let last_bits = self.bits(id)?;
                let pause_ms = self.word(id)?;
                let len = self.triple(id)?;
                TzxBlock::PureData {
                    zero,
                    one,
                    last_bits,
                    pause_ms,
                    data: self.take(id, len)?,
                }
            }
            0x15 => {
                let each = self.word(id)?;
                let pause_ms = self.word(id)?;
                let last_bits = self.bits(id)?;
                let len = self.triple(id)?;
                TzxBlock::DirectRecording {
                    each,
                    pause_ms,
                    last_bits,
                    samples: self.take(id, len)?,
                }
            }
            0x20 => TzxBlock::Pause { ms: self.word(id)? },
            0x21 => {
                let len = usize::from(self.byte(id)?);
                TzxBlock::GroupStart {
                    name: self.take(id, len)?,
                }
            }
            0x22 => TzxBlock::GroupEnd,
            0x23 => TzxBlock::Jump {
                offset: self.word(id)? as i16,
            },
            0x24 => TzxBlock::LoopStart {
                count: self.word(id)?,
            },
            0x25 => TzxBlock::LoopEnd,
            0x2A => {
                self.dword(id)?;
                TzxBlock::StopIf48k
            }
            0x2B => {
                self.dword(id)?;
                TzxBlock::SetLevel {
                    level: self.byte(id)? != 0,
                }
            }
            0x30 => {
                let len = usize::from(self.byte(id)?);
                TzxBlock::Text {
                    seconds: None,
                    text: self.take(id, len)?,
                }
            }
            0x31 => {
                let seconds = self.byte(id)?;
                let len = usize::from(self.byte(id)?);
                TzxBlock::Text {
                    seconds: Some(seconds),
                    text: self.take(id, len)?,
                }
            }
            0x32 => {
                let len = usize::from(self.word(id)?);
                TzxBlock::ArchiveInfo {
                    body: self.take(id, len)?,
                }
            }
            0x33 => {
                let count = usize::from(self.byte(id)?);
                TzxBlock::Hardware {
                    body: self.take(id, 3 * count)?,
                }
            }
            0x35 => {
                let id_span = self.take(id, 10)?;
                let len = self.dword(id)?;
                TzxBlock::Custom {
                    id: id_span,
                    body: self.take(id, len)?,
                }
            }
            0x5A => {
                self.take(id, 9)?;
                TzxBlock::Glue
            }
            // 0x26 call sequence and 0x28 select block name other blocks to
            // play, which is a decision for a stack or for a person and not for
            // a player; their payloads are counted like anything else's.
            0x26 => {
                let count = usize::from(self.word(id)?);
                TzxBlock::Skipped {
                    id,
                    body: self.take(id, 2 * count)?,
                }
            }
            0x27 => TzxBlock::Skipped {
                id,
                body: Span::EMPTY,
            },
            0x28 => {
                let len = usize::from(self.word(id)?);
                TzxBlock::Skipped {
                    id,
                    body: self.take(id, len)?,
                }
            }
            // The extension rule, which is what makes an unknown block a seek
            // rather than the end of the file: the four bytes after the id are
            // the length of the rest of it. The deprecated C64 blocks (0x16,
            // 0x17), the CSW recording (0x18) and the generalized data block
            // (0x19) are all written that way, and are all waveform this player
            // does not produce.
            _ => {
                let len = self.dword(id)?;
                TzxBlock::Skipped {
                    id,
                    body: self.take(id, len)?,
                }
            }
        })
    }

    fn byte(&mut self, id: u8) -> Result<u8, TzxError> {
        let span = self.take(id, 1)?;
        Ok(self.bytes[span.at])
    }

    fn word(&mut self, id: u8) -> Result<u16, TzxError> {
        let span = self.take(id, 2)?;
        Ok(u16::from_le_bytes([
            self.bytes[span.at],
            self.bytes[span.at + 1],
        ]))
    }

    fn triple(&mut self, id: u8) -> Result<usize, TzxError> {
        let span = self.take(id, 3)?;
        let at = span.at;
        Ok(usize::from(self.bytes[at])
            | usize::from(self.bytes[at + 1]) << 8
            | usize::from(self.bytes[at + 2]) << 16)
    }

    fn dword(&mut self, id: u8) -> Result<usize, TzxError> {
        let span = self.take(id, 4)?;
        let at = span.at;
        let mut len = 0usize;
        for shift in 0..4 {
            len |= usize::from(self.bytes[at + shift]) << (8 * shift);
        }
        Ok(len)
    }

    /// The count of bits in the last byte, which the format allows to be
    /// nonsense and which is clamped rather than refused: a file that says
    /// nine, or zero, is one whose last byte is a whole one, and refusing to
    /// open the tape over it would be refusing the ninety-nine blocks that
    /// were fine.
    fn bits(&mut self, id: u8) -> Result<u8, TzxError> {
        let bits = self.byte(id)?;
        Ok(if (1..=8).contains(&bits) { bits } else { 8 })
    }

    fn take(&mut self, id: u8, len: usize) -> Result<Span, TzxError> {
        let have = self.bytes.len() - self.at;
        if len > have {
            return Err(TzxError::Truncated {
                at: self.at,
                id,
                want: len,
                have,
            });
        }
        let span = Span { at: self.at, len };
        self.at += len;
        Ok(span)
    }
}

fn header_bytes() -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.push(VERSION.0);
    bytes.push(VERSION.1);
    bytes
}

/// Builds a TZX file block by block.
///
/// Everything goes through the same parser on the way out, so a builder that
/// wrote a field in the wrong place fails in [`Builder::build`] rather than in
/// whatever was being tested with it.
#[derive(Debug, Clone)]
pub struct Builder {
    bytes: Vec<u8>,
}

impl Builder {
    /// A `0x10` standard speed data block: a TAP block with a pause.
    pub fn standard(self, pause_ms: u16, body: &[u8]) -> Builder {
        let len = u16::try_from(body.len()).expect("a block is at most 64 KB");
        self.raw(0x10, &[&pause_ms.to_le_bytes(), &len.to_le_bytes(), body])
    }

    /// A `0x10` block with a flag byte and a checksum around `data`, which is
    /// what [`crate::tap::Builder::block`] writes.
    pub fn block(self, flag: u8, data: &[u8], pause_ms: u16) -> Builder {
        let mut body = Vec::with_capacity(data.len() + 2);
        body.push(flag);
        body.extend_from_slice(data);
        body.push(crate::tap::checksum(&body));
        self.standard(pause_ms, &body)
    }

    /// A `0x11` turbo speed data block.
    pub fn turbo(self, turbo: &Turbo, body: &[u8]) -> Builder {
        let len = u32::try_from(body.len()).expect("a block is at most 16 MB");
        self.raw(
            0x11,
            &[
                &turbo.pilot.to_le_bytes(),
                &turbo.sync_first.to_le_bytes(),
                &turbo.sync_second.to_le_bytes(),
                &turbo.zero.to_le_bytes(),
                &turbo.one.to_le_bytes(),
                &turbo.pilot_pulses.to_le_bytes(),
                &[turbo.last_bits],
                &turbo.pause_ms.to_le_bytes(),
                &len.to_le_bytes()[..3],
                body,
            ],
        )
    }

    /// A `0x12` pure tone.
    pub fn tone(self, length: u16, pulses: u16) -> Builder {
        self.raw(0x12, &[&length.to_le_bytes(), &pulses.to_le_bytes()])
    }

    /// A `0x13` pulse sequence.
    pub fn pulses(self, lengths: &[u16]) -> Builder {
        let count = u8::try_from(lengths.len()).expect("at most 255 pulses in a sequence");
        let mut body = vec![count];
        for length in lengths {
            body.extend_from_slice(&length.to_le_bytes());
        }
        self.raw(0x13, &[&body])
    }

    /// A `0x14` pure data block: no pilot, no sync, just bits.
    pub fn pure_data(
        self,
        zero: u16,
        one: u16,
        last_bits: u8,
        pause_ms: u16,
        body: &[u8],
    ) -> Builder {
        let len = u32::try_from(body.len()).expect("a block is at most 16 MB");
        self.raw(
            0x14,
            &[
                &zero.to_le_bytes(),
                &one.to_le_bytes(),
                &[last_bits],
                &pause_ms.to_le_bytes(),
                &len.to_le_bytes()[..3],
                body,
            ],
        )
    }

    /// A `0x15` direct recording: one bit a sample, most significant first.
    pub fn direct(self, each: u16, pause_ms: u16, last_bits: u8, samples: &[u8]) -> Builder {
        let len = u32::try_from(samples.len()).expect("a recording is at most 16 MB");
        self.raw(
            0x15,
            &[
                &each.to_le_bytes(),
                &pause_ms.to_le_bytes(),
                &[last_bits],
                &len.to_le_bytes()[..3],
                samples,
            ],
        )
    }

    /// A `0x20` pause, or at zero a request to stop the tape.
    pub fn pause(self, ms: u16) -> Builder {
        self.raw(0x20, &[&ms.to_le_bytes()])
    }

    /// A `0x2A` stop-if-48K.
    pub fn stop_if_48k(self) -> Builder {
        self.raw(0x2A, &[&0u32.to_le_bytes()])
    }

    /// A `0x2B` set signal level.
    pub fn level(self, level: bool) -> Builder {
        self.raw(0x2B, &[&1u32.to_le_bytes(), &[u8::from(level)]])
    }

    /// A `0x23` jump, relative to the jump block itself.
    pub fn jump(self, offset: i16) -> Builder {
        self.raw(0x23, &[&offset.to_le_bytes()])
    }

    /// A `0x24` loop start: the blocks after it play `count` times in total.
    pub fn loop_start(self, count: u16) -> Builder {
        self.raw(0x24, &[&count.to_le_bytes()])
    }

    /// A `0x25` loop end.
    pub fn loop_end(self) -> Builder {
        self.raw(0x25, &[])
    }

    /// A `0x21` group start.
    pub fn group_start(self, name: &str) -> Builder {
        let len = u8::try_from(name.len()).expect("a group name is at most 255 bytes");
        self.raw(0x21, &[&[len], name.as_bytes()])
    }

    /// A `0x22` group end.
    pub fn group_end(self) -> Builder {
        self.raw(0x22, &[])
    }

    /// A `0x30` text description.
    pub fn text(self, text: &str) -> Builder {
        let len = u8::try_from(text.len()).expect("a description is at most 255 bytes");
        self.raw(0x30, &[&[len], text.as_bytes()])
    }

    /// A `0x32` archive info block carrying `entries` of `(id, text)`.
    pub fn archive_info(self, entries: &[(u8, &str)]) -> Builder {
        let mut body = vec![u8::try_from(entries.len()).expect("at most 255 entries")];
        for (id, text) in entries {
            let len = u8::try_from(text.len()).expect("an entry is at most 255 bytes");
            body.push(*id);
            body.push(len);
            body.extend_from_slice(text.as_bytes());
        }
        let len = u16::try_from(body.len()).expect("archive info is at most 64 KB");
        self.raw(0x32, &[&len.to_le_bytes(), &body])
    }

    /// A block of an id this player does not know, written the way the format
    /// says an unknown block has to be: a four-byte length and then the rest.
    pub fn unknown(self, id: u8, body: &[u8]) -> Builder {
        let len = u32::try_from(body.len()).expect("a block is at most 4 GB");
        self.raw(id, &[&len.to_le_bytes(), body])
    }

    fn raw(mut self, id: u8, parts: &[&[u8]]) -> Builder {
        self.bytes.push(id);
        for part in parts {
            self.bytes.extend_from_slice(part);
        }
        self
    }

    /// The bytes, without parsing them. What a test that wants a malformed
    /// file starts from.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn build(self) -> Tzx {
        Tzx::parse(&self.bytes).expect("the builder writes well-formed blocks")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tap::DATA_FLAG;

    #[test]
    fn a_file_round_trips_through_parsing() {
        let tzx = Tzx::builder()
            .text("a tape")
            .block(DATA_FLAG, &[1, 2, 3], 500)
            .build();
        let parsed = Tzx::parse(tzx.bytes()).expect("well formed");

        assert_eq!(parsed, tzx);
        assert_eq!(parsed.version(), VERSION);
        assert_eq!(parsed.hash(), tzx.hash());
        assert_eq!(parsed.len(), 2);
        // The ten-byte header, then eight bytes of text block, then the id,
        // the pause and the length: the body starts at 23.
        let data = Span { at: 23, len: 5 };
        assert_eq!(
            parsed.block(1),
            Some(TzxBlock::StandardData {
                pause_ms: 500,
                data,
            })
        );
        // Flag, three bytes, checksum.
        assert_eq!(parsed.span(data), &[0xFF, 1, 2, 3, 0xFF ^ 1 ^ 2 ^ 3]);
    }

    #[test]
    fn a_file_without_the_signature_is_not_a_tzx() {
        assert_eq!(Tzx::parse(b"not a tape"), Err(TzxError::NotATzx));
        assert_eq!(Tzx::parse(&[]), Err(TzxError::NotATzx));
        // The signature and nothing else: no version bytes.
        assert_eq!(Tzx::parse(MAGIC), Err(TzxError::NotATzx));
        assert!(
            Tzx::parse(&header_bytes())
                .expect("a header alone")
                .is_empty()
        );
    }

    #[test]
    fn a_block_that_runs_off_the_end_is_refused() {
        let mut bytes = Tzx::builder().standard(0, &[1, 2, 3]).into_bytes();
        bytes.truncate(bytes.len() - 1);

        assert_eq!(
            Tzx::parse(&bytes),
            Err(TzxError::Truncated {
                at: 15,
                id: 0x10,
                want: 3,
                have: 2
            })
        );
    }

    #[test]
    fn a_turbo_block_keeps_every_length_it_was_given() {
        let turbo = Turbo {
            pilot: 1234,
            pilot_pulses: 7,
            sync_first: 11,
            sync_second: 22,
            zero: 300,
            one: 600,
            last_bits: 3,
            pause_ms: 42,
        };
        let tzx = Tzx::builder().turbo(&turbo, &[0xAA, 0xBB]).build();

        let TzxBlock::TurboData {
            pilot,
            pilot_pulses,
            sync_first,
            sync_second,
            zero,
            one,
            last_bits,
            pause_ms,
            data,
        } = tzx.block(0).expect("one block")
        else {
            panic!("a turbo block");
        };
        assert_eq!(
            (pilot, pilot_pulses, sync_first, sync_second),
            (1234, 7, 11, 22)
        );
        assert_eq!((zero, one, last_bits, pause_ms), (300, 600, 3, 42));
        assert_eq!(tzx.span(data), &[0xAA, 0xBB]);
    }

    #[test]
    fn the_blocks_with_no_waveform_are_read_for_what_they_say() {
        let tzx = Tzx::builder()
            .group_start("loader")
            .pure_data(100, 200, 4, 0, &[0xF0])
            .pulses(&[10, 20, 30])
            .tone(2168, 100)
            .direct(79, 0, 8, &[0b1010_1010])
            .group_end()
            .jump(-3)
            .loop_start(5)
            .loop_end()
            .pause(0)
            .stop_if_48k()
            .level(true)
            .build();

        let kinds: Vec<_> = tzx.blocks().iter().map(|b| b.kind()).collect();
        assert_eq!(
            kinds,
            [
                "group start",
                "pure data",
                "pulse sequence",
                "pure tone",
                "direct recording",
                "group end",
                "jump",
                "loop start",
                "loop end",
                "stop the tape",
                "stop if 48K",
                "set level",
            ]
        );
        assert_eq!(tzx.block(6), Some(TzxBlock::Jump { offset: -3 }));
        assert_eq!(tzx.block(7), Some(TzxBlock::LoopStart { count: 5 }));
        assert_eq!(tzx.block(11), Some(TzxBlock::SetLevel { level: true }));
        let TzxBlock::PulseSequence { pulses } = tzx.block(2).expect("a sequence") else {
            panic!("a pulse sequence");
        };
        assert_eq!(tzx.span(pulses), &[10, 0, 20, 0, 30, 0]);
    }

    #[test]
    fn an_unknown_block_is_skipped_by_its_length_and_the_rest_of_the_file_survives() {
        // The one property that matters: a file from a later version of the
        // format still loads, because the block nobody here has heard of says
        // how long it is.
        let tzx = Tzx::builder()
            .unknown(0x19, &[0; 40])
            .block(DATA_FLAG, &[7], 0)
            .unknown(0x99, &[])
            .build();

        assert_eq!(tzx.len(), 3);
        assert_eq!(
            tzx.block(0),
            Some(TzxBlock::Skipped {
                id: 0x19,
                body: Span { at: 15, len: 40 }
            })
        );
        assert!(matches!(tzx.block(1), Some(TzxBlock::StandardData { .. })));
        let TzxBlock::StandardData { data, .. } = tzx.block(1).expect("the data block") else {
            panic!("a standard block");
        };
        assert_eq!(tzx.span(data), &[DATA_FLAG, 7, DATA_FLAG ^ 7]);
    }

    #[test]
    fn text_and_archive_info_are_read_and_shown() {
        let tzx = Tzx::builder()
            .archive_info(&[(0x00, "Manic Miner"), (0x01, "Bug-Byte"), (0x03, "1983")])
            .text("Press any key")
            .block(DATA_FLAG, &[0], 0)
            .build();

        assert_eq!(
            tzx.archive_info(),
            vec![
                (ArchiveId::Title, "Manic Miner".to_string()),
                (ArchiveId::Publisher, "Bug-Byte".to_string()),
                (ArchiveId::Year, "1983".to_string()),
            ]
        );
        assert_eq!(tzx.descriptions(), ["Press any key"]);

        let shown = tzx.to_string();
        assert!(shown.contains("title: Manic Miner"), "{shown}");
        assert!(shown.contains("publisher: Bug-Byte"), "{shown}");
        assert!(shown.contains("Press any key"), "{shown}");
        assert!(shown.contains("standard data"), "{shown}");
    }

    #[test]
    fn a_truncated_archive_entry_ends_the_list_rather_than_the_load() {
        // The count says two and the second entry's length runs past the end of
        // the block. The tape is already mounted by the time anybody reads this.
        let body = [2u8, 0x00, 4, b'a', b'b', b'c', b'd', 0x01, 9, b'x'];
        let len = u16::try_from(body.len()).expect("short");
        let mut bytes = header_bytes();
        bytes.push(0x32);
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&body);
        let tzx = Tzx::parse(&bytes).expect("well formed as a block");

        assert_eq!(
            tzx.archive_info(),
            vec![(ArchiveId::Title, "abcd".to_string())]
        );
    }

    /// The blocks as a waveform, which is the half of the format that matters
    /// to the machine. [`TINY`] timings, so that every number below can be
    /// checked on paper.
    mod plays {
        use super::*;
        use crate::pulse::{Player, Pulse};

        /// The ROM's shape at numbers small enough to add up in one's head:
        /// two pilot pulses in front of a data block, and a millisecond of
        /// seven T-states.
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

        fn pulses(tzx: &Tzx) -> Vec<Pulse> {
            let mut player = Player::new();
            let mut out = Vec::new();
            while let Some(pulse) = player.next_pulse(tzx, &TINY) {
                out.push(pulse);
            }
            out
        }

        fn ticks(tzx: &Tzx) -> Vec<u32> {
            pulses(tzx).iter().map(|p| p.ticks).collect()
        }

        #[test]
        fn a_standard_block_is_the_rom_shape_with_the_pause_the_file_asked_for() {
            let tzx = Tzx::builder().standard(3, &[DATA_FLAG]).build();

            assert_eq!(
                ticks(&tzx),
                vec![
                    100, 100, // the data pilot: 0xFF is not a header
                    10, 20, // sync
                    2, 2, 2, 2, 2, 2, 2, 2, // eight one bits
                    2, 2, 2, 2, 2, 2, 2, 2,  //
                    7,  // a millisecond, which is the edge that ends the last bit
                    14, // and the other two milliseconds of the pause
                ]
            );
        }

        #[test]
        fn a_turbo_block_plays_its_own_lengths_and_stops_where_it_says_to() {
            // Three bits of the last byte, which is what a loader that packs a
            // checksum into a partial byte writes.
            let turbo = Turbo {
                pilot: 50,
                pilot_pulses: 2,
                sync_first: 6,
                sync_second: 7,
                zero: 3,
                one: 8,
                last_bits: 3,
                pause_ms: 0,
            };
            let tzx = Tzx::builder().turbo(&turbo, &[0x0F, 0b1010_0000]).build();

            assert_eq!(
                ticks(&tzx),
                vec![
                    50, 50, // the pilot, as many pulses as it asked for
                    6, 7, // its own sync pair
                    3, 3, 3, 3, 3, 3, 3, 3, // 0x0F: four zeroes
                    8, 8, 8, 8, 8, 8, 8, 8, // and four ones
                    8, 8, 3, 3, 8,
                    8, // three bits of 0b101, and no more
                       // no pause, so no edge to end the last bit either:
                       // the block after it is what supplies one
                ]
            );
        }

        #[test]
        fn a_pure_data_block_has_no_pilot_and_no_sync() {
            let tzx = Tzx::builder().pure_data(3, 8, 8, 0, &[0x80]).build();
            assert_eq!(
                ticks(&tzx),
                vec![8, 8, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3]
            );
        }

        #[test]
        fn a_tone_is_a_run_of_one_length_and_a_sequence_is_a_list_of_them() {
            let tzx = Tzx::builder().tone(40, 3).pulses(&[11, 22, 33]).build();
            let pulses = pulses(&tzx);

            assert_eq!(
                pulses.iter().map(|p| p.ticks).collect::<Vec<_>>(),
                [40, 40, 40, 11, 22, 33]
            );
            // Both are ordinary edges, so the line alternates across the join.
            for pair in pulses.windows(2) {
                assert_ne!(pair[0].level, pair[1].level);
            }
        }

        #[test]
        fn a_direct_recording_says_what_the_level_was_rather_than_that_it_changed() {
            // 0b1100_0011: two samples high, four low, two high — and then a
            // second byte of which only the top two bits count.
            let tzx = Tzx::builder()
                .direct(10, 0, 2, &[0b1100_0011, 0b1100_0000])
                .build();

            assert_eq!(
                pulses(&tzx),
                vec![
                    // The run of equal samples is one pulse, not one an edge:
                    // a recording sampled at 10 T-states would otherwise put
                    // an event on the emulation thread for every one of them.
                    Pulse {
                        ticks: 20,
                        level: true
                    },
                    Pulse {
                        ticks: 40,
                        level: false
                    },
                    // The last two samples of the first byte and the two the
                    // second byte contributes are all high, and one run.
                    Pulse {
                        ticks: 40,
                        level: true
                    },
                ]
            );
        }

        #[test]
        fn a_pause_block_ends_the_block_before_it() {
            // The pause is where a data block with none of its own gets the
            // edge that ends its last bit, which is the reason a file writes
            // the two next to each other.
            let tzx = Tzx::builder()
                .pure_data(3, 8, 8, 0, &[0x00])
                .pause(3)
                .build();
            let pulses = pulses(&tzx);

            assert_eq!(pulses.len(), 16 + 2);
            assert_eq!(
                pulses[16],
                Pulse {
                    ticks: 7,
                    level: true
                }
            );
            assert_eq!(
                pulses[17],
                Pulse {
                    ticks: 14,
                    level: false
                }
            );
        }

        #[test]
        fn a_pause_of_no_time_stops_the_tape_where_it_is() {
            // Both spellings of it: the pause block at zero, and the block
            // that stops a 48K machine, which this is.
            for tzx in [
                Tzx::builder().tone(40, 1).pause(0).tone(50, 1).build(),
                Tzx::builder().tone(40, 1).stop_if_48k().tone(50, 1).build(),
            ] {
                let mut player = Player::new();
                assert_eq!(player.next_pulse(&tzx, &TINY).map(|p| p.ticks), Some(40));
                assert_eq!(player.next_pulse(&tzx, &TINY), None);
                assert!(player.stopped(), "stopped, and not at the end of the tape");
                assert!(!player.finished());
                assert_eq!(player.block(), 2, "waiting on the block after it");

                // Starting the tape again carries on from there.
                player.resume();
                assert_eq!(player.next_pulse(&tzx, &TINY).map(|p| p.ticks), Some(50));
                assert_eq!(player.next_pulse(&tzx, &TINY), None);
                assert!(player.finished());
            }
        }

        #[test]
        fn a_loop_plays_its_body_the_number_of_times_it_says() {
            let tzx = Tzx::builder()
                .loop_start(3)
                .tone(40, 1)
                .tone(50, 1)
                .loop_end()
                .tone(60, 1)
                .build();

            assert_eq!(ticks(&tzx), [40, 50, 40, 50, 40, 50, 60]);
        }

        #[test]
        fn a_jump_goes_where_it_points_and_a_loop_after_it_still_works() {
            // Forwards over a block, and then backwards over one.
            let tzx = Tzx::builder()
                .jump(2)
                .tone(10, 1) // skipped
                .tone(20, 1)
                .jump(2)
                .tone(30, 1) // skipped
                .tone(40, 1)
                .build();

            assert_eq!(ticks(&tzx), [20, 40]);
        }

        #[test]
        fn a_tape_that_does_nothing_forever_runs_out_rather_than_hanging() {
            // The format's way of saying "loop forever" is a jump to the block
            // itself, and a file that does that with no waveform in the loop
            // is asking the player to spin. It gives up instead.
            let tzx = Tzx::builder().jump(0).build();
            let mut player = Player::new();

            assert_eq!(player.next_pulse(&tzx, &TINY), None);
            assert!(player.finished());
        }

        #[test]
        fn a_jump_off_the_front_of_the_tape_is_the_end_of_it() {
            let tzx = Tzx::builder().tone(10, 1).jump(-5).build();
            assert_eq!(ticks(&tzx), [10]);
        }

        #[test]
        fn the_blocks_that_are_words_rather_than_signal_play_nothing() {
            let tzx = Tzx::builder()
                .archive_info(&[(0x00, "a game")])
                .text("insert side two")
                .group_start("loader")
                .unknown(0x19, &[0; 8])
                .tone(40, 1)
                .group_end()
                .build();

            assert_eq!(ticks(&tzx), [40]);
        }

        #[test]
        fn setting_the_level_moves_the_line_without_an_edge_of_its_own() {
            // A tone flips from wherever the line is, so a block that puts it
            // high first makes the tone's first pulse low.
            let high = Tzx::builder().level(true).tone(40, 2).build();
            let low = Tzx::builder().level(false).tone(40, 2).build();

            assert_eq!(
                pulses(&high).iter().map(|p| p.level).collect::<Vec<_>>(),
                [false, true]
            );
            assert_eq!(
                pulses(&low).iter().map(|p| p.level).collect::<Vec<_>>(),
                [true, false]
            );
        }
    }

    #[test]
    fn a_count_of_bits_the_format_does_not_allow_is_a_whole_byte() {
        let tzx = Tzx::builder()
            .pure_data(1, 2, 0, 0, &[0xFF])
            .pure_data(1, 2, 9, 0, &[0xFF])
            .build();

        for index in 0..2 {
            let TzxBlock::PureData { last_bits, .. } = tzx.block(index).expect("a block") else {
                panic!("pure data");
            };
            assert_eq!(last_bits, 8);
        }
    }
}
