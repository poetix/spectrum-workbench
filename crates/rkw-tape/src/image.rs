//! A tape, whichever file it came out of.
//!
//! A deck holds one of these rather than a [`Tap`] or a [`Tzx`], for the
//! obvious reason that it does not care: what it plays is pulses, and both
//! formats produce them through the same [`Plan`]. What it does care about is
//! that mounting a tape and cloning a machine cost a pointer, which is why the
//! images are behind [`Arc`] here rather than at the far end in
//! `rkw_spectrum`: a checkpoint ring (ADR-0017) clones the machine, and the
//! tape in it is up to a megabyte.
//!
//! ```
//! use rkw_tape::{Image, Plan, Tap, Timing, Tzx};
//!
//! let timing = Timing::rom(3_500_000);
//! let tap = Image::from(Tap::builder().block(0xFF, &[1, 2, 3]).build());
//! let tzx = Image::from(Tzx::builder().block(0xFF, &[1, 2, 3], 1000).build());
//!
//! // The same three bytes, at the same timings, off two different files.
//! assert_eq!(tap.len(), tzx.len());
//! assert_eq!(tap.data_block(0), tzx.data_block(0));
//!
//! let (Some(Plan::Data(a)), Some(Plan::Data(b))) =
//!     (tap.plan(0, &timing), tzx.plan(0, &timing))
//! else {
//!     panic!("both are data blocks")
//! };
//! assert_eq!((a.pilot, a.zero, a.one, a.bytes), (b.pilot, b.zero, b.one, b.bytes));
//! // A second of silence after each, which the TAP assumes and the TZX says.
//! assert_eq!(a.pause, 3_500_000);
//! assert_eq!(b.tail + b.pause, 3_500_000);
//! ```

use std::sync::Arc;

use crate::pulse::{Plan, Playable, Timing};
use crate::tap::Tap;
use crate::tzx::{Tzx, TzxBlock};

/// A mounted tape image.
///
/// Cloning one is a reference count, so a machine that holds a tape can be
/// cloned into a checkpoint without copying it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Image {
    Tap(Arc<Tap>),
    Tzx(Arc<Tzx>),
}

impl Image {
    /// Blocks on the tape, of every kind: a TZX counts its text and control
    /// blocks, because they are what its jumps are relative to and what its
    /// block numbers mean.
    pub fn len(&self) -> usize {
        match self {
            Image::Tap(tap) => tap.len(),
            Image::Tzx(tzx) => tzx.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The file, as it would be written out.
    pub fn bytes(&self) -> &[u8] {
        match self {
            Image::Tap(tap) => tap.bytes(),
            Image::Tzx(tzx) => tzx.bytes(),
        }
    }

    /// FNV-1a over the file, which is the name ADR-0017 wants a recorded
    /// session to know its tape by.
    pub fn hash(&self) -> u64 {
        match self {
            Image::Tap(tap) => tap.hash(),
            Image::Tzx(tzx) => tzx.hash(),
        }
    }

    /// The bytes of block `index` if it is one a ROM loader could have read —
    /// a flag, some data and a checksum — and `None` for a block that is
    /// something else, which on a TZX is most of them.
    ///
    /// This is what the accelerated `LD-BYTES` needs and the only thing that
    /// wants a tape image in terms of bytes rather than pulses. A turbo block
    /// qualifies: what makes it turbo is the pulse lengths, and a trap does
    /// not play them.
    pub fn data_block(&self, index: usize) -> Option<&[u8]> {
        match self {
            Image::Tap(tap) => tap.block(index).map(|block| block.body()),
            Image::Tzx(tzx) => match tzx.block(index)? {
                TzxBlock::StandardData { data, .. }
                | TzxBlock::TurboData { data, .. }
                | TzxBlock::PureData { data, .. } => Some(tzx.span(data)),
                _ => None,
            },
        }
    }

    /// What block `index` plays. See [`Playable`].
    pub fn plan(&self, index: usize, timing: &Timing) -> Option<Plan<'_>> {
        match self {
            Image::Tap(tap) => tap.plan(index, timing),
            Image::Tzx(tzx) => tzx.plan(index, timing),
        }
    }
}

impl Playable for Image {
    fn plan(&self, index: usize, timing: &Timing) -> Option<Plan<'_>> {
        Image::plan(self, index, timing)
    }
}

impl From<Tap> for Image {
    fn from(tap: Tap) -> Image {
        Image::Tap(Arc::new(tap))
    }
}

impl From<Arc<Tap>> for Image {
    fn from(tap: Arc<Tap>) -> Image {
        Image::Tap(tap)
    }
}

impl From<Tzx> for Image {
    fn from(tzx: Tzx) -> Image {
        Image::Tzx(Arc::new(tzx))
    }
}

impl From<Arc<Tzx>> for Image {
    fn from(tzx: Arc<Tzx>) -> Image {
        Image::Tzx(tzx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tap::DATA_FLAG;

    fn pulses(image: &Image, timing: &Timing) -> Vec<crate::Pulse> {
        let mut player = crate::Player::new();
        let mut out = Vec::new();
        while let Some(pulse) = player.next_pulse(image, timing) {
            out.push(pulse);
        }
        out
    }

    #[test]
    fn a_tap_and_a_tzx_of_the_same_block_play_the_same_waveform() {
        let timing = Timing::rom(3_500_000);
        let tap = Image::from(Tap::builder().block(DATA_FLAG, &[0x42]).build());
        // A TZX standard speed block is a TAP block with its pause written
        // down, so up to that pause the two are the same signal.
        let tzx = Image::from(Tzx::builder().block(DATA_FLAG, &[0x42], 1000).build());

        let (tap, tzx) = (pulses(&tap, &timing), pulses(&tzx, &timing));
        assert_eq!(tap.len(), tzx.len());
        assert_eq!(tap[..tap.len() - 2], tzx[..tzx.len() - 2]);

        // What differs is how the last bit is ended. The ROM leaves 945
        // T-states and a TZX leaves a millisecond, and both are followed by
        // the rest of a second of silence.
        let seconds = |ends: &[crate::Pulse]| -> u32 { ends.iter().map(|p| p.ticks).sum() };
        assert_eq!(tap[tap.len() - 2].ticks, 945);
        assert_eq!(tzx[tzx.len() - 2].ticks, timing.ms);
        assert_eq!(seconds(&tzx[tzx.len() - 2..]), 3_500_000);
        assert_eq!(seconds(&tap[tap.len() - 2..]), 3_500_945);
    }

    #[test]
    fn only_the_blocks_a_rom_loader_could_read_have_bytes() {
        let image = Image::from(
            Tzx::builder()
                .text("a tape")
                .block(DATA_FLAG, &[1], 0)
                .tone(2168, 10)
                .build(),
        );

        assert_eq!(image.data_block(0), None, "a text block is not data");
        assert_eq!(
            image.data_block(1),
            Some(&[DATA_FLAG, 1, DATA_FLAG ^ 1][..])
        );
        assert_eq!(image.data_block(2), None, "a tone is not data");
        assert_eq!(image.data_block(3), None, "and neither is the end");
    }

    #[test]
    fn an_image_is_named_by_its_bytes() {
        let tap = Image::from(Tap::builder().block(DATA_FLAG, &[1]).build());
        let same = Image::from(Tap::parse(tap.bytes()).expect("well formed"));
        let other = Image::from(Tap::builder().block(DATA_FLAG, &[2]).build());

        assert_eq!(tap.hash(), same.hash());
        assert_ne!(tap.hash(), other.hash());
    }
}
