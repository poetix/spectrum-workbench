//! The executor, checked on what it returns rather than on what it prints.
//!
//! Every assertion here is against a structured value. That is the property
//! ADR-0016 asks ticket 0010 for and the one that is expensive to get back
//! later: a test that asserted on formatted text would pass just as well
//! against an executor that only had text to give, and a DAP adapter built on
//! that one would be parsing its own debugger's output.

mod common;

use common::{ORG, STACK};
use rkw_debug::cmd::exec::{Armed, ExecError, Outcome, Session};
use rkw_debug::cmd::parse::{Format, Unit};
use rkw_debug::emu::Config;
use rkw_debug::{Access, StopReason};
use z80::{Cpu, FlatMemory};

/// ```text
/// 8000  3E 2A     LD A,42
/// 8002  CD 10 80  CALL fill
/// 8005  00        NOP
/// 8006  76        HALT
/// 8010  21 00 90  fill:  LD HL,$9000
/// 8013  77               LD (HL),A
/// 8014  C9               RET
/// ```
const PROGRAM: &[u8] = &[0x3E, 0x2A, 0xCD, 0x10, 0x80, 0x00, 0x76];
const FILL: &[u8] = &[0x21, 0x00, 0x90, 0x77, 0xC9];
const TARGET: u16 = 0x9000;

fn session() -> Session<FlatMemory> {
    let mut mem = FlatMemory::new();
    mem.load(ORG, PROGRAM);
    mem.load(ORG + 0x10, FILL);
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    cpu.regs.sp = STACK;
    Session::new(cpu, mem, Config::default())
}

/// Run a line and unwrap what it produced. Panics on anything that did not
/// happen, because a test that swallowed an error would be asserting on the
/// starting state.
fn run(session: &mut Session<FlatMemory>, line: &str) -> Outcome {
    session
        .run_line(line)
        .unwrap_or_else(|e| panic!("{line:?}: {e}"))
        .unwrap_or_else(|| panic!("{line:?} did nothing"))
}

fn stop(session: &mut Session<FlatMemory>, line: &str) -> rkw_debug::cmd::Stop {
    match run(session, line) {
        Outcome::Stopped(stop) => stop,
        other => panic!("{line:?} returned {other:?}"),
    }
}

#[test]
fn a_breakpoint_stops_a_continue_and_says_which_one() {
    let mut s = session();
    let Outcome::Armed(Armed::Breakpoint(bp)) = run(&mut s, "break $8005") else {
        panic!("not a breakpoint");
    };
    assert_eq!((bp.id, bp.addr), (1, 0x8005));

    let stop = stop(&mut s, "continue");
    assert_eq!(
        stop.reason,
        StopReason::Breakpoint {
            id: 1,
            addr: 0x8005
        }
    );
    assert_eq!(stop.pc, 0x8005);
    assert_eq!(stop.next.text, "NOP", "the instruction about to run");
    assert_eq!(s.regs().a, 42, "and the call ran on the way");
    assert_eq!(s.machine().ram[TARGET as usize], 42);
}

#[test]
fn a_conditional_breakpoint_passes_until_the_condition_holds() {
    // B counts down from 3; stop on the last time round.
    let mut mem = FlatMemory::new();
    mem.load(ORG, &[0x06, 0x03, 0x05, 0x20, 0xFD, 0x76]); // LD B,3 ; loop: DEC B ; JR NZ,loop ; HALT
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    let mut s = Session::new(cpu, mem, Config::default());

    run(&mut s, "break $8002 if b == 1");
    let stop = stop(&mut s, "continue");
    assert!(matches!(stop.reason, StopReason::Breakpoint { .. }));
    assert_eq!(s.regs().b, 1);

    let Outcome::List(list) = run(&mut s, "info breakpoints") else {
        panic!("not a list");
    };
    assert_eq!(
        list.breakpoints[0].hits, 1,
        "hits count firings, not passes"
    );
}

#[test]
fn stepping_goes_in_and_next_goes_over() {
    let mut s = session();
    stop(&mut s, "step"); // LD A,42
    assert_eq!(stop(&mut s, "step").pc, ORG + 0x10, "into the call");

    let mut s = session();
    stop(&mut s, "step");
    let stop = stop(&mut s, "next");
    assert_eq!(stop.pc, ORG + 5, "over the call");
    assert_eq!(s.machine().ram[TARGET as usize], 42, "which still ran");
}

#[test]
fn a_step_count_stops_early_when_something_else_does() {
    let mut s = session();
    run(&mut s, "break $8013");
    let stop = stop(&mut s, "step 20");
    assert_eq!(
        stop.reason,
        StopReason::Breakpoint {
            id: 1,
            addr: 0x8013
        },
        "the breakpoint is news, so the rest of the count is abandoned"
    );
}

#[test]
fn finish_runs_to_the_return() {
    let mut s = session();
    stop(&mut s, "step");
    stop(&mut s, "step"); // now inside fill
    let stop = stop(&mut s, "finish");
    assert_eq!(stop.pc, ORG + 5, "back after the CALL");
}

