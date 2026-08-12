//! The emulation thread: a slice loop, and the three channels of ADR-0007.
//!
//! The machine runs on its own thread in slices bounded by a T-state deadline,
//! the deadline being the earlier of the next scheduled hardware event and the
//! next control tick. At the top of each slice the inbound command ring is
//! drained; along the way events are pushed to the outbound ring; when
//! something stops the machine the fact is published as a state transition
//! rather than as a message, because a missed breakpoint is a broken debugger
//! and the event ring is allowed to lose things.
//!
//! # Why the loop is a struct rather than a thread body
//!
//! [`Emu`] is the whole machine and the loop that drives it, and [`spawn`] is
//! twelve lines that put it on a thread. Keeping them apart is what makes the
//! interesting properties testable: a test can call [`Emu::slice`] inside a
//! counting allocator and know exactly what ran, or call [`Emu::replay`] to
//! re-apply a recorded command log to a fresh machine and compare the result,
//! neither of which is possible against a thread that owns its own state.
//!
//! # Determinism
//!
//! A command is applied at a control tick, and a control tick is a T-state and
//! not a wall-clock instant. So "the moment a command took effect" is a number
//! the machine agrees with, and a log of commands stamped with those numbers
//! ([`Stamped`]) replays exactly — which is the difference between "it crashed
//! after I poked that byte" and a test. [`Emu::replay`] is that replay, and
//! `tests/replay.rs` is the assertion that it lands in the same place.
//!
//! # Allocation
//!
//! The slice loop does not allocate. The rings are sized at construction, the
//! command log is a `Vec` with its capacity reserved up front and never grown,
//! and an event is sixteen bytes written into a slot that already exists. The
//! exception is arming: a breakpoint command inserts into a `HashMap`, which
//! is allowed to allocate because it happens at the rate a person types. What
//! must not allocate is the running, and `tests/no_alloc.rs` asserts it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

use crossbeam_utils::CachePadded;
use z80::Cpu;

use crate::command::{Command, CommandRx, CommandTx, Stamped};
use crate::event::{Event, EventRx, EventTx};
use crate::machine::Machine;
use crate::ring::{self, Full, Record};
use crate::{Access, Debugger, PortAccess, StopReason};

/// One scanline of a 48K Spectrum, which is how often commands are drained
/// (ADR-0007): 64 µs of control latency, and under 1% of the loop's time.
pub const SCANLINE: u64 = 224;

/// What the emulation thread is doing. Published as a state transition, which
/// is what makes a stop lossless where an event is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RunState {
    Running = 0,
    Paused = 1,
    /// The thread has been told to stop and will hand the machine back when it
    /// is joined.
    Exited = 2,
}

impl RunState {
    fn from_u8(v: u8) -> RunState {
        match v {
            0 => RunState::Running,
            2 => RunState::Exited,
            _ => RunState::Paused,
        }
    }
}

/// The stop channel: a run state and, when it is not `Running`, the reason.
///
/// The two are written in the order a reader needs them — reason first, then
/// the state with a release — so a reader that acquires a non-running state
/// sees the reason that goes with it.
struct Shared {
    state: CachePadded<AtomicU8>,
    reason_lo: CachePadded<AtomicU64>,
    reason_hi: AtomicU64,
    /// Stops published so far. A machine that stopped, was resumed and stopped
    /// again looks identical in the two fields above; this is how a waiter
    /// tells "still the stop I already saw" from "a new one".
    stops: CachePadded<AtomicU64>,
}

impl Shared {
    fn new() -> Shared {
        let [lo, hi] = StopReason::Paused.encode();
        Shared {
            state: CachePadded::new(AtomicU8::new(RunState::Paused as u8)),
            reason_lo: CachePadded::new(AtomicU64::new(lo)),
            reason_hi: AtomicU64::new(hi),
            stops: CachePadded::new(AtomicU64::new(0)),
        }
    }

    fn stops(&self) -> u64 {
        self.stops.load(Ordering::Acquire)
    }

    fn state(&self) -> RunState {
        RunState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, state: RunState) {
        self.state.store(state as u8, Ordering::Release);
    }

