//! The slice loop and the three channels.
//!
//! What is checked here is the shape of ADR-0007: a slice ends at the earlier
//! of the control tick and the next scheduled hardware event, commands are
//! applied at that boundary rather than when they were sent, a stop is a state
//! transition that cannot be lost, and a movement that takes longer than one
//! slice survives being handed back and forth.

mod common;

use std::time::Duration;

use common::{ORG, machine};
use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu, RunState, SCANLINE, spawn};
use rkw_debug::event::Event;
use rkw_debug::machine::{Clock, Machine};
use rkw_debug::{Debugger, StopReason};
use z80::disasm::Peek;
use z80::{Bus, FlatMemory};

/// A loop with a read, a write and a branch, staying inside 256 bytes of data
/// so it cannot write over itself however long it runs.
///
/// ```text
/// 8000  21 00 90  restart: LD HL,$9000
/// 8003  06 00              LD B,0
/// 8005  7E        loop:    LD A,(HL)
/// 8006  3C                 INC A
/// 8007  77                 LD (HL),A
/// 8008  23                 INC HL
/// 8009  10 FA              DJNZ loop
/// 800B  C3 00 80           JP restart
/// ```
const BUSY: &[u8] = &[
    0x21, 0x00, 0x90, 0x06, 0x00, 0x7E, 0x3C, 0x77, 0x23, 0x10, 0xFA, 0xC3, 0x00, 0x80,
];

/// A call to a subroutine that takes about 680 T-states, which is three
/// scanlines: long enough that stepping over it cannot be one slice.
///
/// ```text
/// 8000  CD 06 80  CALL sub
/// 8003  76        HALT
/// 8006  06 32     sub:   LD B,50
/// 8008  10 FE     delay: DJNZ delay
/// 800A  C9        RET
/// ```
const SLOW_CALL: &[u8] = &[
    0xCD, 0x06, 0x80, 0x76, 0x00, 0x00, 0x06, 0x32, 0x10, 0xFE, 0xC9,
];

const RETURNS_TO: u16 = ORG + 3;
const SUB: u16 = ORG + 6;

/// The longest an instruction in these programs takes, and so the most a slice
/// can overrun its deadline by: instructions are not interruptible.
const OVERRUN: u64 = 24;

fn emu(program: &[u8], config: Config) -> (Emu<FlatMemory>, rkw_debug::Handle) {
    let (cpu, mem) = machine(program);
    Emu::new(cpu, mem, Debugger::new(), config)
}

/// Slice until the machine stops, or give up. A test that hangs says nothing.
fn slice_until_stopped(emu: &mut Emu<FlatMemory>, slices: u32) -> Option<StopReason> {
    for _ in 0..slices {
        if emu.slice() != RunState::Running {
            return emu.stop_reason();
        }
    }
    None
}

#[test]
fn it_starts_paused_and_runs_when_told_to() {
    let (mut emu, mut handle) = emu(BUSY, Config::default());
    assert_eq!(handle.state(), RunState::Paused);

    // A slice while paused runs nothing.
    assert_eq!(emu.slice(), RunState::Paused);
    assert_eq!(emu.machine.t_states(), 0);

    handle.send(Command::Resume).unwrap();
    assert_eq!(emu.slice(), RunState::Running);
    assert!(emu.machine.t_states() >= SCANLINE);
}

#[test]
fn a_slice_ends_at_the_control_tick() {
    let (mut emu, mut handle) = emu(BUSY, Config::default());
    handle.send(Command::Resume).unwrap();

    for slice in 1..=8u64 {
        emu.slice();
        let t = emu.machine.t_states();
        // The deadline is a floor: the instruction that crosses it finishes,
        // and the next slice is measured from where the clock actually got
        // to, so the overrun can accumulate but the machine cannot fall
        // behind.
        assert!(
            t >= slice * SCANLINE && t < slice * (SCANLINE + OVERRUN),
            "slice {slice} ended at {t}"
        );
    }
}

