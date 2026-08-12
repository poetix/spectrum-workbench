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
}

impl Machine for FlatMemory {}
