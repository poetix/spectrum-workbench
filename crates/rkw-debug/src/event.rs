//! What the emulation thread says on the way past, in sixteen bytes a time.
//!
//! These go out through the lossy ring of ADR-0007: high volume, and worth
//! nothing if producing one can stall the machine. Every event carries the
//! T-state it happened at, which is what lets a consumer put events from
//! different sources in order without a lock and without a clock of its own.
//!
//! Only [`Event::Applied`] has a producer today. The trace ring (ticket 0022)
//! is what [`Event::Exec`] is for, and the display write journal (ticket 0025)
//! is what [`Event::MemWrite`] is for; both are hooks in the run loop rather
//! than new machinery, so the vocabulary is defined here where its encoding
//! can be round-tripped.
//!
//! # The layout
//!
//! The first word is the T-state, whole. The second is a kind byte and the
//! payload above it, which for every kind so far is a 16-bit address or port
//! and a byte of data. Nothing here is packed tightly enough to be clever
//! about; the size is set by the slot, not by the fields.

use crate::ring::{self, Record};

/// One thing that happened, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// An instruction was fetched at `pc`.
    Exec { t: u64, pc: u16 },
    /// The machine wrote `value` to `addr`.
    MemWrite { t: u64, addr: u16, value: u8 },
    /// The machine wrote `value` to `port`.
    PortOut { t: u64, port: u16, value: u8 },
    /// The `seq`th command since the thread started was applied at `t`.
    ///
    /// This is the stamp of ADR-0007: the sender knows when a command took
    /// effect in emulated time, not merely that it did.
    Applied { t: u64, seq: u32 },
}

const EXEC: u64 = 1;
const MEM_WRITE: u64 = 2;
const PORT_OUT: u64 = 3;
const APPLIED: u64 = 4;

impl Event {
    /// The T-state the event happened at, whichever kind it is.
    pub fn t(self) -> u64 {
        match self {
            Event::Exec { t, .. }
            | Event::MemWrite { t, .. }
            | Event::PortOut { t, .. }
            | Event::Applied { t, .. } => t,
        }
    }
}

impl Record for Event {
    fn encode(self) -> [u64; 2] {
        let (t, kind, payload) = match self {
            Event::Exec { t, pc } => (t, EXEC, u64::from(pc) << 8),
            Event::MemWrite { t, addr, value } => {
                (t, MEM_WRITE, u64::from(addr) << 8 | u64::from(value) << 24)
            }
            Event::PortOut { t, port, value } => {
                (t, PORT_OUT, u64::from(port) << 8 | u64::from(value) << 24)
            }
            Event::Applied { t, seq } => (t, APPLIED, u64::from(seq) << 8),
        };
        [t, kind | payload]
    }

    fn decode(words: [u64; 2]) -> Option<Self> {
        let t = words[0];
        let addr = (words[1] >> 8) as u16;
        let value = (words[1] >> 24) as u8;
        match words[1] & 0xFF {
            EXEC => Some(Event::Exec { t, pc: addr }),
            MEM_WRITE => Some(Event::MemWrite { t, addr, value }),
            PORT_OUT => Some(Event::PortOut {
                t,
                port: addr,
                value,
            }),
            APPLIED => Some(Event::Applied {
                t,
                seq: (words[1] >> 8) as u32,
            }),
            _ => None,
        }
    }
}

/// The writing end of the event ring, which is the emulation thread's.
pub type EventTx = ring::Producer<Event>;
/// The reading end, which is the debugger's or the UI's.
pub type EventRx = ring::Consumer<Event>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips() {
        let events = [
            Event::Exec {
                t: 0x1234_5678_9ABC,
                pc: 0x8000,
            },
            Event::MemWrite {
                t: 1,
                addr: 0x4000,
                value: 0xFF,
            },
            Event::PortOut {
                t: u64::MAX,
                port: 0xFEFE,
                value: 0x7F,
            },
            Event::Applied {
                t: 224,
                seq: u32::MAX,
            },
        ];
        for e in events {
            assert_eq!(Event::decode(e.encode()), Some(e), "{e:?}");
        }
    }

    #[test]
    fn an_unknown_kind_decodes_to_nothing() {
        assert_eq!(Event::decode([0, 0]), None);
        assert_eq!(Event::decode([0, 0xFF]), None);
    }

    #[test]
    fn t_is_available_without_matching() {
        assert_eq!(Event::Applied { t: 99, seq: 1 }.t(), 99);
    }
}
