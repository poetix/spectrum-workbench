//! [`Request`] to [`Outcome`]: the second of the command layer's three parts,
//! and the only one that touches a machine.
//!
//! Everything here returns data. There is no `String` in an [`Outcome`] that a
//! formatter could have produced from the value beside it, because a DAP
//! adapter answering `variables` or `readMemory` consumes these same results
//! and must not be parsing its own debugger's output to do it (ADR-0016).
//!
//! # Why the session owns the machine
//!
//! A [`Session`] holds the [`Emu`] rather than talking to one on another
//! thread. Movement goes through the command ring all the same — so it is
//! stamped into the replay log like any other input — but the session then
//! drives the slice loop itself and reads the machine directly when it stops.
//!
//! That is the right shape for a REPL, where the person typing is not doing
//! anything else while the machine runs, and it is the wrong one for a DAP
//! adapter, which has to answer `pause` mid-run (ticket 0023). What makes that
//! affordable later is that the split this module is arranged around is
//! request-to-result, not thread-to-thread: an adapter that answers requests
//! from the emulation thread returns the same [`Outcome`] values.
//!
//! # Arming does not go through the ring
//!
//! [`Command`] deliberately carries no breakpoint ids, because ids are minted
//! on the emulation thread and the way back is lossy. The session is on that
//! thread's side of the queue, so it arms directly and returns the id it
//! minted. Nothing is lost from the replay log by doing so: a breakpoint
//! changes where a run stops, and the log records what was done at each stop
//! rather than replaying the decision to stop there.

use std::fmt;

use z80::disasm::{Instruction, Peek, decode};
use z80::{Cpu, Regs};

use super::parse::{Addr, Base, Format, Info, ParseError, Request, Unit, parse};
use crate::breakpoints::{Breakpoint, Breakpoints, Id, PortWatch, Watchpoint};
use crate::command::Command;
use crate::emu::{Config, Emu, Handle, RunState};
use crate::machine::Machine;
use crate::{Debugger, StopReason};

/// How long a `continue` runs before handing control back, in T-states.
///
/// A REPL with no way to interrupt a running machine — there is no signal
/// handling in this crate and no thread to receive one — would otherwise be a
/// REPL that a `JR $` takes away for good. Roughly half a minute of emulated
/// time, which is a fraction of a second of real time, and a script that runs
/// away says so instead of hanging.
pub const DEFAULT_RUN_LIMIT: u64 = 100_000_000;

/// The most items one command will read or disassemble. A mistyped count is a
/// slip, not a request for a megabyte of hex.
pub const MAX_ITEMS: u32 = 4096;

/// Where a movement ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    /// [`StopReason::OutOfBudget`] means the run limit, and nothing else: the
    /// slice loop absorbs its own budget exhaustion.
    pub reason: StopReason,
    pub pc: u16,
    pub t: u64,
    pub instructions: u64,
    /// What will run next, not what just ran.
    pub next: Instruction,
}

/// The registers, and the two counters that say when they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterView {
    pub regs: Regs,
    pub t: u64,
    pub instructions: u64,
}

/// Bytes read for `x`, exactly as they are in memory. How they are grouped and
/// rendered is the formatter's decision, not one baked in here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDump {
    pub addr: u16,
    pub unit: Unit,
    pub format: Format,
    pub bytes: Vec<u8>,
}

/// One NUL-terminated string, or as much of one as the read limit allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringAt {
    pub addr: u16,
    pub bytes: Vec<u8>,
    /// False when the read stopped at [`MAX_STRING`] rather than at a NUL.
    pub terminated: bool,
}

/// How far a string read goes before giving up on finding a NUL.
pub const MAX_STRING: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disassembly {
    pub instructions: Vec<Instruction>,
    /// Where `PC` is, so a front end can mark it. Not necessarily inside the
    /// listing.
    pub pc: u16,
}

/// What was just armed, as stored — so the caller gets the id that was minted
/// and the condition as it was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Armed {
    Breakpoint(Breakpoint),
    Watchpoint(Watchpoint),
    PortWatch(PortWatch),
}

