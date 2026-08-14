//! The TAP container: length-prefixed blocks of exactly what the tape carried.
//!
//! A TAP file is the barest possible recording of a tape: for each block, a
//! little-endian 16-bit length and then that many bytes, which are the bytes
//! the ROM's loader would have handed back — a flag byte, the data, and an XOR
//! checksum over both. There is no timing information in the file at all,
//! which is what makes it small and what makes it lossy: a block that was
//! saved by a custom loader with its own pulse lengths cannot be written down
//! here, and that is what ticket 0017's TZX is for.
//!
//! # Parsing is validation, not decoding
//!
//! [`Tap::parse`] keeps the file's bytes as they came and records where each
//! block body starts. Nothing is copied per block and nothing is interpreted:
//! a block is nineteen bytes of header or forty kilobytes of screen depending
//! only on what wrote it, and deciding which is [`Block::header`]'s business
//! and usually nobody's.
//!
//! The index exists because the player has to be able to ask for block *n*
//! without walking the file. It lives inside an emulated machine that gets
//! cloned into a checkpoint ring, so walking it once at mount time and never
//! again is the difference between an index lookup and a scan on every block
//! boundary.
//!
//! # What is refused
//!
//! A length that runs off the end of the file, and a zero-length block. The
//! first is a truncated download, which is common; the second cannot be
//! anything, since even the emptiest real block has a flag byte. Everything
//! else is accepted, *including a bad checksum*: a tape with a corrupt block
//! on it is a tape, the ROM's response to one is the thing under test, and a
//! parser that refused to open it would make that untestable.

/// The flag byte in front of a header block. The ROM treats any flag under
/// `0x80` as a header, and nothing has ever written a different one.
pub const HEADER_FLAG: u8 = 0x00;

/// The flag byte in front of a data block.
pub const DATA_FLAG: u8 = 0xFF;

/// Bytes in a header block's body: the flag, seventeen of header, the
/// checksum.
pub const HEADER_LEN: usize = 19;

/// The XOR of a block's bytes, which is what its last byte has to be.
///
/// The whole of the ROM's error detection: every byte from the flag onwards is
/// XORed into a running total, and the byte after the data has to bring it to
/// zero. It catches any single corrupt byte and half of everything else, which
/// in 1982 was the difference between "R Tape loading error" and a program
/// that crashes ten minutes later.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |sum, byte| sum ^ byte)
}

/// A TAP file: its bytes, and where the blocks in it are.
#[derive(Clone, PartialEq, Eq)]
pub struct Tap {
    bytes: Vec<u8>,
    /// The offset and length of each block *body* — what is between the length
    /// headers, which is what the tape actually carried.
    blocks: Vec<(usize, usize)>,
}

impl Tap {
    /// Read a TAP file, checking that its blocks frame the whole of it.
    pub fn parse(bytes: &[u8]) -> Result<Tap, TapError> {
        let mut blocks = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            if at + 2 > bytes.len() {
                return Err(TapError::Truncated {
                    at,
                    want: 2,
                    have: bytes.len() - at,
                });
            }
            let len = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
            let body = at + 2;
            if len == 0 {
                return Err(TapError::EmptyBlock { at });
            }
            if body + len > bytes.len() {
                return Err(TapError::Truncated {
                    at,
                    want: len,
                    have: bytes.len() - body,
                });
            }
            blocks.push((body, len));
            at = body + len;
        }
        Ok(Tap {
            bytes: bytes.to_vec(),
            blocks,
        })
    }

    /// A tape with nothing on it, which is what a machine with no tape mounted
    /// plays.
    pub fn empty() -> Tap {
        Tap {
            bytes: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// Assemble a tape a block at a time.
    pub fn builder() -> Builder {
        Builder { tap: Tap::empty() }
    }

    /// Blocks on the tape.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Block `index`, or `None` past the end of the tape.
    pub fn block(&self, index: usize) -> Option<Block<'_>> {
        let &(at, len) = self.blocks.get(index)?;
        Some(Block {
            body: &self.bytes[at..at + len],
        })
    }

    /// Every block in order.
    pub fn blocks(&self) -> impl Iterator<Item = Block<'_>> {
        (0..self.len()).map(|i| self.block(i).expect("in range"))
    }

    /// The file, as it would be written out.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// FNV-1a over the file.
    ///
    /// ADR-0017 wants a recorded session to name its tape by content and not
    /// only by path, so that a tape edited between the recording and the replay
    /// is a refusal rather than a divergence with no visible cause. This is
    /// that name. It is not a cryptographic hash and is not being asked to be
    /// one: what it has to catch is a file that changed, not a file somebody
    /// arranged to collide.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in &self.bytes {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

