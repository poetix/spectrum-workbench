//! The debugger core: what stops the machine, and what moves it.
//!
//! This crate knows about a CPU and a bus and nothing about a user interface
//! (ADR-0013). It owns the breakpoint and watchpoint state, the check that
//! runs per instruction and per memory access, and the four ways of moving
//! that are not "run": step, step over, step out, run to cursor. It also owns
//! the emulation thread those movements run on ([`Emu`]) and the three
//! channels of ADR-0007 that connect it to whatever is driving.
//!
//! On top of that sits [`cmd`], the command layer: a parser producing
//! [`Request`] values, an executor returning structured [`Outcome`]s, and a
//! formatter that turns those into terminal text. Those three are kept apart so
//! that a front end can take the first two and none of the third — a REPL runs
//! all three, and `rkw-dap` will run parse and execute and serialise the result
//! (ADR-0016). The terminal shell over it is the `rkw-cli` crate, and it is
//! thin on purpose.
//!
//! # What it needs from a machine
//!
//! `Bus + Peek`. The bus is how the CPU runs; [`Peek`] is how the debugger
//! looks without the machine noticing — the byte a write is about to replace,
//! the return address on the stack, the instruction under `PC`. Keeping the
//! two apart is what makes it impossible for a debugger read to be mistaken
//! for a machine read.
//!
//! # What it costs
//!
//! Per instruction: one predictable branch when nothing is armed, one bit test
//! when something is (ADR-0008). Per memory access: the same. Everything
//! else — conditions, hit counts, ignore counts, the map lookup that says
//! which breakpoint this is — happens only once the bitmap has said yes.
//!
//! ```
//! use rkw_debug::{Debugger, StopReason};
//! use z80::{Cpu, FlatMemory};
//!
//! let mut mem = FlatMemory::new();
//! mem.load(0x8000, &[0x3E, 0x2A, 0x00, 0x76]); // LD A,42 ; NOP ; HALT
//!
//! let mut cpu = Cpu::new();
//! cpu.regs.pc = 0x8000;
//!
//! let mut dbg = Debugger::new();
//! let id = dbg.breakpoints.add_exec(0x8002);
//!
//! assert_eq!(dbg.resume(&mut cpu, &mut mem, 100), StopReason::Breakpoint { id, addr: 0x8002 });
//! assert_eq!(cpu.regs.a, 42);
//! ```

mod bitmap;
pub mod breakpoints;
mod bus;
pub mod cmd;
pub mod command;
pub mod condition;
pub mod emu;
pub mod event;
pub mod machine;
pub mod ring;

pub use bitmap::Bitmap;
pub use breakpoints::{Access, Breakpoint, Breakpoints, Id, PortAccess, PortWatch, Watchpoint};
pub use cmd::{Outcome, Request, Session};
pub use command::{Command, Stamped};
pub use condition::{Cmp, Condition, Operand};
pub use emu::{Config, Emu, Handle, RunState, spawn};
pub use event::Event;
pub use machine::{Clock, Machine};

use breakpoints::Temporary;
use bus::DebugBus;
use z80::disasm::{Flow, Peek, decode};
use z80::{Bus, Cpu, Stop};

/// Why the machine stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The requested movement finished: one instruction stepped, a step-over
    /// or step-out returned, a run-to-cursor arrived.
    Step,
    Breakpoint {
        id: Id,
        addr: u16,
    },
    Watchpoint {
        id: Id,
        addr: u16,
        access: Access,
        /// Equal to `new` for a read.
        old: u8,
        new: u8,
    },
    PortWatchpoint {
        id: Id,
        port: u16,
        access: PortAccess,
        value: u8,
    },
    /// `HALT` with interrupts disabled: nothing can wake the CPU, so running
    /// on would only burn the budget. A `HALT` with interrupts enabled is not
    /// a stop, because an interrupt is expected to arrive.
    Halted,
    /// Someone asked the machine to stop. [`Command::Pause`], applied at the
    /// control tick like every other command.
    Paused,
    /// The instruction budget ran out, or the slice reached its deadline. Not
    /// an error — it is how a caller keeps control, and how [`Emu`] hands back
    /// between slices without the movement in progress being abandoned.
    OutOfBudget,
}

impl StopReason {
    /// True for the reasons a user asked for, as against the ones that
    /// happened to them.
    pub fn is_requested(self) -> bool {
        matches!(
            self,
            StopReason::Step | StopReason::Paused | StopReason::OutOfBudget
        )
    }
}

/// The debugger. Owns what is armed; borrows the CPU and the machine when it
/// is asked to move them.
#[derive(Default)]
pub struct Debugger {
    pub breakpoints: Breakpoints,
}

impl Debugger {
    pub fn new() -> Self {
        Debugger {
            breakpoints: Breakpoints::new(),
        }
    }

