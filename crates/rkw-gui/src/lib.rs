//! The windowed front end: a window, a keyboard, and a speaker.
//!
//! ```text
//!   window thread                       emulation thread
//!   ─────────────                       ────────────────
//!   key events ──► HostKeys ──► Command::Keys ──► Spectrum's matrix
//!   Framebuffer ◄── swap chain ◄── Presenting ◄── end of frame
//!                                       │
//!   cpal callback ◄── Output ◄── sample ring ◄── AudioMachine
//!                       │
//!                       └── fill level ──► the pacer, which is what
//!                                          decides when the machine runs
//! ```
//!
//! Three channels out of ADR-0007 carry commands, events and stops, and this
//! adds two more that could not go through them: the picture, which is a
//! hundred kilobytes rather than sixteen bytes (ADR-0025), and the sound,
//! which the audio device pulls at its own rate rather than the machine's.
//!
//! # What is where
//!
//! - [`keys`] is a host key press turned into either a key on the machine or
//!   a command to this frontend.
//! - [`pacing`] is how fast the machine is allowed to run, which at normal
//!   speed is a question about the sample ring and not about a timer.
//! - [`speaker`] is the `cpal` end: a device, a stream, and a callback that
//!   neither blocks nor allocates.
//! - [`session`] assembles the machine, puts it on a thread, and is the only
//!   thing the window can reach it through.
//! - [`app`] is the `winit` event loop and the blit.
//!
//! Everything but the two device modules is testable without a window or a
//! sound card, which is why the policy lives outside them.

pub mod app;
pub mod keys;
pub mod pacing;
pub mod session;
pub mod speaker;

pub use app::App;
pub use keys::{Action, Hotkey};
pub use pacing::{Speed, SpeedControl};
pub use session::Session;