impl std::fmt::Debug for Tap {
    /// The blocks and their sizes rather than forty kilobytes of hex.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tap")
            .field("blocks", &self.len())
            .field("bytes", &self.bytes.len())
            .field("hash", &format_args!("{:#018x}", self.hash()))
            .finish()
    }
}

/// One block, borrowed out of the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block<'a> {
    body: &'a [u8],
}

impl<'a> Block<'a> {
    /// The whole of what the tape carried: flag, data and checksum.
    pub fn body(self) -> &'a [u8] {
        self.body
    }

    /// The first byte, which says what kind of block this is.
    pub fn flag(self) -> u8 {
        self.body[0]
    }

    /// True for a block the ROM would treat as a header, which is the long
    /// pilot tone and the nineteen-byte body.
    pub fn is_header(self) -> bool {
        self.flag() < 0x80
    }

    /// The data between the flag and the checksum.
    pub fn data(self) -> &'a [u8] {
        if self.body.len() < 2 {
            &[]
        } else {
            &self.body[1..self.body.len() - 1]
        }
    }

    /// The last byte, which the XOR of everything before it has to equal.
    /// `None` for a block too short to have one.
    pub fn checksum(self) -> Option<u8> {
        if self.body.len() < 2 {
            None
        } else {
            self.body.last().copied()
        }
    }

    /// Whether the checksum agrees with the data, which is exactly the test
    /// the ROM makes before reporting a loading error.
    pub fn checksum_ok(self) -> bool {
        self.body.len() >= 2 && checksum(self.body) == 0
    }

    /// What a header block says about the file behind it, or `None` if this is
    /// not a well-formed header.
    pub fn header(self) -> Option<Header> {
        if !self.is_header() || self.body.len() != HEADER_LEN {
            return None;
        }
        let data = self.data();
        let word = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let mut name = [0; 10];
        name.copy_from_slice(&data[1..11]);
        Some(Header {
            kind: FileKind::of(data[0]),
            name,
            length: word(11),
            param1: word(13),
            param2: word(15),
        })
    }
}

/// The seventeen bytes a header block carries about the file after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub kind: FileKind,
    /// Ten characters, space-padded, in the Spectrum's character set.
    pub name: [u8; 10],
    /// Bytes in the data block that follows.
    pub length: u16,
    /// Autostart line for a program, start address for code.
    pub param1: u16,
    /// Where the variables start in a program; unused for code.
    pub param2: u16,
}

impl Header {
    /// The name with its padding trimmed. Bytes outside ASCII are left as `?`
    /// rather than guessed at: the Spectrum's set diverges above `0x7F`, and a
    /// header is not the place to have an opinion about it.
    pub fn name(self) -> String {
        self.name
            .iter()
            .map(|&b| {
                if (32..127).contains(&b) {
                    b as char
                } else {
                    '?'
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

/// What a header says it is in front of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Program,
    NumberArray,
    CharArray,
    Code,
    /// Anything else, which the ROM's `SAVE` cannot produce and a custom
    /// loader can.
    Other(u8),
}

impl FileKind {
    fn of(byte: u8) -> FileKind {
        match byte {
            0 => FileKind::Program,
            1 => FileKind::NumberArray,
            2 => FileKind::CharArray,
            3 => FileKind::Code,
            other => FileKind::Other(other),
        }
    }
}

/// Builds a TAP file block by block.
#[derive(Debug, Clone)]
pub struct Builder {
    tap: Tap,
}

impl Builder {
    /// Add a block with `flag` in front of `data` and the checksum after it,
    /// which is what the ROM's `SAVE` writes.
    pub fn block(self, flag: u8, data: &[u8]) -> Builder {
        let mut body = Vec::with_capacity(data.len() + 2);
        body.push(flag);
        body.extend_from_slice(data);
        body.push(checksum(&body));
        self.body(&body)
    }

    /// Add a block body verbatim — flag, data and checksum as given, whether
    /// or not they agree. What a recorded tape produces, and how a test builds
    /// a corrupt block.
    pub fn body(mut self, body: &[u8]) -> Builder {
        let at = self.tap.bytes.len() + 2;
        let len = u16::try_from(body.len()).expect("a block is at most 64 KB");
        self.tap.bytes.extend_from_slice(&len.to_le_bytes());
        self.tap.bytes.extend_from_slice(body);
        self.tap.blocks.push((at, body.len()));
        self
    }

    /// A header block for `data` about to be saved at `start`, and then the
    /// data block itself: the pair the ROM's `SAVE "name" CODE` writes.
    pub fn code(self, name: &str, start: u16, data: &[u8]) -> Builder {
        let mut header = [0u8; HEADER_LEN - 2];
        header[0] = 3; // FileKind::Code
        header[1..11].fill(b' ');
        for (at, byte) in name.bytes().take(10).enumerate() {
            header[1 + at] = byte;
        }
        let length = u16::try_from(data.len()).expect("a code block is at most 64 KB");
        header[11..13].copy_from_slice(&length.to_le_bytes());
        header[13..15].copy_from_slice(&start.to_le_bytes());
        header[15..17].copy_from_slice(&32768u16.to_le_bytes());
        self.block(HEADER_FLAG, &header).block(DATA_FLAG, data)
    }

    pub fn build(self) -> Tap {
        self.tap
    }
}

/// What is wrong with a file that is not a TAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapError {
    /// A block claims more bytes than the file has left.
    Truncated { at: usize, want: usize, have: usize },
    /// A block of no bytes at all, which cannot even carry a flag.
    EmptyBlock { at: usize },
}

impl std::fmt::Display for TapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TapError::Truncated { at, want, have } => write!(
                f,
                "truncated TAP: the block at {at} wants {want} bytes and {have} are left"
            ),
            TapError::EmptyBlock { at } => write!(f, "empty block at {at}"),
        }
    }
}