#[test]
fn a_slice_never_runs_past_a_scheduled_event() {
    /// A machine with something on the clock — a frame interrupt, in the
    /// shape ticket 0012 will fill in.
    struct Scheduled {
        mem: FlatMemory,
        period: u64,
        next: u64,
        /// The clock reading at each service, so the test can say not merely
        /// that the event happened but that it happened on time.
        serviced_at: Vec<u64>,
    }

    impl Bus for Scheduled {
        fn read(&mut self, addr: u16) -> u8 {
            self.mem.read(addr)
        }
        fn write(&mut self, addr: u16, value: u8) {
            self.mem.write(addr, value);
        }
        fn input(&mut self, port: u16) -> u8 {
            self.mem.input(port)
        }
        fn output(&mut self, port: u16, value: u8) {
            self.mem.output(port, value);
        }
        fn tick(&mut self, t: u32) {
            self.mem.tick(t);
        }
    }

    impl Peek for Scheduled {
        fn peek(&self, addr: u16) -> u8 {
            self.mem.peek(addr)
        }
    }

    impl Clock for Scheduled {
        fn t_states(&self) -> u64 {
            self.mem.t
        }
    }

    impl Machine for Scheduled {
        fn next_event(&self) -> Option<u64> {
            Some(self.next)
        }
        fn service_event(&mut self) {
            self.serviced_at.push(self.mem.t);
            while self.next <= self.mem.t {
                self.next += self.period;
            }
        }
    }

    let (cpu, mem) = machine(BUSY);
    let scheduled = Scheduled {
        mem,
        period: 300,
        next: 300,
        serviced_at: Vec::new(),
    };
    let (mut emu, mut handle) = Emu::new(
        cpu,
        scheduled,
        Debugger::new(),
        Config {
            // Longer than the period, so the event and not the control tick is
            // what ends each run.
            control_interval: 1000,
            ..Config::default()
        },
    );
    handle.send(Command::Resume).unwrap();
    emu.slice();

    // Three events in the slice, each serviced when it was due rather than at
    // the end of the slice: the loop runs to the earliest deadline, not to the
    // one it would rather have.
    let serviced = emu.machine.serviced_at.clone();
    assert_eq!(serviced.len(), 3, "serviced at {serviced:?}");
    for (n, at) in serviced.iter().enumerate() {
        let due = 300 * (n as u64 + 1);
        assert!(
            (due..due + OVERRUN).contains(at),
            "event {n} serviced at {at}"
        );
    }
    let t = emu.machine.t_states();
    assert!((1000..1000 + OVERRUN).contains(&t), "ran to {t}");
}

#[test]
fn a_breakpoint_stops_the_machine_by_transition_not_by_message() {
    let (mut emu, mut handle) = emu(BUSY, Config::default());
    handle.send(Command::Break(ORG + 0x0B)).unwrap(); // the JP at the end
    handle.send(Command::Resume).unwrap();

    let reason = slice_until_stopped(&mut emu, 100).expect("the breakpoint should have fired");
    assert!(matches!(
        reason,
        StopReason::Breakpoint {
            addr: a,
            ..
        } if a == ORG + 0x0B
    ));
    assert_eq!(handle.state(), RunState::Paused);
    // The same reason is readable from the handle, which never saw an event.
    assert_eq!(handle.stop_reason(), Some(reason));
}

#[test]
fn pause_stops_at_the_next_control_tick() {
    let (mut emu, mut handle) = emu(BUSY, Config::default());
    handle.send(Command::Resume).unwrap();
    emu.slice();
    let paused_at_least = emu.machine.t_states();

    handle.send(Command::Pause).unwrap();
    assert_eq!(emu.slice(), RunState::Paused);
    assert_eq!(handle.stop_reason(), Some(StopReason::Paused));
    // The command was applied at the top of the slice, so nothing ran in it.
    assert_eq!(emu.machine.t_states(), paused_at_least);
}

