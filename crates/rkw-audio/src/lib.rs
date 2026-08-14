//! The beeper: one bit of a port at 3.5 MHz, turned into sound at whatever
//! rate the host audio device happens to run at.
//!
//! ```
//! use rkw_audio::{EdgeLog, Levels};
//!
//! let mut log = EdgeLog::new();
//! // A 1 kHz note: the speaker flips every 1750 T-states.
//! for half in 0..40u32 {
//!     let levels = Levels { speaker: half % 2 == 0, mic: false };
//!     log.record(half * 1750, levels);
//! }
//! assert_eq!(log.frame().0.len(), 40);
//! ```
//!
//! # This crate has never heard of a Spectrum
//!
//! Everything here takes the clock rate, the frame length and the sample rate
//! as parameters. That is not generality for its own sake: ADR-0017 requires
//! that host state and wall-clock time stay outside the machine, or replay
//! stops being deterministic, and the sample rate is the most host-shaped
//! number in the program. A crate that cannot name `Spectrum` cannot acquire
//! machine state by accident, and a machine that depends on this one cannot
//! acquire a sample rate by accident either. It also means the resampler can
//! be tested against a 1 kHz clock at ten samples a second, where every
//! boundary is an integer and the right answer can be worked out on paper.
//!
//! # How it is laid out
//!
//! - [`edges`] is what the speaker did and when: a fixed log of level changes
//!   stamped with their T-state, which is the only part of this that lives
//!   inside the machine.
//! - [`resample`] turns those edges into samples, by averaging the level over
//!   each output window rather than reading it at each output instant.
//! - [`filter`] is the speaker: the cone the signal came out of in 1984, which
//!   is most of what makes a Spectrum sound like one.
//! - [`ring`] is the buffer between the emulation thread, which makes 20 ms of
//!   sound at a time, and the device, which asks for a few milliseconds of it
//!   on the dot.
//! - [`beeper`] is the whole path assembled, and is what the emulation thread
//!   calls once a frame.
//! - [`output`] is the device's end: the volume knob, and what to play when
//!   the machine has not kept up.

pub mod beeper;
pub mod edges;
pub mod filter;
pub mod output;
pub mod resample;
pub mod ring;

pub use beeper::{Beeper, Config};
pub use edges::{EdgeLog, Levels, MAX_EDGES, levels_of, pack, tick_of};
pub use filter::{Chain, Speaker};
pub use output::{Fill, Output, Volume};
pub use resample::{DECIMATOR_TAPS, Decimator, Rates, Windowed};
pub use ring::{SampleRx, SampleTx};
