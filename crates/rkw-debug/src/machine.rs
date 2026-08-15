//! What the slice loop needs from a machine beyond a bus.
//!
//! ADR-0002 says the CPU never adds up T-states — the bus does, because
//! contention is a property of the machine and not of the instruction. The
//! slice loop of ADR-0007 runs to a T-state deadline, so it has to be able to
//! ask what the time is; that is [`Clock`], and every bus already knows the
//! answer because every bus counts `tick`s.
//!
//! [`Machine`] adds the schedule. A slice ends at the earliest of the control
//! tick and the next scheduled hardware event, so the loop asks the machine
//! when its next one is and hands control back to it when the clock arrives
//! there. Nothing in this crate has a hardware event to schedule — the frame
//! interrupt is ticket 0012 and the tape is 0016 — so the default is "none",
//! and the ULA implements two methods rather than a loop.

use z80::disasm::Peek;
use z80::{Bus, FlatMemory};

use crate::command::Tape;

/// Something that knows how much emulated time has passed.
pub trait Clock {
    /// T-states since the machine was made. Monotonic: an emulator that reset
    /// this would make every deadline in flight meaningless.
    fn t_states(&self) -> u64;
}

impl Clock for FlatMemory {
    fn t_states(&self) -> u64 {
        self.t
    }
}

/// Everything the emulation thread needs of the machine it is running.
///
/// The two scheduling methods have defaults because a bare memory has no
/// hardware to schedule. A Spectrum overrides them: `next_event` returns the
/// T-state of the next frame interrupt or tape edge, and `service_event` is
/// where the ULA raises it and works out when the following one is due.
pub trait Machine: Bus + Peek + Clock {
    /// The T-state of the next scheduled hardware event, if there is one. The
    /// slice loop will not run past it.
    fn next_event(&self) -> Option<u64> {
        None
    }

    /// The clock has reached [`Machine::next_event`]. Raise whatever was due
    /// and schedule the one after it.
    fn service_event(&mut self) {}

    /// Every key the user is holding down, as a bit per key.
    ///
    /// The layout is the machine's own — a Spectrum reads this as its eight
    /// half-rows of five, low half-row first — and a machine with no keyboard
    /// ignores it, which is why there is a default. What matters here is where
    /// it arrives from: [`Command::Keys`](crate::command::Command::Keys), so
    /// that a keypress is applied at a T-state the machine agrees with and
    /// lands in the replay log with everything else the user did. A frontend
    /// that stored the matrix in an atomic the ULA read directly would be
    /// faster by 64 µs and would make every recorded session unreproducible.
    fn set_keys(&mut self, _matrix: u64) {}

    /// Press a button on the tape deck, if there is one.
    ///
    /// Here rather than on the frontend's own copy of the machine for the same
    /// reason as [`Machine::set_keys`]: there is no such copy. The machine is
    /// on the emulation thread, and starting a tape is an input that has to
    /// land at a T-state the machine agrees with — a pilot tone that began a
    /// slice earlier or later is a different load.
    fn tape(&mut self, _button: Tape) {}
}

impl Machine for FlatMemory {}