/// Everything armed, which is what `info breakpoints` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArmedList {
    pub breakpoints: Vec<Breakpoint>,
    pub watchpoints: Vec<Watchpoint>,
    pub ports: Vec<PortWatch>,
}

impl ArmedList {
    pub fn is_empty(&self) -> bool {
        self.breakpoints.is_empty() && self.watchpoints.is_empty() && self.ports.is_empty()
    }
}

/// What `trace` saw: the instructions it ran, in order, and where it ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    pub steps: Vec<Instruction>,
    pub end: Stop,
    /// True when something stopped the trace before it had run its count.
    pub interrupted: bool,
}

/// The result of one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Stopped(Stop),
    Registers(RegisterView),
    Memory(MemoryDump),
    Strings(Vec<StringAt>),
    Disassembly(Disassembly),
    Armed(Armed),
    /// Ids that are no longer armed.
    Removed(Vec<Id>),
    /// Ids whose enabled state changed, and what it changed to.
    Enabled {
        ids: Vec<Id>,
        enabled: bool,
    },
    List(ArmedList),
    Trace(Trace),
    Poked {
        addr: u16,
        old: u8,
        new: u8,
    },
    Reset(RegisterView),
    /// A file of commands to run. The executor does no I/O — the driver reads
    /// files, because what a file name means is a property of where the
    /// session is running and not of the debugger.
    Script(String),
    Help(&'static [(&'static str, &'static str)]),
    Quit,
}

/// Why a command could not be carried out. Parsing has already succeeded by
/// this point: these are the failures that need a machine to notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    NoSuchId(Id),
    TooMany {
        asked: u32,
        limit: u32,
    },
    /// The machine has quit and cannot be moved again.
    Exited,
    /// The command ring was full, which needs the emulation thread to be
    /// wedged; it cannot happen while the session drives the loop itself.
    Busy,
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::NoSuchId(id) => write!(f, "no breakpoint or watchpoint {id}"),
            ExecError::TooMany { asked, limit } => {
                write!(f, "{asked} is more than the limit of {limit}")
            }
            ExecError::Exited => write!(f, "the machine has exited"),
            ExecError::Busy => write!(f, "the command queue is full"),
        }
    }
}

impl std::error::Error for ExecError {}