    fn set_stop(&self, reason: StopReason, state: RunState) {
        let [lo, hi] = reason.encode();
        self.reason_lo.store(lo, Ordering::Relaxed);
        self.reason_hi.store(hi, Ordering::Relaxed);
        self.set_state(state);
        self.stops
            .store(self.stops.load(Ordering::Relaxed) + 1, Ordering::Release);
    }

    /// The reason, if the machine is not running. `None` while it is: a reason
    /// read mid-run would be the previous stop's, which is worse than nothing.
    fn stop_reason(&self) -> Option<StopReason> {
        if self.state() == RunState::Running {
            return None;
        }
        StopReason::decode([
            self.reason_lo.load(Ordering::Relaxed),
            self.reason_hi.load(Ordering::Relaxed),
        ])
    }
}

const STOP_STEP: u64 = 1;
const STOP_BREAKPOINT: u64 = 2;
const STOP_WATCHPOINT: u64 = 3;
const STOP_PORT: u64 = 4;
const STOP_HALTED: u64 = 5;
const STOP_PAUSED: u64 = 6;
const STOP_OUT_OF_BUDGET: u64 = 7;

impl Record for StopReason {
    /// Kind and id in the first word, the addresses and bytes in the second.
    fn encode(self) -> [u64; 2] {
        match self {
            StopReason::Step => [STOP_STEP, 0],
            StopReason::Halted => [STOP_HALTED, 0],
            StopReason::Paused => [STOP_PAUSED, 0],
            StopReason::OutOfBudget => [STOP_OUT_OF_BUDGET, 0],
            StopReason::Breakpoint { id, addr } => {
                [STOP_BREAKPOINT | u64::from(id) << 8, u64::from(addr)]
            }
            StopReason::Watchpoint {
                id,
                addr,
                access,
                old,
                new,
            } => [
                STOP_WATCHPOINT | u64::from(id) << 8,
                u64::from(addr)
                    | u64::from(access == Access::Write) << 16
                    | u64::from(old) << 24
                    | u64::from(new) << 32,
            ],
            StopReason::PortWatchpoint {
                id,
                port,
                access,
                value,
            } => [
                STOP_PORT | u64::from(id) << 8,
                u64::from(port)
                    | u64::from(access == PortAccess::Out) << 16
                    | u64::from(value) << 24,
            ],
        }
    }

    fn decode(words: [u64; 2]) -> Option<StopReason> {
        let id = (words[0] >> 8) as u32;
        let addr = words[1] as u16;
        let out = words[1] >> 16 & 1 != 0;
        match words[0] & 0xFF {
            STOP_STEP => Some(StopReason::Step),
            STOP_HALTED => Some(StopReason::Halted),
            STOP_PAUSED => Some(StopReason::Paused),
            STOP_OUT_OF_BUDGET => Some(StopReason::OutOfBudget),
            STOP_BREAKPOINT => Some(StopReason::Breakpoint { id, addr }),
            STOP_WATCHPOINT => Some(StopReason::Watchpoint {
                id,
                addr,
                access: if out { Access::Write } else { Access::Read },
                old: (words[1] >> 24) as u8,
                new: (words[1] >> 32) as u8,
            }),
            STOP_PORT => Some(StopReason::PortWatchpoint {
                id,
                port: addr,
                access: if out { PortAccess::Out } else { PortAccess::In },
                value: (words[1] >> 24) as u8,
            }),
            _ => None,
        }
    }
}

/// How the channels are sized and how often control gets a look in.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Records in the outbound event ring. Power of two.
    pub event_capacity: usize,
    /// Records in the inbound command ring. Power of two.
    pub command_capacity: usize,
    /// T-states between command drains. One scanline by default.
    pub control_interval: u64,
    /// How many applied commands to record for replay. Zero records none, and
    /// the buffer never grows, so a long session drops the overflow rather
    /// than allocating on the emulation thread.
    pub log_capacity: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            event_capacity: 4096,
            command_capacity: 256,
            control_interval: SCANLINE,
            log_capacity: 0,
        }
    }
}

