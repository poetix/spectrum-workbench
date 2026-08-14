//! Spectrum tapes: the bytes, the waveform they stand for, and the waveform
//! read back into bytes.
//!
//! A Spectrum tape is not a file format. It is an audio signal, and the only
//! thing the machine can see of it is one bit — `EAR`, bit 6 of a port `0xFE`
//! read — which the loading routine polls in a tight loop and times. Every
//! loader ever written measures the interval between two edges and decides
//! from it; none of them can see a byte, a block or a checksum. So the object
//! this crate is really about is a list of pulse lengths in T-states, and TAP
//! is a shorthand for one particular way of generating that list.
//!
//! That is why [`pulse`] is the middle of the crate rather than a detail of
//! [`tap`]: the waveform is what the machine consumes, and the ROM's own
//! timings are one [`Timing`] among the several a [`Tzx`] carries.
//!
//! # How it is laid out
//!
//! - [`tap`] is the simpler container: length-prefixed blocks, each a flag
//!   byte, some data and an XOR checksum. Parsing is validation only — a `Tap`
//!   holds the file bytes it was given and an index of where its blocks are.
//! - [`tzx`] is the other one: the same blocks with their timings written
//!   down, plus the ones no ROM ever wrote — turbo blocks, bare pulse
//!   sequences, recordings of the line — and control flow over them.
//! - [`pulse`] turns either into pulses: pilot, sync, two pulses a bit, a
//!   pause, the next block. [`Player`] is the state machine that walks it, and
//!   it is plain `Copy` data with the image passed in, because it lives inside
//!   an emulated machine that gets cloned into checkpoints (ADR-0017). It asks
//!   the image for a [`Plan`] per block, which is the only thing the two
//!   formats have to agree about.
//! - [`image`] is the pair of them behind an [`std::sync::Arc`], which is what
//!   a deck mounts.
//! - [`save`] is the same thing backwards: edges in, blocks out, for the `MIC`
//!   bit that a saving program drives. It writes TAP, because what the ROM
//!   saves is what a TAP file can hold.
//!
//! # Nothing here knows what a Spectrum is
//!
//! The clock rate arrives as an argument to [`Timing::rom`] and everything
//! else is in T-states. The reason is ADR-0021's: this crate is where the
//! format lives, and a crate that cannot name the machine cannot end up
//! holding a piece of it.
//!
//! ```
//! use rkw_tape::{Tap, Timing, pulse::Player};
//!
//! let tap = Tap::builder().block(0xFF, &[1, 2, 3]).build();
//! let timing = Timing::rom(3_500_000);
//!
//! // The first thing off the tape is a pilot pulse, and there are 3223 of
//! // them in front of a data block.
//! let mut player = Player::new();
//! assert_eq!(player.next_pulse(&tap, &timing).map(|p| p.ticks), Some(2168));
//! ```

pub mod image;
pub mod pulse;
pub mod save;
pub mod tap;
pub mod tzx;

pub use image::Image;
pub use pulse::{Data, Direct, Plan, Playable, Player, Pulse, Timing};
pub use save::Recorder;
pub use tap::{Block, Tap, TapError, checksum};
pub use tzx::{Tzx, TzxBlock, TzxError};
