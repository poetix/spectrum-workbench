//! The command layer: a parser, an executor and a formatter, in that order and
//! kept apart on purpose (ADR-0013, ADR-0016).
//!
//! ```text
//! "break $8002 if a == $2a"
//!        │
//!   parse│  Request::Break { addr, condition }
//!        ▼
//!    execute  Outcome::Armed(Breakpoint { id: 1, .. })
//!        │
//!  format▼
//! "Breakpoint 1 at $8002 if a == $2a"
//! ```
//!
//! A REPL runs all three. A DAP adapter runs the first two and serialises the
//! result to JSON, except for `evaluate` with `context: "repl"`, which runs all
//! three and is thereby the same debugger inside an editor. Nothing scrapes the
//! formatter's output, and the executor never returns a `String` where it could
//! return a value — which is cheap here and expensive to retrofit, because a
//! formatter-shaped executor forces a front end to parse its own debugger's
//! output.
//!
//! ```
//! use rkw_debug::cmd::{Outcome, Session, format};
//! use rkw_debug::emu::Config;
//! use z80::{Cpu, FlatMemory};
//!
//! let mut mem = FlatMemory::new();
//! mem.load(0x8000, &[0x3E, 0x2A, 0x00, 0x76]); // LD A,42 ; NOP ; HALT
//! let mut cpu = Cpu::new();
//! cpu.regs.pc = 0x8000;
//!
//! let mut session = Session::new(cpu, mem, Config::default());
//! session.run_line("break $8002").unwrap();
//! let outcome = session.run_line("continue").unwrap().unwrap();
//!
//! assert_eq!(session.regs().a, 42);
//! assert!(format::render(&outcome).starts_with("Breakpoint 1 at $8002"));
//! ```

pub mod exec;
pub mod format;
pub mod parse;

pub use exec::{
    Armed, ArmedList, Disassembly, Error, ExecError, LIST_CONTEXT, MemoryDump, Outcome,
    RegisterView, Session, Stop, StringAt, Trace,
};
pub use parse::{Addr, Base, Format, Info, ParseError, Place, Request, Unit, parse};

/// Re-exported so that a front end driving the command layer has one crate to
/// depend on: everything source-level in an [`Outcome`] is one of these.
pub use rkw_dbginfo::{DebugInfo, Frame, Listing, Located, ResolveError, Site, Sources};