/// The machine, the debugger, and the loop that moves them.
pub struct Emu<M: Machine> {
    pub cpu: Cpu,
    pub machine: M,
    pub debugger: Debugger,
    commands: CommandRx,
    events: EventTx,
    shared: Arc<Shared>,
    control_interval: u64,
    log: Vec<Stamped>,
    applied: u32,
}

impl<M: Machine> Emu<M> {
    /// Build the machine and the handle that talks to it. It starts paused,
    /// because a debugger that ran before it was told to would have nowhere to
    /// put the first breakpoint.
    pub fn new(cpu: Cpu, machine: M, debugger: Debugger, config: Config) -> (Emu<M>, Handle) {
        let (command_tx, command_rx) = ring::channel::<Command>(config.command_capacity);
        let (event_tx, event_rx) = ring::channel::<Event>(config.event_capacity);
        let shared = Arc::new(Shared::new());
        let emu = Emu {
            cpu,
            machine,
            debugger,
            commands: command_rx,
            events: event_tx,
            shared: Arc::clone(&shared),
            control_interval: config.control_interval,
            log: Vec::with_capacity(config.log_capacity),
            applied: 0,
        };
        let handle = Handle {
            commands: command_tx,
            events: event_rx,
            shared,
            thread: None,
            stops_seen: 0,
        };
        (emu, handle)
    }

    /// Slices until something says `Exited`. This is the thread body.
    pub fn run(&mut self) {
        loop {
            match self.slice() {
                RunState::Exited => return,
                // Nothing to do until a command arrives, and the sender
                // unparks after pushing one. A spurious wake just runs an
                // empty drain.
                RunState::Paused => std::thread::park(),
                RunState::Running => {}
            }
        }
    }

    /// One slice: drain commands, then run until the earlier of the next
    /// scheduled hardware event and the next control tick. Returns the state
    /// the machine is in afterwards.
    pub fn slice(&mut self) -> RunState {
        self.drain();
        if self.shared.state() != RunState::Running {
            return self.shared.state();
        }
        self.run_to(self.machine.t_states() + self.control_interval);
        self.shared.state()
    }

    /// Run one recorded session again, applying each command at the T-state it
    /// was applied at the first time and stopping when the clock reaches
    /// `until`.
    ///
    /// The machine passed to [`Emu::new`] has to start where the recorded one
    /// did; what this asserts by construction is that nothing else is needed.
    /// A slice boundary is a function of the clock alone, so the replayed run
    /// takes the same instruction boundaries as the recorded one and every
    /// command lands on the same one.
    pub fn replay(&mut self, log: &[Stamped], until: u64) {
        for entry in log {
            self.run_to(entry.t);
            self.apply(entry.command);
        }
        self.run_to(until);
    }

    /// The commands applied so far, with the T-state each was applied at.
    /// Empty unless [`Config::log_capacity`] asked for it.
    pub fn log(&self) -> &[Stamped] {
        &self.log
    }

    pub fn state(&self) -> RunState {
        self.shared.state()
    }

    pub fn stop_reason(&self) -> Option<StopReason> {
        self.shared.stop_reason()
    }