#[test]
fn until_runs_to_an_address_and_run_starts_again() {
    let mut s = session();
    assert_eq!(stop(&mut s, "until $8013").pc, 0x8013);

    let stop = stop(&mut s, "run");
    assert_eq!(
        stop.reason,
        StopReason::Halted,
        "nothing is armed any more, so it runs to the HALT"
    );
    assert_eq!(s.entry(), ORG, "run went back to where PC started");
}

#[test]
fn a_watchpoint_reports_the_write_that_changed_the_byte() {
    let mut s = session();
    let Outcome::Armed(Armed::Watchpoint(w)) = run(&mut s, "watch $9000") else {
        panic!("not a watchpoint");
    };
    assert!(w.on_write && !w.on_read);

    let stop = stop(&mut s, "continue");
    assert_eq!(
        stop.reason,
        StopReason::Watchpoint {
            id: 1,
            addr: TARGET,
            access: Access::Write,
            old: 0,
            new: 42,
        }
    );
}

#[test]
fn reset_and_halt_are_reported_rather_than_run_through() {
    let mut s = session();
    assert_eq!(stop(&mut s, "continue").reason, StopReason::Halted);

    let Outcome::Reset(view) = run(&mut s, "reset") else {
        panic!("not a reset");
    };
    assert_eq!(view.regs.pc, 0);
    assert!(!view.regs.halted);
    assert_eq!(view.regs.a, 42, "a reset is not a power-on");
}

#[test]
fn examine_returns_the_bytes_and_the_shape_asked_for() {
    let mut s = session();
    let Outcome::Memory(dump) = run(&mut s, "x/4 $8000") else {
        panic!("not a dump");
    };
    assert_eq!(dump.addr, ORG);
    assert_eq!(dump.bytes, PROGRAM[..4]);
    assert_eq!((dump.format, dump.unit), (Format::Hex, Unit::Byte));

    let Outcome::Memory(dump) = run(&mut s, "x/2w $8000") else {
        panic!("not a dump");
    };
    assert_eq!(dump.bytes.len(), 4, "two words is four bytes");
}

#[test]
fn examine_through_a_register_reads_where_the_register_points_now() {
    let mut s = session();
    for _ in 0..3 {
        stop(&mut s, "step"); // LD A,42 ; CALL ; LD HL,$9000
    }
    assert_eq!(s.regs().hl(), TARGET);
    let Outcome::Memory(dump) = run(&mut s, "x/1 hl") else {
        panic!("not a dump");
    };
    assert_eq!(dump.addr, TARGET);
}

#[test]
fn examine_as_strings_stops_at_the_nul() {
    let mut s = session();
    s.machine_mut().load(0x9100, b"hi\0there\0");
    let Outcome::Strings(strings) = run(&mut s, "x/2s $9100") else {
        panic!("not strings");
    };
    assert_eq!(strings[0].bytes, b"hi");
    assert!(strings[0].terminated);
    assert_eq!(strings[1].addr, 0x9103, "the next one starts past the NUL");
    assert_eq!(strings[1].bytes, b"there");
}

#[test]
fn disassembly_marks_where_pc_is_and_backs_up_when_it_can() {
    let mut s = session();
    stop(&mut s, "step");
    stop(&mut s, "step"); // inside fill, at $8010

    let Outcome::Disassembly(dis) = run(&mut s, "disas $8000 3") else {
        panic!("not a disassembly");
    };
    assert_eq!(
        dis.instructions
            .iter()
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>(),
        ["LD A,$2A", "CALL $8010", "NOP"]
    );
    assert_eq!(dis.pc, 0x8010, "which is not in this listing, and says so");

    let Outcome::Disassembly(dis) = run(&mut s, "disas") else {
        panic!("not a disassembly");
    };
    assert!(
        dis.instructions[0].addr < dis.pc,
        "with no address it shows what led here"
    );
    assert!(dis.instructions.iter().any(|i| i.addr == dis.pc));
}

#[test]
fn trace_shows_the_instructions_it_ran() {
    let mut s = session();
    let Outcome::Trace(trace) = run(&mut s, "trace 3") else {
        panic!("not a trace");
    };
    assert_eq!(
        trace
            .steps
            .iter()
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>(),
        ["LD A,$2A", "CALL $8010", "LD HL,$9000"]
    );
    assert!(!trace.interrupted);
    assert_eq!(trace.end.pc, 0x8013);
}

#[test]
fn a_trace_that_hits_something_says_it_stopped_early() {
    let mut s = session();
    run(&mut s, "break $8010");
    let Outcome::Trace(trace) = run(&mut s, "trace 10") else {
        panic!("not a trace");
    };
    assert!(trace.interrupted);
    assert_eq!(trace.steps.len(), 2);
    assert!(matches!(
        trace.end.reason,
        StopReason::Breakpoint { addr: 0x8010, .. }
    ));
}