    /// Run one instruction, whatever is armed. Watchpoints still report,
    /// because a step that silently corrupted a watched byte would be a lie,
    /// but a breakpoint at the address stepped to does not stop anything that
    /// has already stopped.
    pub fn step<B: Bus + Peek>(&mut self, cpu: &mut Cpu, bus: &mut B) -> StopReason {
        self.breakpoints.clear_temporaries();
        match self.step_one(cpu, bus) {
            Some(reason) => reason,
            None => StopReason::Step,
        }
    }

    /// Step, but treat a call as one instruction.
    ///
    /// A conditional call that is not taken, or a `RET` that is, is just a
    /// step. When a call is taken, the landing site is the address after it,
    /// guarded by the stack pointer: a recursive call reaching the same return
    /// address arrives with a lower `SP`, and stepping over one call should
    /// not stop inside the next one down.
    ///
    /// A repeating block instruction — `LDIR` and its family — does not
    /// advance `PC` between iterations, so stepping it would appear to do
    /// nothing. Step-over runs it to completion, which is what "over" means
    /// for an instruction that is its own loop.
    pub fn step_over<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        budget: u64,
    ) -> StopReason {
        match self.begin_step_over(cpu, bus) {
            Some(reason) => reason,
            None => self.run(cpu, bus, budget),
        }
    }

    /// The arming half of [`Debugger::step_over`]: `Some` if it is already
    /// over, `None` if the machine now has to run to the temporary this left
    /// behind.
    ///
    /// Split out because a slice loop cannot call a method that runs to
    /// completion. What the two halves share is that arming happens between
    /// runs, never mid-slice.
    pub(crate) fn begin_step_over<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
    ) -> Option<StopReason> {
        self.breakpoints.clear_temporaries();
        let d = decode(bus, cpu.regs.pc);
        let returns_here = d.next_addr();
        let sp_before = cpu.regs.sp;

        match d.flow {
            Flow::Call { .. } | Flow::Rst(_) | Flow::Repeat => {}
            _ => return Some(self.step(cpu, bus)),
        }

        if let Some(reason) = self.step_one(cpu, bus) {
            return Some(reason);
        }
        if cpu.regs.pc == returns_here {
            // The call was conditional and not taken, or the block instruction
            // was on its last iteration.
            return Some(StopReason::Step);
        }

        let guard = match d.flow {
            // A repeat has pushed nothing, so there is no stack to guard on.
            Flow::Repeat => None,
            _ => Some(sp_before),
        };
        self.breakpoints.add_temporary(Temporary {
            addr: returns_here,
            sp_at_least: guard,
        });
        None
    }

    /// Run until the current routine returns.
    ///
    /// The return address is taken from the top of the stack, which is where a
    /// `CALL` put it. That is a guess — the top of the stack is whatever the
    /// routine last pushed — and it is the same guess every debugger of this
    /// kind makes. When it is wrong the run stops somewhere unexpected or
    /// hits the budget, rather than misreporting anything.
    pub fn step_out<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        budget: u64,
    ) -> StopReason {
        self.begin_step_out(cpu, bus);
        self.run(cpu, bus, budget)
    }

    /// The arming half of [`Debugger::step_out`].
    pub(crate) fn begin_step_out<B: Bus + Peek>(&mut self, cpu: &mut Cpu, bus: &mut B) {
        self.breakpoints.clear_temporaries();
        let sp = cpu.regs.sp;
        let ret = u16::from_le_bytes([bus.peek(sp), bus.peek(sp.wrapping_add(1))]);
        self.breakpoints.add_temporary(Temporary {
            addr: ret,
            // After the return, SP is two higher than it is now.
            sp_at_least: Some(sp.wrapping_add(2)),
        });
    }

    /// Run until `addr` is reached, or something else stops first.
    ///
    /// Running to the address already under `PC` runs a whole lap rather than
    /// returning immediately, which is what "run to cursor" means inside a
    /// loop.
    pub fn run_to<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        addr: u16,
        budget: u64,
    ) -> StopReason {
        self.begin_run_to(addr);
        self.run(cpu, bus, budget)
    }

    /// The arming half of [`Debugger::run_to`].
    pub(crate) fn begin_run_to(&mut self, addr: u16) {
        self.breakpoints.clear_temporaries();
        self.breakpoints.add_temporary(Temporary {
            addr,
            sp_at_least: None,
        });
    }

    /// Run until something stops the machine or the budget runs out.
    ///
    /// The first instruction always runs, so resuming from a breakpoint does
    /// not immediately hit it again.
    pub fn resume<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        budget: u64,
    ) -> StopReason {
        self.breakpoints.clear_temporaries();
        self.run(cpu, bus, budget)
    }

    /// Run until the machine's clock reaches `deadline`, or something stops it
    /// first.
    ///
    /// This is what the emulation thread calls, once per slice (ADR-0007). It
    /// arms nothing and clears nothing, because the movement it is continuing
    /// was armed before the first slice: a step-over that takes a million
    /// T-states is one arming and several thousand slices, and its landing
    /// site has to survive every one of them.
    ///
    /// The deadline is a floor rather than an exact stop. Instructions are not
    /// interruptible, so the last one of a slice runs past it by up to twenty
    /// or so T-states, and the next slice's deadline is measured from where
    /// the clock actually got to.
    pub fn run_until<B: Bus + Peek + Clock>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        deadline: u64,
    ) -> StopReason {
        self.run_limited(cpu, bus, |bus: &B, _| bus.t_states() < deadline)
    }

    /// The loop everything that moves ends up in.
    ///
    /// There are two of them, chosen once per run by whether anything is
    /// watching the bus. Wrapping the bus is what a watchpoint costs, and it
    /// is not the bit test — it is that the CPU is then monomorphised against
    /// a bus that reaches the real one through a pointer, which costs about
    /// 40% on the measurement in `tests/throughput.rs`. A run with no
    /// watchpoints armed hands the CPU the machine's own bus and pays none of
    /// it.
    ///
    /// Deciding once per run is sound because arming happens between runs:
    /// commands are applied at the control tick, never mid-slice (ADR-0007).
    fn run<B: Bus + Peek>(&mut self, cpu: &mut Cpu, bus: &mut B, budget: u64) -> StopReason {
        self.run_limited(cpu, bus, |_: &B, done| done < budget)
    }

    /// The same loop, with what ends it left to the caller: an instruction
    /// count for a budgeted run, the clock for a slice. `more` is handed the
    /// machine's own bus and the number of instructions run so far, and is
    /// asked before each one.
    fn run_limited<B: Bus + Peek, F: FnMut(&B, u64) -> bool>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        more: F,
    ) -> StopReason {
        let reason = if self.breakpoints.bus_armed() {
            self.run_watched(cpu, bus, more)
        } else {
            self.run_unwatched(cpu, bus, more)
        };
        // A run that stopped abandons any pending step; one that merely ran
        // out of budget has not finished, and its landing site stays armed.
        if reason != StopReason::OutOfBudget {
            self.breakpoints.clear_temporaries();
        }
        reason
    }

    /// Nothing is watching the bus, so the CPU is handed the machine's own.
    fn run_unwatched<B: Bus + Peek, F: FnMut(&B, u64) -> bool>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        mut more: F,
    ) -> StopReason {
        let mut done = 0;
        while more(bus, done) {
            let stop = cpu.step(bus);
            done += 1;
            if let Some(reason) = between(&mut self.breakpoints, cpu, bus, stop) {
                return reason;
            }
        }
        StopReason::OutOfBudget
    }

    /// Something is watching the bus, so every access goes through the check.
    fn run_watched<B: Bus + Peek, F: FnMut(&B, u64) -> bool>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        mut more: F,
    ) -> StopReason {
        let mut dbus = DebugBus::new(bus, &mut self.breakpoints);
        let mut done = 0;
        while more(dbus.split().1, done) {
            let stop = cpu.step(&mut dbus);
            done += 1;
            if let Some(reason) = dbus.take_hit() {
                return reason;
            }
            let (breaks, mem) = dbus.split();
            if let Some(reason) = between(breaks, cpu, mem, stop) {
                return reason;
            }
        }
        StopReason::OutOfBudget
    }

    /// One instruction, with the bus wrapped so watchpoints can see it.
    /// `None` means nothing stopped it.
    fn step_one<B: Bus + Peek>(&mut self, cpu: &mut Cpu, bus: &mut B) -> Option<StopReason> {
        let mut dbus = DebugBus::new(bus, &mut self.breakpoints);
        let stop = cpu.step(&mut dbus);
        if let Some(hit) = dbus.take_hit() {
            return Some(hit);
        }
        if stop == Stop::Halt && !cpu.regs.iff1 {
            return Some(StopReason::Halted);
        }
        None
    }
}