    /// Push an event to the lossy outbound ring. This is how the trace ring
    /// (ticket 0022) and the write journal (ticket 0025) will report; nothing
    /// here waits for a reader.
    pub fn emit(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Run while the machine is running and the clock is short of `target`,
    /// servicing hardware events as they come due.
    fn run_to(&mut self, target: u64) {
        while self.shared.state() == RunState::Running && self.machine.t_states() < target {
            let deadline = self.deadline(target);
            let reason = self
                .debugger
                .run_until(&mut self.cpu, &mut self.machine, deadline);
            if reason == StopReason::OutOfBudget {
                self.service_due();
            } else {
                self.shared.set_stop(reason, RunState::Paused);
            }
        }
    }

    /// The earlier of the caller's deadline and the machine's next scheduled
    /// event.
    fn deadline(&self, target: u64) -> u64 {
        match self.machine.next_event() {
            Some(event) if event < target => event,
            _ => target,
        }
    }

    fn service_due(&mut self) {
        if let Some(event) = self.machine.next_event()
            && self.machine.t_states() >= event
        {
            self.machine.service_event();
        }
    }

    /// Apply everything waiting, at the T-state the machine has reached.
    fn drain(&mut self) {
        while let Some(command) = self.commands.pop() {
            self.apply(command);
            if self.shared.state() == RunState::Exited {
                return;
            }
        }
    }

    /// Apply one command, stamp it, and say so.
    ///
    /// Movement commands arm rather than run: a step-over sets its temporary
    /// breakpoint and puts the machine back in `Running`, and the slice loop
    /// carries it out over as many slices as it takes. The exception is a
    /// single step, which is one instruction and is over before this returns.
    fn apply(&mut self, command: Command) {
        let t = self.machine.t_states();
        match command {
            Command::Resume => {
                self.debugger.breakpoints.clear_temporaries();
                self.shared.set_state(RunState::Running);
            }
            Command::Pause => self.shared.set_stop(StopReason::Paused, RunState::Paused),
            Command::Step => {
                let reason = self.debugger.step(&mut self.cpu, &mut self.machine);
                self.shared.set_stop(reason, RunState::Paused);
            }
            Command::StepOver => {
                match self
                    .debugger
                    .begin_step_over(&mut self.cpu, &mut self.machine)
                {
                    Some(reason) => self.shared.set_stop(reason, RunState::Paused),
                    None => self.shared.set_state(RunState::Running),
                }
            }
            Command::StepOut => {
                self.debugger
                    .begin_step_out(&mut self.cpu, &mut self.machine);
                self.shared.set_state(RunState::Running);
            }
            Command::RunTo(addr) => {
                self.debugger.begin_run_to(addr);
                self.shared.set_state(RunState::Running);
            }
            Command::Break(addr) => {
                self.debugger.breakpoints.add_exec(addr);
            }
            Command::Unbreak(addr) => {
                if let Some(id) = self.debugger.breakpoints.breakpoint(addr).map(|b| b.id) {
                    self.debugger.breakpoints.remove(id);
                }
            }
            Command::Watch { addr, read, write } => {
                self.debugger.breakpoints.watch_mem(addr, read, write);
            }
            Command::Unwatch(addr) => {
                if let Some(id) = self.debugger.breakpoints.watchpoint(addr).map(|w| w.id) {
                    self.debugger.breakpoints.remove(id);
                }
            }
            Command::ClearAll => self.debugger.breakpoints.clear(),
            // The raw accessor, not a machine cycle: a poke is the debugger
            // writing, and nothing in the machine should be able to tell.
            Command::Poke { addr, value } => self.machine.write(addr, value),
            Command::Reset => self.cpu.reset(),
            Command::SetPc(addr) => self.cpu.regs.pc = addr,
            Command::Quit => self.shared.set_stop(StopReason::Paused, RunState::Exited),
        }
        self.applied += 1;
        // Within capacity, so this cannot grow and cannot allocate. A log that
        // filled up stops recording rather than becoming the thing that
        // breaks the rule it exists to check.
        if self.log.len() < self.log.capacity() {
            self.log.push(Stamped { t, command });
        }
        self.events.push(Event::Applied {
            t,
            seq: self.applied,
        });
    }
}

/// The other end: send commands, read events, see what the machine is doing.
///
/// One of these, because the rings are single-producer single-consumer. A UI
/// that wants two senders puts this behind its own lock, which is fine at
/// control rate and is exactly what must not happen on the emulation thread.
pub struct Handle {
    commands: CommandTx,
    events: EventRx,
    shared: Arc<Shared>,
    thread: Option<Thread>,
    /// The stop count this end has already been told about, so
    /// [`Handle::wait_for_stop`] waits for a new one.
    stops_seen: u64,
}

impl Handle {
    /// Queue a command and wake the thread if it is parked.
    ///
    /// Fails only if the command ring is full, which means the emulation
    /// thread has not reached a control tick in the time it took to send a
    /// ring's worth of commands. Nothing is dropped: the command comes back
    /// for the caller to retry.
    pub fn send(&mut self, command: Command) -> Result<(), Full<Command>> {
        self.commands.try_push(command)?;
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
        Ok(())
    }

