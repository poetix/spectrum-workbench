//! A recorded command log replays to the same machine.
//!
//! This is the property that is easy to lose and hard to notice losing: apply
//! a command at whatever moment is convenient — when it arrives, at a
//! wall-clock tick, between two instructions of the UI's choosing — and the
//! run stops being a function of its inputs. Stamping each command with the
//! T-state it was applied at, and applying it there again, makes "it crashed
//! after I poked that byte" into a test case.
//!
//! What is compared is the whole machine: registers, all 64K of memory, and
//! the clock. A replay that agreed about the registers and not the memory
//! would be a replay of something else.

mod common;

use common::{ORG, machine};
use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu, RunState};
use rkw_debug::machine::Clock;
use rkw_debug::{Debugger, StopReason};
use z80::{Cpu, FlatMemory};

/// A loop that reads, increments and writes back through 256 bytes of data,
/// so a divergence of one instruction shows up in memory rather than only in
/// the registers.
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

/// Long enough that the program walks its whole 256 bytes several times over,
/// so a command applied one lap out shows up as a different byte rather than
/// as nothing.
const SLICES: u32 = 400;

/// Roughly what one lap of the program costs: 256 bytes at 37 T-states each.
const LAP: u64 = 9472;

fn config() -> Config {
    Config {
        log_capacity: 512,
        ..Config::default()
    }
}

/// Registers, memory and clock: everything a divergence could hide in.
struct Snapshot {
    regs: z80::Regs,
    ram: Box<[u8; 0x1_0000]>,
    t: u64,
}

fn state(cpu: &Cpu, mem: &FlatMemory) -> Snapshot {
    Snapshot {
        regs: cpu.regs,
        ram: mem.ram.clone(),
        t: mem.t,
    }
}

/// Drive a session that pokes, arms, stops and moves, and hand back the log
/// and the state it ended in.
fn record() -> (Vec<rkw_debug::Stamped>, Snapshot) {
    let (cpu, mem) = machine(BUSY);
    let (mut emu, mut handle) = Emu::new(cpu, mem, Debugger::new(), config());

    handle.send(Command::Resume).unwrap();
    for slice in 0..SLICES {
        // A poke every few slices, at addresses the program is walking over,
        // so the machine's own writes and the debugger's interleave.
        if slice % 7 == 3 {
            handle
                .send(Command::Poke {
                    addr: 0x9000 + slice as u16,
                    value: slice as u8,
                })
                .unwrap();
        }
        if slice == 11 {
            handle.send(Command::Break(ORG + 0x0B)).unwrap();
        }
        if slice == 20 {
            handle.send(Command::Pause).unwrap();
        }
        if slice == 22 {
            handle.send(Command::Step).unwrap();
            handle.send(Command::Resume).unwrap();
        }
        // A breakpoint that fires leaves the machine paused; resume it and
        // carry on, which is what a person would do.
        if emu.slice() != RunState::Running {
            handle.send(Command::Resume).unwrap();
        }
    }
    let log = emu.log().to_vec();
    (log, state(&emu.cpu, &emu.machine))
}

#[test]
fn a_recorded_command_log_replays_to_the_same_machine() {
    let (log, recorded) = record();
    assert!(
        log.len() >= 8,
        "the session should have applied a few commands, not {}",
        log.len()
    );

    let (cpu, mem) = machine(BUSY);
    let (mut replayed, _handle) = Emu::new(cpu, mem, Debugger::new(), Config::default());
    replayed.replay(&log, recorded.t);

    let after = state(&replayed.cpu, &replayed.machine);
    assert_eq!(after.regs, recorded.regs, "registers diverged");
    assert_eq!(after.t, recorded.t, "the clock diverged");
    assert!(
        after.ram == recorded.ram,
        "memory diverged at {:?}",
        after
            .ram
            .iter()
            .zip(recorded.ram.iter())
            .position(|(a, b)| a != b)
    );
}

#[test]
fn a_command_applied_at_the_wrong_moment_lands_somewhere_else() {
    // The test above is only worth having if the stamp is what makes it pass.
    // Move one poke a lap later and the machine ends up somewhere else, which
    // is what applying commands "when convenient" would do every time.
    let (log, recorded) = record();
    let mut shifted = log.clone();
    let poke = shifted
        .iter()
        .position(|s| matches!(s.command, Command::Poke { .. }))
        .expect("the session pokes");
    shifted[poke].t += LAP;

    let (cpu, mem) = machine(BUSY);
    let (mut replayed, _handle) = Emu::new(cpu, mem, Debugger::new(), Config::default());
    replayed.replay(&shifted, recorded.t);
    assert!(
        replayed.machine.ram != recorded.ram,
        "a poke a lap out of place should leave a different machine"
    );
}

#[test]
fn every_applied_command_is_stamped_in_order() {
    let (log, _) = record();
    assert!(
        log.windows(2).all(|w| w[0].t <= w[1].t),
        "stamps should be non-decreasing: {log:?}"
    );
    assert_eq!(log[0].command, Command::Resume);
    assert_eq!(log[0].t, 0);
    // Every stamp after the first is a control tick the machine reached, not a
    // moment the sender chose: the machine had to have run to get there.
    assert!(log.iter().skip(1).any(|s| s.t > 0));
}

#[test]
fn a_log_replays_the_stop_it_ended_in() {
    // Arm a breakpoint, run into it, and check the replay stops in the same
    // place rather than running past it.
    let (cpu, mem) = machine(BUSY);
    let (mut emu, mut handle) = Emu::new(cpu, mem, Debugger::new(), config());
    handle.send(Command::Break(ORG + 0x0B)).unwrap();
    handle.send(Command::Resume).unwrap();
    while emu.slice() == RunState::Running {}
    let stopped_at = emu.machine.t_states();
    let reason = emu.stop_reason();
    assert!(matches!(reason, Some(StopReason::Breakpoint { .. })));

    let (cpu, mem) = machine(BUSY);
    let (mut replayed, _handle) = Emu::new(cpu, mem, Debugger::new(), Config::default());
    replayed.replay(emu.log(), stopped_at);
    assert_eq!(replayed.machine.t_states(), stopped_at);
    assert_eq!(replayed.cpu.regs.pc, emu.cpu.regs.pc);
    assert_eq!(replayed.stop_reason(), reason);
}

#[test]
fn the_log_stops_recording_rather_than_growing() {
    // The buffer is sized once, because growing it would allocate on the
    // emulation thread. A session longer than the buffer loses the tail of
    // the log, which is a recording problem and not a correctness one.
    let (cpu, mem) = machine(BUSY);
    let (mut emu, mut handle) = Emu::new(
        cpu,
        mem,
        Debugger::new(),
        Config {
            log_capacity: 4,
            ..Config::default()
        },
    );
    for _ in 0..10 {
        handle.send(Command::Poke { addr: 0, value: 1 }).unwrap();
        emu.slice();
    }
    assert_eq!(emu.log().len(), 4);
}
