//! What the outside world asks the emulation thread to do.
//!
//! These go in through the lossless ring of ADR-0007. They are rare — a person
//! types them, or a front end sends one per user action — and they are applied
//! at the control tick rather than the moment they arrive, which is what makes
//! them deterministic: the tick is a T-state, not a wall-clock instant, so the
//! same command log applied to the same starting state lands in the same
//! places every time. See [`Stamped`].
//!
//! # Why nothing here carries an id
//!
//! Breakpoint ids are minted on the emulation thread, so a command that
//! created one would have to send it back, and the way back is lossy. Every
//! command here therefore names what it means by address, which the sender
//! already knows. Asking the machine questions — what is armed, what is at
//! this address — is a request/response problem rather than a queue one, and
//! belongs to the command layer of ticket 0010.

use crate::ring::{self, Record};

/// One instruction to the emulation thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Run until something stops the machine.
    Resume,
    /// Stop at the next control tick, reporting [`crate::StopReason::Paused`].
    Pause,
    /// One instruction.
    Step,
    /// One instruction, treating a call or a repeating block instruction as
    /// one.
    StepOver,
    /// Run until the current routine returns.
    StepOut,
    /// Run until this address is reached.
    RunTo(u16),
    /// Set an execution breakpoint.
    Break(u16),
    /// Remove whatever breakpoint is at this address.
    Unbreak(u16),
    /// Watch an address for reads, writes or both.
    Watch { addr: u16, read: bool, write: bool },
    /// Stop watching an address.
    Unwatch(u16),
    /// Remove every breakpoint and watchpoint.
    ClearAll,
    /// Write a byte into memory without the machine noticing an access.
    Poke { addr: u16, value: u8 },
    /// The state of the machine's input matrix: every key that is down, as
    /// forty bits the machine reads however its hardware does.
    ///
    /// A whole matrix rather than a press or a release, because a frontend
    /// rebuilds it from the host keys it is holding and a lost edge would
    /// leave a key down forever. Sending the state is idempotent; sending an
    /// edge is not.
    ///
    /// What the bits mean is the machine's business — see
    /// [`Machine::set_keys`](crate::machine::Machine::set_keys). This crate
    /// only carries them, and a machine with no keyboard ignores them.
    Keys(u64),
    /// Reset the CPU. Not a power-on: the register file survives, as it does
    /// on real hardware.
    Reset,
    /// Put the program counter somewhere. A debugger moving `PC` is an input
    /// the machine could not have worked out for itself, so it goes through
    /// here rather than being written behind the log's back.
    SetPc(u16),
    /// Stop the thread. It hands the machine back when it is joined.
    Quit,
}

const RESUME: u64 = 1;
const PAUSE: u64 = 2;
const STEP: u64 = 3;
const STEP_OVER: u64 = 4;
const STEP_OUT: u64 = 5;
const RUN_TO: u64 = 6;
const BREAK: u64 = 7;
const UNBREAK: u64 = 8;
const WATCH: u64 = 9;
const UNWATCH: u64 = 10;
const CLEAR_ALL: u64 = 11;
const POKE: u64 = 12;
const QUIT: u64 = 13;
const RESET: u64 = 14;
const SET_PC: u64 = 15;
const KEYS: u64 = 16;

/// How much of a [`Command::Keys`] word is matrix: forty keys, which is what
/// fits above the kind byte and is what a Spectrum has. A sender with a wider
/// matrix than this needs a wider record, and would find out here rather than
/// by having its top bits quietly dropped.
const KEYS_BITS: u32 = 40;
const KEYS_MASK: u64 = (1 << KEYS_BITS) - 1;

/// A command and the T-state at which the emulation thread applied it.
///
/// A log of these replays exactly: apply each one when the machine's clock
/// reaches its stamp and the run is the run that happened, which turns "it
/// crashed after I poked that byte" into a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamped {
    pub t: u64,
    pub command: Command,
}