/// A line that did not parse, or a command that did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Parse(ParseError),
    Exec(ExecError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => e.fmt(f),
            Error::Exec(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Error {
        Error::Parse(e)
    }
}

impl From<ExecError> for Error {
    fn from(e: ExecError) -> Error {
        Error::Exec(e)
    }
}

/// A machine, a debugger, and the commands that drive them.
pub struct Session<M: Machine> {
    emu: Emu<M>,
    handle: Handle,
    entry: u16,
    run_limit: Option<u64>,
}

impl<M: Machine> Session<M> {
    /// A session over a machine, starting paused with `PC` as its entry point.
    pub fn new(cpu: Cpu, machine: M, config: Config) -> Session<M> {
        Session::with_debugger(cpu, machine, Debugger::new(), config)
    }

    pub fn with_debugger(cpu: Cpu, machine: M, debugger: Debugger, config: Config) -> Session<M> {
        let entry = cpu.regs.pc;
        let (emu, handle) = Emu::new(cpu, machine, debugger, config);
        Session {
            emu,
            handle,
            entry,
            run_limit: Some(DEFAULT_RUN_LIMIT),
        }
    }

    /// Where `run` restarts from.
    pub fn entry(&self) -> u16 {
        self.entry
    }

    pub fn set_entry(&mut self, entry: u16) {
        self.entry = entry;
    }

    /// T-states one movement may take before handing control back with
    /// [`StopReason::OutOfBudget`]. `None` runs until the machine stops, which
    /// is right for a caller that can interrupt and wrong for one that cannot.
    pub fn set_run_limit(&mut self, limit: Option<u64>) {
        self.run_limit = limit;
    }

    pub fn run_limit(&self) -> Option<u64> {
        self.run_limit
    }

    pub fn emu(&self) -> &Emu<M> {
        &self.emu
    }

    pub fn emu_mut(&mut self) -> &mut Emu<M> {
        &mut self.emu
    }

    pub fn regs(&self) -> &Regs {
        &self.emu.cpu.regs
    }

    pub fn machine(&self) -> &M {
        &self.emu.machine
    }

    pub fn machine_mut(&mut self) -> &mut M {
        &mut self.emu.machine
    }

    /// Parse a line and run it. `Ok(None)` for a blank or comment line.
    ///
    /// This is the whole of what a shell has to call, which is what "the REPL
    /// is a thin shell over the command layer" means in practice.
    pub fn run_line(&mut self, line: &str) -> Result<Option<Outcome>, Error> {
        match parse(line)? {
            None => Ok(None),
            Some(request) => Ok(Some(self.execute(&request)?)),
        }
    }

    pub fn execute(&mut self, request: &Request) -> Result<Outcome, ExecError> {
        match request {
            Request::Break { addr, condition } => {
                let addr = self.resolve(*addr);
                let breaks = &mut self.emu.debugger.breakpoints;
                let id = breaks.add_exec(addr);
                breaks.set_condition(id, condition.clone());
                let bp = breaks.breakpoint(addr).expect("just armed").clone();
                Ok(Outcome::Armed(Armed::Breakpoint(bp)))
            }
            Request::Watch { addr, read, write } => {
                let addr = self.resolve(*addr);
                let breaks = &mut self.emu.debugger.breakpoints;
                breaks.watch_mem(addr, *read, *write);
                let w = breaks.watchpoint(addr).expect("just armed").clone();
                Ok(Outcome::Armed(Armed::Watchpoint(w)))
            }
            Request::PortWatch {
                value,
                mask,
                on_in,
                on_out,
            } => {
                let breaks = &mut self.emu.debugger.breakpoints;
                let id = breaks.watch_port(*mask, *value, *on_in, *on_out);
                let p = breaks
                    .port_watchpoints()
                    .iter()
                    .find(|p| p.id == id)
                    .expect("just armed")
                    .clone();
                Ok(Outcome::Armed(Armed::PortWatch(p)))
            }
            Request::Delete(ids) => self.delete(ids),
            Request::Enable(ids) => self.set_enabled(ids, true),
            Request::Disable(ids) => self.set_enabled(ids, false),
            Request::Step(n) => self.repeat(Command::Step, *n),
            Request::Next(n) => self.repeat(Command::StepOver, *n),
            Request::Finish => Ok(Outcome::Stopped(self.movement(Command::StepOut)?)),
            Request::Continue => Ok(Outcome::Stopped(self.movement(Command::Resume)?)),
            Request::Until(addr) => {
                let addr = self.resolve(*addr);
                Ok(Outcome::Stopped(self.movement(Command::RunTo(addr))?))
            }
            Request::Run(addr) => {
                if let Some(addr) = addr {
                    self.entry = self.resolve(*addr);
                }
                self.apply(Command::Reset)?;
                self.apply(Command::SetPc(self.entry))?;
                Ok(Outcome::Stopped(self.movement(Command::Resume)?))
            }
            Request::Reset => {
                self.apply(Command::Reset)?;
                Ok(Outcome::Reset(self.registers()))
            }
            Request::Regs | Request::Info(Info::Registers) => {
                Ok(Outcome::Registers(self.registers()))
            }
            Request::Examine {
                addr,
                count,
                format,
                unit,
            } => self.examine(*addr, *count, *format, *unit),
            Request::Disas { addr, count } => self.disassemble(*addr, *count),
            Request::Trace(n) => self.trace(*n),
            Request::Info(Info::Breakpoints) => Ok(Outcome::List(self.armed())),
            Request::Poke { addr, value } => {
                let addr = self.resolve(*addr);
                let old = self.emu.machine.peek(addr);
                self.apply(Command::Poke {
                    addr,
                    value: *value,
                })?;
                Ok(Outcome::Poked {
                    addr,
                    old,
                    new: self.emu.machine.peek(addr),
                })
            }
            Request::Source(path) => Ok(Outcome::Script(path.clone())),
            Request::Help => Ok(Outcome::Help(super::parse::HELP)),
            Request::Quit => {
                self.apply(Command::Quit)?;
                Ok(Outcome::Quit)
            }
        }
    }

    // ---- Arming ----------------------------------------------------------

    fn delete(&mut self, ids: &[Id]) -> Result<Outcome, ExecError> {
        let breaks = &mut self.emu.debugger.breakpoints;
        if ids.is_empty() {
            let all = every_id(breaks);
            breaks.clear();
            return Ok(Outcome::Removed(all));
        }
        // Checked before anything is removed, so a list with one bad id in it
        // leaves the session as it was rather than half done.
        if let Some(id) = ids.iter().find(|id| !armed_id(breaks, **id)) {
            return Err(ExecError::NoSuchId(*id));
        }
        for id in ids {
            breaks.remove(*id);
        }
        Ok(Outcome::Removed(ids.to_vec()))
    }

    fn set_enabled(&mut self, ids: &[Id], enabled: bool) -> Result<Outcome, ExecError> {
        let breaks = &mut self.emu.debugger.breakpoints;
        let ids = if ids.is_empty() {
            every_id(breaks)
        } else {
            ids.to_vec()
        };
        if let Some(id) = ids.iter().find(|id| !armed_id(breaks, **id)) {
            return Err(ExecError::NoSuchId(*id));
        }
        for id in &ids {
            if breaks.set_enabled(*id, enabled) {
                continue;
            }
            if breaks.edit_watch(*id, |w| w.enabled = enabled) {
                continue;
            }
            breaks.edit_port_watch(*id, |p| p.enabled = enabled);
        }
        Ok(Outcome::Enabled { ids, enabled })
    }

    fn armed(&self) -> ArmedList {
        let breaks = &self.emu.debugger.breakpoints;
        ArmedList {
            breakpoints: breaks.breakpoints().into_iter().cloned().collect(),
            watchpoints: breaks.watchpoints().into_iter().cloned().collect(),
            ports: breaks.port_watchpoints().to_vec(),
        }
    }

    // ---- Movement --------------------------------------------------------

    /// Send a command and let the slice loop apply it. Applying is what the
    /// emulation thread does at a control tick; here the session is that
    /// thread, so a send is followed by exactly one slice.
    ///
    /// A machine that has quit is not written to at all — the drain would
    /// happily apply a reset to it, and a session that went on changing a
    /// machine it has said goodbye to is a session with two answers to what
    /// state it is in.
    fn apply(&mut self, command: Command) -> Result<(), ExecError> {
        if command != Command::Quit && self.emu.state() == RunState::Exited {
            return Err(ExecError::Exited);
        }
        self.handle.send(command).map_err(|_| ExecError::Busy)?;
        self.emu.slice();
        Ok(())
    }

    /// Apply a movement and slice until the machine stops.
    fn movement(&mut self, command: Command) -> Result<Stop, ExecError> {
        if self.emu.state() == RunState::Exited {
            return Err(ExecError::Exited);
        }
        self.handle.send(command).map_err(|_| ExecError::Busy)?;
        let started_at = self.emu.machine.t_states();
        loop {
            match self.emu.slice() {
                RunState::Paused => break,
                RunState::Exited => return Err(ExecError::Exited),
                RunState::Running => {
                    if let Some(limit) = self.run_limit
                        && self.emu.machine.t_states() - started_at >= limit
                    {
                        self.apply(Command::Pause)?;
                        return Ok(self.stop(StopReason::OutOfBudget));
                    }
                }
            }
        }
        let reason = self.emu.stop_reason().unwrap_or(StopReason::Paused);
        Ok(self.stop(reason))
    }

    /// `step 5` is five steps, and stops early if one of them lands on
    /// something: a breakpoint reached on the third of five steps is news.
    fn repeat(&mut self, command: Command, times: u32) -> Result<Outcome, ExecError> {
        let mut stop = self.movement(command)?;
        for _ in 1..times {
            if !stop.reason.is_requested() || stop.reason == StopReason::OutOfBudget {
                break;
            }
            if let Some(reason) = self.arrived_at_breakpoint() {
                stop = self.stop(reason);
                break;
            }
            stop = self.movement(command)?;
        }
        Ok(Outcome::Stopped(stop))
    }

    /// Whether the address just stepped to is one the user asked to be told
    /// about.
    ///
    /// A single step does not ask this: [`Debugger::step`] will not stop a
    /// movement that has already finished, because a breakpoint at the address
    /// you deliberately stepped to has nothing to tell you. A *count* is a
    /// different question — the remaining steps were asked for in ignorance of
    /// where they were going — so it is asked here, between iterations, and
    /// only when there are iterations left to abandon.
    ///
    /// The breakpoint fires properly when it fires: its condition is evaluated
    /// and its hit count advances, exactly as it would under `continue`.
    fn arrived_at_breakpoint(&mut self) -> Option<StopReason> {
        let pc = self.emu.cpu.regs.pc;
        if !self.emu.debugger.breakpoints.exec_maybe(pc) {
            return None;
        }
        let id =
            self.emu
                .debugger
                .breakpoints
                .fired_exec(pc, &self.emu.cpu.regs, &self.emu.machine)?;
        Some(StopReason::Breakpoint { id, addr: pc })
    }

    fn stop(&self, reason: StopReason) -> Stop {
        let pc = self.emu.cpu.regs.pc;
        Stop {
            reason,
            pc,
            t: self.emu.machine.t_states(),
            instructions: self.emu.cpu.instructions,
            next: Instruction::render(&self.emu.machine, &decode(&self.emu.machine, pc)),
        }
    }

    // ---- Looking ---------------------------------------------------------

    fn registers(&self) -> RegisterView {
        RegisterView {
            regs: self.emu.cpu.regs,
            t: self.emu.machine.t_states(),
            instructions: self.emu.cpu.instructions,
        }
    }

    fn examine(
        &mut self,
        addr: Addr,
        count: u32,
        format: Format,
        unit: Unit,
    ) -> Result<Outcome, ExecError> {
        if count > MAX_ITEMS {
            return Err(ExecError::TooMany {
                asked: count,
                limit: MAX_ITEMS,
            });
        }
        let addr = self.resolve(addr);
        if format == Format::Str {
            return Ok(Outcome::Strings(self.strings(addr, count)));
        }
        let len = count as usize * unit.width();
        let bytes = (0..len)
            .map(|i| self.emu.machine.peek(addr.wrapping_add(i as u16)))
            .collect();
        Ok(Outcome::Memory(MemoryDump {
            addr,
            unit,
            format,
            bytes,
        }))
    }

    fn strings(&self, addr: u16, count: u32) -> Vec<StringAt> {
        let mut at = addr;
        let mut strings = Vec::new();
        for _ in 0..count {
            let mut bytes = Vec::new();
            let mut terminated = false;
            while bytes.len() < MAX_STRING {
                let b = self.emu.machine.peek(at.wrapping_add(bytes.len() as u16));
                if b == 0 {
                    terminated = true;
                    break;
                }
                bytes.push(b);
            }
            let len = bytes.len() as u16 + u16::from(terminated);
            strings.push(StringAt {
                addr: at,
                bytes,
                terminated,
            });
            at = at.wrapping_add(len);
        }
        strings
    }

    /// Disassemble `count` instructions, and when no address was given a few
    /// before `PC` as well: the question at a breakpoint is usually "how did
    /// this get here", and the answer is above the line.
    fn disassemble(&mut self, addr: Option<Addr>, count: u32) -> Result<Outcome, ExecError> {
        if count > MAX_ITEMS {
            return Err(ExecError::TooMany {
                asked: count,
                limit: MAX_ITEMS,
            });
        }
        let pc = self.emu.cpu.regs.pc;
        let (start, count) = match addr {
            Some(addr) => (self.resolve(addr), count),
            None => (
                back_up(&self.emu.machine, pc, CONTEXT_BEFORE),
                count + CONTEXT_BEFORE,
            ),
        };
        let mut at = start;
        let mut instructions = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let d = decode(&self.emu.machine, at);
            instructions.push(Instruction::render(&self.emu.machine, &d));
            at = d.next_addr();
        }
        Ok(Outcome::Disassembly(Disassembly { instructions, pc }))
    }

    fn trace(&mut self, count: u32) -> Result<Outcome, ExecError> {
        if count > MAX_ITEMS {
            return Err(ExecError::TooMany {
                asked: count,
                limit: MAX_ITEMS,
            });
        }
        let mut steps = Vec::with_capacity(count as usize);
        let mut interrupted = false;
        let mut stop = self.stop(StopReason::Step);
        for i in 0..count {
            // Rendered before the step, because the interesting instruction is
            // the one about to run and after the step it may not be there any
            // more.
            let pc = self.emu.cpu.regs.pc;
            let at_pc = Instruction::render(&self.emu.machine, &decode(&self.emu.machine, pc));
            stop = self.movement(Command::Step)?;
            steps.push(at_pc);
            if !stop.reason.is_requested() || stop.reason == StopReason::OutOfBudget {
                interrupted = i + 1 < count;
                break;
            }
            if i + 1 < count
                && let Some(reason) = self.arrived_at_breakpoint()
            {
                stop = self.stop(reason);
                interrupted = true;
                break;
            }
        }
        Ok(Outcome::Trace(Trace {
            steps,
            end: stop,
            interrupted,
        }))
    }

    fn resolve(&self, addr: Addr) -> u16 {
        let base = match addr.base {
            Base::Abs(a) => a,
            Base::Pc => self.emu.cpu.regs.pc,
            Base::Reg(r) => self.emu.cpu.regs.get16(r),
        };
        base.wrapping_add(addr.offset as u16)
    }
}