    /// The next event, or `None` if there are none waiting.
    pub fn poll(&mut self) -> Option<Event> {
        self.events.pop()
    }

    /// Events overwritten before this end got to them. Any report built from
    /// polled events has to say this number out loud.
    pub fn dropped_events(&self) -> u64 {
        self.events.dropped()
    }

    pub fn state(&self) -> RunState {
        self.shared.state()
    }

    /// Why the machine stopped, or `None` while it is running.
    pub fn stop_reason(&self) -> Option<StopReason> {
        self.shared.stop_reason()
    }

    /// Block until the machine stops again, or the timeout expires.
    ///
    /// "Again" is the whole point: a machine that is already paused is the
    /// normal state of a debugger, so a wait that returned on that would
    /// return immediately after every `Resume` and report the stop before it.
    /// Each call therefore waits for a stop this handle has not already been
    /// told about.
    ///
    /// A convenience for a caller with nothing else to do — a test, or a REPL
    /// waiting on a step. It spins rather than sleeping on a condition
    /// variable because the emulation thread must not be made to signal one:
    /// the point of publishing a stop as a state transition is that it costs a
    /// store.
    pub fn wait_for_stop(&mut self, timeout: Duration) -> Option<StopReason> {
        let deadline = Instant::now() + timeout;
        loop {
            let stops = self.shared.stops();
            if stops != self.stops_seen {
                self.stops_seen = stops;
                return self.shared.stop_reason();
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::yield_now();
        }
    }
}

impl Drop for Handle {
    /// A parked thread with nothing left to wake it is a leak, so dropping the
    /// handle asks it to quit. Best effort: if the ring is full the thread has
    /// commands to get through anyway, and a caller that cares sends
    /// [`Command::Quit`] itself and joins.
    fn drop(&mut self) {
        let _ = self.commands.try_push(Command::Quit);
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
    }
}

/// Put a machine on its own thread. The join handle gives the [`Emu`] back —
/// machine, CPU, debugger and command log — once it has quit.
pub fn spawn<M: Machine + Send + 'static>(
    cpu: Cpu,
    machine: M,
    debugger: Debugger,
    config: Config,
) -> (Handle, JoinHandle<Emu<M>>) {
    let (mut emu, mut handle) = Emu::new(cpu, machine, debugger, config);
    let join = std::thread::Builder::new()
        .name("emulation".into())
        .spawn(move || {
            emu.run();
            emu
        })
        .expect("spawning the emulation thread");
    handle.thread = Some(join.thread().clone());
    (handle, join)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stop_reason_round_trips_through_the_stop_channel() {
        let reasons = [
            StopReason::Step,
            StopReason::Halted,
            StopReason::Paused,
            StopReason::OutOfBudget,
            StopReason::Breakpoint {
                id: 7,
                addr: 0x8000,
            },
            StopReason::Watchpoint {
                id: u32::MAX,
                addr: 0x4000,
                access: Access::Write,
                old: 0x12,
                new: 0x34,
            },
            StopReason::Watchpoint {
                id: 1,
                addr: 0xFFFF,
                access: Access::Read,
                old: 0xAB,
                new: 0xAB,
            },
            StopReason::PortWatchpoint {
                id: 2,
                port: 0xFEFE,
                access: PortAccess::Out,
                value: 0x0F,
            },
            StopReason::PortWatchpoint {
                id: 3,
                port: 0x00FE,
                access: PortAccess::In,
                value: 0xBF,
            },
        ];
        for reason in reasons {
            assert_eq!(
                StopReason::decode(reason.encode()),
                Some(reason),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn the_shared_state_starts_paused() {
        let shared = Shared::new();
        assert_eq!(shared.state(), RunState::Paused);
        assert_eq!(shared.stop_reason(), Some(StopReason::Paused));
        shared.set_state(RunState::Running);
        assert_eq!(shared.stop_reason(), None);
    }
}