impl Record for Command {
    /// Everything fits in the first word: a kind byte, then an address and a
    /// value above it. The second word is reserved, and is required to be zero
    /// on the way back in so that a future field cannot be read as an old
    /// command silently.
    fn encode(self) -> [u64; 2] {
        let word = match self {
            Command::Resume => RESUME,
            Command::Pause => PAUSE,
            Command::Step => STEP,
            Command::StepOver => STEP_OVER,
            Command::StepOut => STEP_OUT,
            Command::RunTo(addr) => RUN_TO | u64::from(addr) << 8,
            Command::Break(addr) => BREAK | u64::from(addr) << 8,
            Command::Unbreak(addr) => UNBREAK | u64::from(addr) << 8,
            Command::Watch { addr, read, write } => {
                WATCH | u64::from(addr) << 8 | u64::from(read) << 24 | u64::from(write) << 25
            }
            Command::Unwatch(addr) => UNWATCH | u64::from(addr) << 8,
            Command::ClearAll => CLEAR_ALL,
            Command::Poke { addr, value } => POKE | u64::from(addr) << 8 | u64::from(value) << 24,
            Command::Keys(matrix) => KEYS | (matrix & KEYS_MASK) << 8,
            Command::Reset => RESET,
            Command::SetPc(addr) => SET_PC | u64::from(addr) << 8,
            Command::Quit => QUIT,
        };
        [word, 0]
    }

    fn decode(words: [u64; 2]) -> Option<Self> {
        if words[1] != 0 {
            return None;
        }
        let word = words[0];
        let addr = (word >> 8) as u16;
        let value = (word >> 24) as u8;
        match word & 0xFF {
            RESUME => Some(Command::Resume),
            PAUSE => Some(Command::Pause),
            STEP => Some(Command::Step),
            STEP_OVER => Some(Command::StepOver),
            STEP_OUT => Some(Command::StepOut),
            RUN_TO => Some(Command::RunTo(addr)),
            BREAK => Some(Command::Break(addr)),
            UNBREAK => Some(Command::Unbreak(addr)),
            WATCH => Some(Command::Watch {
                addr,
                read: word >> 24 & 1 != 0,
                write: word >> 25 & 1 != 0,
            }),
            UNWATCH => Some(Command::Unwatch(addr)),
            CLEAR_ALL => Some(Command::ClearAll),
            POKE => Some(Command::Poke { addr, value }),
            KEYS => Some(Command::Keys(word >> 8 & KEYS_MASK)),
            RESET => Some(Command::Reset),
            SET_PC => Some(Command::SetPc(addr)),
            QUIT => Some(Command::Quit),
            _ => None,
        }
    }
}

/// The writing end of the command ring, which is the UI's.
pub type CommandTx = ring::Producer<Command>;
/// The reading end, which is the emulation thread's.
pub type CommandRx = ring::Consumer<Command>;

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Command] = &[
        Command::Resume,
        Command::Pause,
        Command::Step,
        Command::StepOver,
        Command::StepOut,
        Command::RunTo(0x8000),
        Command::Break(0xFFFF),
        Command::Unbreak(0x0001),
        Command::Watch {
            addr: 0x4000,
            read: true,
            write: false,
        },
        Command::Watch {
            addr: 0x4000,
            read: false,
            write: true,
        },
        Command::Unwatch(0x4000),
        Command::ClearAll,
        Command::Poke {
            addr: 0x5C00,
            value: 0xA5,
        },
        Command::Reset,
        Command::SetPc(0x8000),
        Command::Keys(0),
        Command::Keys(KEYS_MASK),
        Command::Keys(0x00_5A_A5_5A_A5),
        Command::Quit,
    ];

    #[test]
    fn every_command_round_trips() {
        for c in ALL {
            assert_eq!(Command::decode(c.encode()), Some(*c), "{c:?}");
        }
    }

    /// The forty bits are all there is room for above the kind byte, so a
    /// sender that put something in bit 40 would get it back as zero rather
    /// than as a stray key.
    #[test]
    fn a_matrix_wider_than_the_record_is_truncated_rather_than_corrupting_the_kind() {
        let too_wide = Command::Keys(1 << KEYS_BITS | 1);
        assert_eq!(Command::decode(too_wide.encode()), Some(Command::Keys(1)));
    }

    #[test]
    fn unknown_kinds_and_reserved_bits_decode_to_nothing() {
        assert_eq!(Command::decode([0, 0]), None);
        assert_eq!(Command::decode([0xFE, 0]), None);
        assert_eq!(Command::decode([RESUME, 1]), None);
    }
}