#[test]
fn a_command_is_applied_at_a_control_tick_and_says_when() {
    let (mut emu, mut handle) = emu(BUSY, Config::default());
    handle.send(Command::Resume).unwrap();
    emu.slice();
    emu.slice();

    handle
        .send(Command::Poke {
            addr: 0x9040,
            value: 0x5A,
        })
        .unwrap();
    emu.slice();

    let applied: Vec<Event> = std::iter::from_fn(|| handle.poll()).collect();
    assert_eq!(handle.dropped_events(), 0);
    // Resume at 0, then the poke at the tick that applied it.
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0], Event::Applied { t: 0, seq: 1 });
    let Event::Applied { t, seq } = applied[1] else {
        panic!("expected an Applied event, got {:?}", applied[1]);
    };
    assert_eq!(seq, 2);
    assert!(
        (2 * SCANLINE..2 * (SCANLINE + OVERRUN)).contains(&t),
        "applied at {t}, which is not the second control tick"
    );
    assert_eq!(emu.machine.ram[0x9040], 0x5A);
}

#[test]
fn a_step_over_spans_as_many_slices_as_it_needs() {
    let (mut emu, mut handle) = emu(SLOW_CALL, Config::default());
    handle.send(Command::StepOver).unwrap();

    // The subroutine is three scanlines long, so the temporary breakpoint has
    // to survive being handed back at the end of each of them.
    assert_eq!(emu.slice(), RunState::Running);
    assert!(emu.cpu.regs.pc >= SUB, "the call was not entered");
    assert_eq!(emu.slice(), RunState::Running);

    let reason = slice_until_stopped(&mut emu, 100).expect("the step-over should have landed");
    assert_eq!(reason, StopReason::Step);
    assert_eq!(emu.cpu.regs.pc, RETURNS_TO);
    assert!(emu.machine.t_states() > 3 * SCANLINE);
}

#[test]
fn a_step_is_one_instruction_and_is_over_when_it_is_applied() {
    let (mut emu, mut handle) = emu(SLOW_CALL, Config::default());
    handle.send(Command::Step).unwrap();

    assert_eq!(emu.slice(), RunState::Paused);
    assert_eq!(emu.stop_reason(), Some(StopReason::Step));
    // One CALL: into the subroutine, not over it.
    assert_eq!(emu.cpu.regs.pc, SUB);
    assert_eq!(emu.machine.t_states(), 17);
}

#[test]
fn arming_and_disarming_go_through_the_same_queue() {
    let (mut emu, mut handle) = emu(BUSY, Config::default());
    handle.send(Command::Break(ORG + 5)).unwrap();
    handle
        .send(Command::Watch {
            addr: 0x9000,
            read: false,
            write: true,
        })
        .unwrap();
    emu.slice();
    assert_eq!(emu.debugger.breakpoints.breakpoints().len(), 1);
    assert_eq!(emu.debugger.breakpoints.watchpoints().len(), 1);

    handle.send(Command::Unbreak(ORG + 5)).unwrap();
    handle.send(Command::Unwatch(0x9000)).unwrap();
    emu.slice();
    assert!(emu.debugger.breakpoints.breakpoints().is_empty());
    assert!(emu.debugger.breakpoints.watchpoints().is_empty());
    assert!(!emu.debugger.breakpoints.exec_armed());

    handle.send(Command::Break(ORG + 5)).unwrap();
    handle.send(Command::ClearAll).unwrap();
    emu.slice();
    assert!(emu.debugger.breakpoints.breakpoints().is_empty());
}

#[test]
fn a_full_command_ring_refuses_rather_than_dropping() {
    let (mut emu, mut handle) = emu(
        BUSY,
        Config {
            command_capacity: 4,
            ..Config::default()
        },
    );
    handle.send(Command::Break(0x1000)).unwrap();
    handle.send(Command::Break(0x2000)).unwrap();
    handle.send(Command::Break(0x3000)).unwrap();
    // The fourth does not fit, and comes back rather than displacing one.
    assert_eq!(
        handle.send(Command::Break(0x4000)),
        Err(rkw_debug::ring::Full(Command::Break(0x4000)))
    );

    emu.slice();
    handle.send(Command::Break(0x4000)).unwrap();
    emu.slice();
    assert_eq!(emu.debugger.breakpoints.breakpoints().len(), 4);
}

