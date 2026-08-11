//! Memory and port watchpoints.
//!
//! These are checked on the bus rather than between instructions, so the tests
//! care about two things the breakpoint tests do not: that the access is
//! reported with the values either side of it, and that an instruction fetch
//! is not mistaken for a read.

mod common;

use common::{BUDGET, ORG, machine};
use rkw_debug::{Access, Debugger, PortAccess, StopReason};

const TARGET: u16 = 0x9000;

#[test]
fn a_write_watchpoint_reports_the_byte_either_side() {
    // LD A,$2A ; LD ($9000),A ; HALT
    let (mut cpu, mut mem) = machine(&[0x3E, 0x2A, 0x32, 0x00, 0x90, 0x76]);
    mem.ram[TARGET as usize] = 0x11;

    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_mem(TARGET, false, true);

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Watchpoint {
            id,
            addr: TARGET,
            access: Access::Write,
            old: 0x11,
            new: 0x2A,
        }
    );
    assert_eq!(
        cpu.regs.pc,
        ORG + 5,
        "the instruction that wrote finished before the stop"
    );
    assert_eq!(mem.ram[TARGET as usize], 0x2A, "and the write happened");
}

#[test]
fn a_read_watchpoint_reports_the_byte_read() {
    // LD A,($9000) ; HALT
    let (mut cpu, mut mem) = machine(&[0x3A, 0x00, 0x90, 0x76]);
    mem.ram[TARGET as usize] = 0x5C;

    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_mem(TARGET, true, false);

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Watchpoint {
            id,
            addr: TARGET,
            access: Access::Read,
            old: 0x5C,
            new: 0x5C,
        }
    );
}

#[test]
fn a_read_watchpoint_ignores_a_write_and_the_other_way_round() {
    // LD A,($9000) ; LD ($9000),A ; HALT
    let (mut cpu, mut mem) = machine(&[0x3A, 0x00, 0x90, 0x32, 0x00, 0x90, 0x76]);
    let mut dbg = Debugger::new();
    dbg.breakpoints.watch_mem(TARGET, false, true);

    // The read passes; the write stops.
    let stop = dbg.resume(&mut cpu, &mut mem, BUDGET);
    assert!(
        matches!(
            stop,
            StopReason::Watchpoint {
                access: Access::Write,
                ..
            }
        ),
        "stopped on {stop:?}"
    );
    assert_eq!(cpu.regs.pc, ORG + 6);
}

#[test]
fn executing_at_a_watched_address_is_not_a_read() {
    // The program itself sits at ORG, and the watchpoint is on its first byte.
    // Fetching an instruction is execution, and execution is what breakpoints
    // are for.
    let (mut cpu, mut mem) = machine(&[0x00, 0x00, 0x76]);
    let mut dbg = Debugger::new();
    dbg.breakpoints.watch_mem(ORG, true, true);

    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
    assert_eq!(dbg.breakpoints.watchpoint(ORG).unwrap().hits, 0);
}

#[test]
fn a_change_only_watchpoint_ignores_a_write_of_the_same_value() {
    // LD A,$11 ; LD ($9000),A ; LD A,$22 ; LD ($9000),A ; HALT
    let (mut cpu, mut mem) = machine(&[
        0x3E, 0x11, 0x32, 0x00, 0x90, 0x3E, 0x22, 0x32, 0x00, 0x90, 0x76,
    ]);
    mem.ram[TARGET as usize] = 0x11;

    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_mem(TARGET, false, true);
    dbg.breakpoints.edit_watch(id, |w| w.on_change_only = true);

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Watchpoint {
            id,
            addr: TARGET,
            access: Access::Write,
            old: 0x11,
            new: 0x22,
        },
        "the first write stored the value that was already there"
    );
    assert_eq!(cpu.regs.pc, ORG + 10);
}