impl std::error::Error for TapError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_a_flag_its_data_and_the_xor_of_both() {
        let tap = Tap::builder().block(DATA_FLAG, &[0x01, 0x02, 0x03]).build();
        let block = tap.block(0).expect("one block");

        assert_eq!(block.flag(), DATA_FLAG);
        assert_eq!(block.data(), &[0x01, 0x02, 0x03]);
        assert_eq!(block.checksum(), Some(0xFF ^ 0x01 ^ 0x02 ^ 0x03));
        assert!(block.checksum_ok());
        assert!(!block.is_header());
        // Length header, flag, three bytes, checksum.
        assert_eq!(tap.bytes().len(), 2 + 5);
    }

    #[test]
    fn a_file_round_trips_through_parsing() {
        let tap = Tap::builder()
            .block(HEADER_FLAG, &[0; 17])
            .block(DATA_FLAG, &[0xAA; 100])
            .build();
        let parsed = Tap::parse(tap.bytes()).expect("well formed");

        assert_eq!(parsed, tap);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.hash(), tap.hash());
        assert_eq!(parsed.block(1).expect("two blocks").data().len(), 100);
        assert!(parsed.block(2).is_none());
    }

    #[test]
    fn a_length_that_runs_off_the_end_is_refused() {
        // Two bytes of block promised, one delivered.
        assert_eq!(
            Tap::parse(&[0x02, 0x00, 0xFF]),
            Err(TapError::Truncated {
                at: 0,
                want: 2,
                have: 1
            })
        );
        // A length header cut in half.
        assert_eq!(
            Tap::parse(&[0x01, 0x00, 0xFF, 0x05]),
            Err(TapError::Truncated {
                at: 3,
                want: 2,
                have: 1
            })
        );
        assert_eq!(
            Tap::parse(&[0x00, 0x00]),
            Err(TapError::EmptyBlock { at: 0 })
        );
    }

    #[test]
    fn an_empty_file_is_a_tape_with_nothing_on_it() {
        let tap = Tap::parse(&[]).expect("an empty file is well formed");
        assert!(tap.is_empty());
        assert!(tap.block(0).is_none());
    }

    #[test]
    fn a_corrupt_block_parses_and_says_so() {
        // The checksum is what the ROM checks, so a tape carrying a bad one has
        // to be openable or the ROM's response to it cannot be tested.
        let mut body = vec![DATA_FLAG, 0x01, 0x02];
        body.push(checksum(&body) ^ 0x01);
        let tap = Tap::builder().body(&body).build();

        assert!(!tap.block(0).expect("one block").checksum_ok());
        assert_eq!(Tap::parse(tap.bytes()), Ok(tap));
    }

    #[test]
    fn a_header_block_says_what_follows_it() {
        let tap = Tap::builder().code("screen", 16384, &[0; 6912]).build();
        let header = tap
            .block(0)
            .expect("a header")
            .header()
            .expect("a well-formed one");

        assert_eq!(header.kind, FileKind::Code);
        assert_eq!(header.name(), "screen");
        assert_eq!(header.length, 6912);
        assert_eq!(header.param1, 16384);
        assert_eq!(tap.block(1).expect("a data block").data().len(), 6912);
        assert!(tap.blocks().all(|b| b.checksum_ok()));
    }

    #[test]
    fn a_data_block_has_no_header_to_read() {
        let tap = Tap::builder()
            .block(DATA_FLAG, &[0; HEADER_LEN - 2])
            .build();
        assert_eq!(tap.block(0).expect("one block").header(), None);
    }
}