#[test]
fn the_event_ring_drops_the_oldest_and_says_how_many() {
    let (mut emu, mut handle) = emu(
        BUSY,
        Config {
            event_capacity: 4,
            command_capacity: 8,
            ..Config::default()
        },
    );
    for addr in 0..5u16 {
        handle.send(Command::Break(addr * 0x100)).unwrap();
    }
    emu.slice();

    // Five events into four slots, of which three are readable: the first
    // two are gone and the reader is told so.
    assert_eq!(handle.poll(), Some(Event::Applied { t: 0, seq: 3 }));
    assert_eq!(handle.dropped_events(), 2);
    assert_eq!(handle.poll(), Some(Event::Applied { t: 0, seq: 4 }));
    assert_eq!(handle.poll(), Some(Event::Applied { t: 0, seq: 5 }));
    assert_eq!(handle.poll(), None);
}

#[test]
fn the_thread_runs_stops_and_hands_the_machine_back() {
    let (cpu, mem) = machine(BUSY);
    let (mut handle, join) = spawn(cpu, mem, Debugger::new(), Config::default());

    handle.send(Command::Break(ORG + 0x0B)).unwrap();
    handle.send(Command::Resume).unwrap();
    let reason = handle
        .wait_for_stop(Duration::from_secs(5))
        .expect("the breakpoint should have stopped the thread");
    assert!(matches!(reason, StopReason::Breakpoint { .. }));
    assert_eq!(handle.state(), RunState::Paused);

    handle.send(Command::Quit).unwrap();
    let emu = join.join().expect("the emulation thread panicked");
    assert_eq!(emu.state(), RunState::Exited);
    assert_eq!(emu.cpu.regs.pc, ORG + 0x0B);
}

#[test]
fn dropping_the_handle_stops_a_parked_thread() {
    let (cpu, mem) = machine(BUSY);
    let (handle, join) = spawn(cpu, mem, Debugger::new(), Config::default());
    drop(handle);
    let emu = join.join().expect("the emulation thread panicked");
    assert_eq!(emu.state(), RunState::Exited);
}

#[test]
fn the_thread_can_be_stopped_while_it_is_running() {
    let (cpu, mem) = machine(BUSY);
    let (mut handle, join) = spawn(cpu, mem, Debugger::new(), Config::default());
    handle.send(Command::Resume).unwrap();
    // Wait until it is actually running, then take it back. The machine cannot
    // stall the UI and the UI cannot stall the machine, so both directions are
    // a store and a drain.
    let start = std::time::Instant::now();
    while handle.state() != RunState::Running {
        assert!(start.elapsed() < Duration::from_secs(5), "never started");
        std::thread::yield_now();
    }
    handle.send(Command::Pause).unwrap();
    assert_eq!(
        handle.wait_for_stop(Duration::from_secs(5)),
        Some(StopReason::Paused)
    );

    handle.send(Command::Quit).unwrap();
    let emu = join.join().expect("the emulation thread panicked");
    assert_eq!(emu.state(), RunState::Exited);
}

#[test]
fn a_halt_nothing_can_end_is_a_stop() {
    // HALT with interrupts disabled at the top of the program.
    let (mut emu, mut handle) = emu(&[0xF3, 0x76], Config::default());
    handle.send(Command::Resume).unwrap();
    assert_eq!(
        slice_until_stopped(&mut emu, 8),
        Some(StopReason::Halted),
        "a HALT with IFF1 clear should stop rather than burn slices"
    );
}

#[test]
fn a_cpu_and_debugger_can_be_driven_without_a_thread() {
    // The whole loop is a struct, so a caller that wants to own the scheduling
    // — a headless test, a batch run, a front end with its own event loop —
    // does not have to spawn anything.
    let (cpu, mem) = machine(BUSY);
    let mut dbg = Debugger::new();
    dbg.breakpoints.add_exec(ORG + 5);
    let (mut emu, mut handle) = Emu::new(cpu, mem, dbg, Config::default());
    handle.send(Command::Resume).unwrap();
    assert!(matches!(
        slice_until_stopped(&mut emu, 4),
        Some(StopReason::Breakpoint { .. })
    ));
}