#[test]
fn deleting_takes_ids_and_deleting_nothing_takes_everything() {
    let mut s = session();
    run(&mut s, "break $8005");
    run(&mut s, "watch $9000");
    run(&mut s, "pwatch $FE $00FF");

    let Outcome::List(list) = run(&mut s, "info breakpoints") else {
        panic!("not a list");
    };
    assert_eq!(
        (
            list.breakpoints.len(),
            list.watchpoints.len(),
            list.ports.len()
        ),
        (1, 1, 1)
    );

    assert_eq!(run(&mut s, "delete 2"), Outcome::Removed(vec![2]));
    assert_eq!(run(&mut s, "delete"), Outcome::Removed(vec![1, 3]));
    let Outcome::List(list) = run(&mut s, "info breakpoints") else {
        panic!("not a list");
    };
    assert!(list.is_empty());
}

#[test]
fn a_bad_id_changes_nothing() {
    let mut s = session();
    run(&mut s, "break $8005");
    assert_eq!(
        s.run_line("delete 1 9"),
        Err(rkw_debug::cmd::Error::Exec(ExecError::NoSuchId(9)))
    );
    let Outcome::List(list) = run(&mut s, "info breakpoints") else {
        panic!("not a list");
    };
    assert_eq!(
        list.breakpoints.len(),
        1,
        "the good id survived the bad one"
    );
}

#[test]
fn a_disabled_breakpoint_does_not_stop_anything() {
    let mut s = session();
    run(&mut s, "break $8005");
    assert_eq!(
        run(&mut s, "disable 1"),
        Outcome::Enabled {
            ids: vec![1],
            enabled: false
        }
    );
    assert_eq!(stop(&mut s, "continue").reason, StopReason::Halted);

    run(&mut s, "enable");
    run(&mut s, "run");
    assert!(matches!(
        s.emu().stop_reason(),
        Some(StopReason::Breakpoint { .. })
    ));
}

#[test]
fn poke_writes_behind_the_machines_back() {
    let mut s = session();
    assert_eq!(
        run(&mut s, "poke $9000 $FF"),
        Outcome::Poked {
            addr: TARGET,
            old: 0,
            new: 0xFF
        }
    );
    // Watching the address proves the poke was not seen as a machine write:
    // the watchpoint is armed first and the byte changes without a stop.
    run(&mut s, "watch $9001");
    assert_eq!(
        run(&mut s, "poke $9001 $01"),
        Outcome::Poked {
            addr: 0x9001,
            old: 0,
            new: 1
        }
    );
    assert_eq!(s.emu().stop_reason(), Some(StopReason::Paused));
}

#[test]
fn a_run_that_never_stops_hands_control_back() {
    let mut mem = FlatMemory::new();
    mem.load(ORG, &[0x18, 0xFE]); // JR $ — forever
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    let mut s = Session::new(cpu, mem, Config::default());
    s.set_run_limit(Some(10_000));

    let limited = stop(&mut s, "continue");
    assert_eq!(limited.reason, StopReason::OutOfBudget);
    assert!(limited.t >= 10_000);
    assert_eq!(limited.pc, ORG, "and the machine is where it got to");

    // Still usable afterwards: the limit is a hand-back, not a failure.
    assert_eq!(stop(&mut s, "step").reason, StopReason::Step);
}

#[test]
fn a_count_beyond_the_limit_is_refused_rather_than_served() {
    let mut s = session();
    assert_eq!(
        s.run_line("x/99999 $4000"),
        Err(rkw_debug::cmd::Error::Exec(ExecError::TooMany {
            asked: 99999,
            limit: 4096
        }))
    );
}

#[test]
fn quitting_leaves_the_machine_alone_and_refuses_to_move_it() {
    let mut s = session();
    assert_eq!(run(&mut s, "quit"), Outcome::Quit);
    for line in ["step", "continue", "reset", "poke $9000 $01", "run"] {
        assert_eq!(
            s.run_line(line),
            Err(rkw_debug::cmd::Error::Exec(ExecError::Exited)),
            "{line}"
        );
    }
    assert_eq!(s.regs().pc, ORG, "and nothing ran");
    assert_eq!(s.machine().ram[TARGET as usize], 0, "and nothing changed");
}

#[test]
fn source_is_handed_back_to_whatever_is_driving() {
    let mut s = session();
    assert_eq!(
        run(&mut s, "source boot.rkw"),
        Outcome::Script("boot.rkw".into()),
        "the executor does no I/O"
    );
}

#[test]
fn movement_is_stamped_into_the_replay_log() {
    let mut mem = FlatMemory::new();
    mem.load(ORG, PROGRAM);
    mem.load(ORG + 0x10, FILL);
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    cpu.regs.sp = STACK;
    let mut s = Session::new(
        cpu,
        mem,
        Config {
            log_capacity: 16,
            ..Config::default()
        },
    );

    run(&mut s, "break $8005");
    stop(&mut s, "step");
    run(&mut s, "poke $9000 $01");
    stop(&mut s, "continue");

    let log = s.emu().log();
    assert_eq!(
        log.iter().map(|e| e.command).collect::<Vec<_>>(),
        [
            rkw_debug::Command::Step,
            rkw_debug::Command::Poke {
                addr: 0x9000,
                value: 1
            },
            rkw_debug::Command::Resume,
        ],
        "arming is not an input to the machine, so it is not in the log"
    );
    assert!(log[0].t < log[2].t, "and each is stamped with when it ran");
}
