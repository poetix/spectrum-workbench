//! The debugger core: what stops the machine, and what moves it.
//!
//! This crate knows about a CPU and a bus and nothing about a user interface
//! (ADR-0013). It owns the breakpoint and watchpoint state, the check that
//! runs per instruction and per memory access, and the four ways of moving
//! that are not "run": step, step over, step out, run to cursor. The command
//! layer and the REPL (ticket 0010) sit on top; the emulation thread and its
//! channels (ticket 0009) will drive [`Debugger::resume`] with a deadline
//! instead of an instruction budget.
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
pub mod condition;

pub use bitmap::Bitmap;
pub use breakpoints::{Access, Breakpoint, Breakpoints, Id, PortAccess, PortWatch, Watchpoint};
pub use condition::{Cmp, Condition, Operand};

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
    /// The instruction budget ran out. Not an error — it is how a caller keeps
    /// control, and how the slice loop of ticket 0009 will hand back.
    OutOfBudget,
}

impl StopReason {
    /// True for the reasons a user asked for, as against the ones that
    /// happened to them.
    pub fn is_requested(self) -> bool {
        matches!(self, StopReason::Step | StopReason::OutOfBudget)
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
        self.breakpoints.clear_temporaries();
        let d = decode(bus, cpu.regs.pc);
        let returns_here = d.next_addr();
        let sp_before = cpu.regs.sp;

        match d.flow {
            Flow::Call { .. } | Flow::Rst(_) | Flow::Repeat => {}
            _ => return self.step(cpu, bus),
        }

        if let Some(reason) = self.step_one(cpu, bus) {
            return reason;
        }
        if cpu.regs.pc == returns_here {
            // The call was conditional and not taken, or the block instruction
            // was on its last iteration.
            return StopReason::Step;
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
        self.run(cpu, bus, budget)
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
        self.breakpoints.clear_temporaries();
        let sp = cpu.regs.sp;
        let ret = u16::from_le_bytes([bus.peek(sp), bus.peek(sp.wrapping_add(1))]);
        self.breakpoints.add_temporary(Temporary {
            addr: ret,
            // After the return, SP is two higher than it is now.
            sp_at_least: Some(sp.wrapping_add(2)),
        });
        self.run(cpu, bus, budget)
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
        self.breakpoints.clear_temporaries();
        self.breakpoints.add_temporary(Temporary {
            addr,
            sp_at_least: None,
        });
        self.run(cpu, bus, budget)
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
        let reason = if self.breakpoints.bus_armed() {
            self.run_watched(cpu, bus, budget)
        } else {
            self.run_unwatched(cpu, bus, budget)
        };
        // A run that stopped abandons any pending step; one that merely ran
        // out of budget has not finished, and its landing site stays armed.
        if reason != StopReason::OutOfBudget {
            self.breakpoints.clear_temporaries();
        }
        reason
    }

    /// Nothing is watching the bus, so the CPU is handed the machine's own.
    fn run_unwatched<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        budget: u64,
    ) -> StopReason {
        for _ in 0..budget {
            let stop = cpu.step(bus);
            if let Some(reason) = between(&mut self.breakpoints, cpu, bus, stop) {
                return reason;
            }
        }
        StopReason::OutOfBudget
    }

    /// Something is watching the bus, so every access goes through the check.
    fn run_watched<B: Bus + Peek>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        budget: u64,
    ) -> StopReason {
        let mut dbus = DebugBus::new(bus, &mut self.breakpoints);
        for _ in 0..budget {
            let stop = cpu.step(&mut dbus);
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