#[test]
fn one_stop_per_instruction_even_when_it_touches_the_address_twice() {
    // LDIR copying $9000 over itself one byte at a time: each iteration reads
    // and writes the watched address. Stopping happens between instructions,
    // so the first hit is reported and the iteration still completes.
    let (mut cpu, mut mem) = machine(&[
        0x21, 0x00, 0x90, // LD HL,$9000
        0x11, 0x00, 0x90, // LD DE,$9000
        0x01, 0x02, 0x00, // LD BC,2
        0xED, 0xB0, // LDIR
        0x76,
    ]);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_mem(TARGET, true, true);

    let stop = dbg.resume(&mut cpu, &mut mem, BUDGET);
    assert_eq!(
        stop,
        StopReason::Watchpoint {
            id,
            addr: TARGET,
            access: Access::Read,
            old: 0,
            new: 0,
        },
        "the read comes first in the iteration"
    );
    assert_eq!(
        dbg.breakpoints.watchpoint(TARGET).unwrap().hits,
        2,
        "both accesses counted; only the first stopped"
    );
}

#[test]
fn a_disabled_watchpoint_is_disarmed_all_the_way_down() {
    let (mut cpu, mut mem) = machine(&[0x3E, 0x2A, 0x32, 0x00, 0x90, 0x76]);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_mem(TARGET, true, true);
    assert_eq!(dbg.breakpoints.armed_write_addresses(), 1);
    assert_eq!(dbg.breakpoints.armed_read_addresses(), 1);

    dbg.breakpoints.edit_watch(id, |w| w.enabled = false);
    assert_eq!(dbg.breakpoints.armed_write_addresses(), 0);
    assert_eq!(dbg.breakpoints.armed_read_addresses(), 0);

    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
    assert_eq!(dbg.breakpoints.detail_probes(), 0);
}

#[test]
fn nothing_watched_reaches_no_tier_below_the_first() {
    let (mut cpu, mut mem) = machine(&[0x3E, 0x2A, 0x32, 0x00, 0x90, 0x76]);
    let mut dbg = Debugger::new();
    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
    assert_eq!(dbg.breakpoints.detail_probes(), 0);
}

#[test]
fn a_port_watchpoint_matches_by_mask() {
    // LD A,$7F ; OUT ($FE),A ; HALT — the port is A in the high byte and $FE
    // in the low, which is how a Spectrum border write looks.
    let (mut cpu, mut mem) = machine(&[0x3E, 0x7F, 0xD3, 0xFE, 0x76]);
    let mut dbg = Debugger::new();
    // The ULA answers to every port with A0 low, so that is what "port $FE"
    // means.
    let id = dbg.breakpoints.watch_port(0x0001, 0x0000, false, true);
    assert_eq!(
        dbg.breakpoints.armed_ports(),
        0x8000,
        "half of every port address"
    );

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::PortWatchpoint {
            id,
            port: 0x7FFE,
            access: PortAccess::Out,
            value: 0x7F,
        }
    );
}

#[test]
fn a_port_watchpoint_can_watch_one_direction() {
    // IN A,($FE) ; OUT ($FE),A ; HALT, watching only the write.
    let (mut cpu, mut mem) = machine(&[0xDB, 0xFE, 0xD3, 0xFE, 0x76]);
    let mut dbg = Debugger::new();
    dbg.breakpoints.watch_port(0x00FF, 0x00FE, false, true);

    let stop = dbg.resume(&mut cpu, &mut mem, BUDGET);
    assert!(
        matches!(
            stop,
            StopReason::PortWatchpoint {
                access: PortAccess::Out,
                ..
            }
        ),
        "the IN passed, the OUT stopped: {stop:?}"
    );
}

#[test]
fn a_port_watchpoint_reports_the_byte_read() {
    // LD A,$BF ; IN A,($FE) ; HALT — A supplies the high half of the port
    // address before the read replaces it, which is how a Spectrum keyboard
    // row is selected.
    let (mut cpu, mut mem) = machine(&[0x3E, 0xBF, 0xDB, 0xFE, 0x76]);
    mem.port_value = 0x1F;
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_port(0x00FF, 0x00FE, true, false);

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::PortWatchpoint {
            id,
            port: 0xBFFE,
            access: PortAccess::In,
            value: 0x1F,
        },
        "the port is A on the high half, and the value is what came back"
    );
}

#[test]
fn removing_a_port_watchpoint_disarms_its_ports() {
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.watch_port(0x0001, 0x0000, true, true);
    assert_eq!(dbg.breakpoints.armed_ports(), 0x8000);
    assert!(dbg.breakpoints.remove(id));
    assert_eq!(dbg.breakpoints.armed_ports(), 0);
}