/// What both loops do between instructions: notice a `HALT` nothing can end,
/// then ask whether the new `PC` is armed.
#[inline]
fn between<P: Peek>(
    breaks: &mut Breakpoints,
    cpu: &Cpu,
    mem: &P,
    stop: Stop,
) -> Option<StopReason> {
    // A HALT that no interrupt can end is a stop; one that an interrupt will
    // end is the machine waiting, which is not the debugger's business.
    if stop == Stop::Halt && !cpu.regs.iff1 {
        return Some(StopReason::Halted);
    }
    check_exec(breaks, cpu, mem)
}

/// Tier 1 and 2 for execution, and tier 3 only if they say so.
///
/// Temporaries are checked first: if a step-over's landing site coincides with
/// a user breakpoint, the user asked for the step and the step is what
/// completed.
fn check_exec<P: Peek>(breaks: &mut Breakpoints, cpu: &Cpu, mem: &P) -> Option<StopReason> {
    let pc = cpu.regs.pc;
    if !breaks.exec_maybe(pc) {
        return None;
    }
    if breaks.fired_temporary(pc, cpu.regs.sp) {
        return Some(StopReason::Step);
    }
    let id = breaks.fired_exec(pc, &cpu.regs, mem)?;
    Some(StopReason::Breakpoint { id, addr: pc })
}