/// How many instructions of context `disas` with no address shows before `PC`.
const CONTEXT_BEFORE: u32 = 3;

/// Where to start disassembling so that the listing lands exactly on `addr`
/// after at most `n` instructions.
///
/// Disassembling backwards is not decidable — any byte can be the middle of an
/// instruction — so this is a heuristic and is named one: it takes the
/// earliest start within reach whose decode chain arrives at `addr` without
/// stepping over it. When there is none, which is what a data table before the
/// code looks like, it gives up and starts at `addr`.
fn back_up<P: Peek>(mem: &P, addr: u16, n: u32) -> u16 {
    let reach = (4 * n) as u16;
    if addr < reach {
        return addr;
    }
    for offset in (1..=reach).rev() {
        let start = addr - offset;
        let mut at = start;
        let mut steps = 0;
        while at < addr && steps <= n {
            at = decode(mem, at).next_addr();
            steps += 1;
        }
        if at == addr && steps <= n {
            return start;
        }
    }
    addr
}

fn every_id(breaks: &Breakpoints) -> Vec<Id> {
    let mut ids: Vec<Id> = breaks
        .breakpoints()
        .iter()
        .map(|b| b.id)
        .chain(breaks.watchpoints().iter().map(|w| w.id))
        .chain(breaks.port_watchpoints().iter().map(|p| p.id))
        .collect();
    ids.sort_unstable();
    ids
}

fn armed_id(breaks: &Breakpoints, id: Id) -> bool {
    breaks.breakpoint_by_id(id).is_some()
        || breaks.watchpoints().iter().any(|w| w.id == id)
        || breaks.port_watchpoints().iter().any(|p| p.id == id)
}
