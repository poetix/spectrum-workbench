//! Execution breakpoints, and the three tiers underneath them.
//!
//! The tier tests matter as much as the behaviour tests: the whole design is
//! about what is *not* consulted, and nothing else in the suite would notice
//! if a hash probe crept onto the per-instruction path.

mod common;

use common::{BUDGET, ORG, machine};
use rkw_debug::{Cmp, Condition, Debugger, Operand, StopReason};
use z80::{Reg8, Reg16};

/// A loop that counts B down from 5, writing each value to $9000.
///
/// ```text
/// 8000  06 05     LD B,5
/// 8002  78        loop: LD A,B
/// 8003  32 00 90  LD ($9000),A
/// 8006  10 FA     DJNZ loop
/// 8008  76        HALT
/// ```
const COUNTDOWN: &[u8] = &[0x06, 0x05, 0x78, 0x32, 0x00, 0x90, 0x10, 0xFA, 0x76];
const LOOP_TOP: u16 = ORG + 2;

#[test]
fn a_breakpoint_stops_at_its_address() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(LOOP_TOP);

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { id, addr: LOOP_TOP }
    );
    assert_eq!(cpu.regs.pc, LOOP_TOP);
    assert_eq!(cpu.regs.b, 5, "stopped before the first iteration ran");
}

#[test]
fn resuming_from_a_breakpoint_does_not_hit_it_again_immediately() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    dbg.breakpoints.add_exec(LOOP_TOP);

    let mut stops = 0;
    while let StopReason::Breakpoint { .. } = dbg.resume(&mut cpu, &mut mem, BUDGET) {
        stops += 1;
        assert!(stops <= 5, "the loop only goes round five times");
    }
    assert_eq!(stops, 5, "one stop per iteration, not one per resume");
}

#[test]
fn a_condition_is_consulted_and_counts_only_the_hits_it_allows() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(LOOP_TOP);
    dbg.breakpoints
        .set_condition(id, Some(Condition::reg8_eq(Reg8::B, 2)));

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { id, addr: LOOP_TOP }
    );
    assert_eq!(cpu.regs.b, 2);

    // Four iterations passed the address; one of them fired.
    let bp = dbg.breakpoints.breakpoint_by_id(id).unwrap();
    assert_eq!(bp.hits, 1, "a hit is a firing, not a passing");
}

#[test]
fn an_ignore_count_passes_that_many_hits_first() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(LOOP_TOP);
    dbg.breakpoints.set_ignore(id, 3);

    assert!(matches!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { .. }
    ));
    assert_eq!(cpu.regs.b, 2, "passes 5, 4 and 3, stops on the fourth");

    let bp = dbg.breakpoints.breakpoint_by_id(id).unwrap();
    assert_eq!(bp.hits, 4, "ignored hits are still hits");
    assert_eq!(bp.ignore, 0, "the count is spent");
}

#[test]
fn a_condition_can_read_memory() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(LOOP_TOP);
    // The byte the loop last wrote, rather than the register it came from.
    dbg.breakpoints.set_condition(
        id,
        Some(Condition::cmp(
            Operand::Mem8(0x9000),
            Cmp::Eq,
            Operand::Imm(3),
        )),
    );

    assert!(matches!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { .. }
    ));
    assert_eq!(mem.ram[0x9000], 3, "the iteration that wrote 3 has run");
    assert_eq!(cpu.regs.b, 2, "and the next one is about to");
}

#[test]
fn a_condition_can_read_a_flag() {
    // LD A,$FF ; ADD A,1 ; NOP ; HALT — the add carries, so the NOP is reached
    // with C set exactly once.
    let (mut cpu, mut mem) = machine(&[0x3E, 0xFF, 0xC6, 0x01, 0x00, 0x76]);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(ORG + 4);
    dbg.breakpoints.set_condition(
        id,
        Some(Condition::cmp(
            Operand::Flag(z80::flag::C),
            Cmp::Eq,
            Operand::Imm(1),
        )),
    );

    assert!(matches!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { .. }
    ));
    assert!(cpu.regs.flag(z80::flag::C));
}

#[test]
fn a_disabled_breakpoint_is_disarmed_all_the_way_down() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(LOOP_TOP);
    assert_eq!(dbg.breakpoints.armed_exec_addresses(), 1);

    dbg.breakpoints.set_enabled(id, false);
    assert!(!dbg.breakpoints.exec_armed(), "tier 1 is off");
    assert_eq!(dbg.breakpoints.armed_exec_addresses(), 0, "tier 2 is off");
    assert!(
        dbg.breakpoints.breakpoint_by_id(id).is_some(),
        "tier 3 keeps it, so re-enabling keeps its hit count"
    );

    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
}

#[test]
fn removing_a_breakpoint_disarms_its_address() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(LOOP_TOP);
    assert!(dbg.breakpoints.remove(id));
    assert!(!dbg.breakpoints.remove(id), "removing twice says so");

    assert_eq!(dbg.breakpoints.armed_exec_addresses(), 0);
    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
}

#[test]
fn arming_an_address_twice_is_one_breakpoint() {
    let mut dbg = Debugger::new();
    let first = dbg.breakpoints.add_exec(0x8000);
    let again = dbg.breakpoints.add_exec(0x8000);
    assert_eq!(first, again);
    assert_eq!(dbg.breakpoints.breakpoints().len(), 1);
    assert_eq!(dbg.breakpoints.armed_exec_addresses(), 1);
}

#[test]
fn nothing_armed_reaches_no_tier_below_the_first() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();

    assert!(!dbg.breakpoints.exec_armed());
    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
    assert_eq!(dbg.breakpoints.detail_probes(), 0);
}

#[test]
fn armed_but_not_hit_reaches_no_hash_probe() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();
    // Armed at an address the program never executes, and at one it does.
    dbg.breakpoints.add_exec(0x1234);
    dbg.breakpoints.add_exec(0x4321);

    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
    assert_eq!(
        dbg.breakpoints.detail_probes(),
        0,
        "the bitmap answered every instruction on its own"
    );
}

#[test]
fn the_budget_hands_control_back_without_stopping_anything() {
    let (mut cpu, mut mem) = machine(COUNTDOWN);
    let mut dbg = Debugger::new();

    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, 2),
        StopReason::OutOfBudget,
        "two instructions and no further"
    );
    assert_eq!(cpu.regs.pc, ORG + 3, "LD B,5 then LD A,B");
    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
}

#[test]
fn a_halt_that_nothing_can_end_is_a_stop() {
    // HALT with interrupts disabled, which is where a Spectrum program that
    // has finished usually ends up.
    let (mut cpu, mut mem) = machine(&[0x76]);
    let mut dbg = Debugger::new();
    assert_eq!(dbg.resume(&mut cpu, &mut mem, BUDGET), StopReason::Halted);
    assert_eq!(cpu.regs.pc, ORG, "PC stays on the HALT while halted");

    // With interrupts enabled the machine is waiting, not stuck, so the
    // debugger keeps running until the caller's budget says otherwise.
    let (mut cpu, mut mem) = machine(&[0xFB, 0x76]); // EI ; HALT
    let mut dbg = Debugger::new();
    assert_eq!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::OutOfBudget
    );
}

#[test]
fn breakpoints_list_in_address_order() {
    let mut dbg = Debugger::new();
    dbg.breakpoints.add_exec(0x9000);
    dbg.breakpoints.add_exec(0x4000);
    dbg.breakpoints.add_exec(0x8000);

    let addrs: Vec<u16> = dbg
        .breakpoints
        .breakpoints()
        .iter()
        .map(|b| b.addr)
        .collect();
    assert_eq!(addrs, [0x4000, 0x8000, 0x9000]);
}

#[test]
fn clearing_removes_everything_the_user_set() {
    let mut dbg = Debugger::new();
    dbg.breakpoints.add_exec(0x8000);
    dbg.breakpoints.watch_mem(0x9000, true, true);
    dbg.breakpoints.watch_port(0x00FF, 0x00FE, true, true);

    dbg.breakpoints.clear();
    assert!(!dbg.breakpoints.exec_armed());
    assert_eq!(dbg.breakpoints.armed_exec_addresses(), 0);
    assert_eq!(dbg.breakpoints.armed_read_addresses(), 0);
    assert_eq!(dbg.breakpoints.armed_write_addresses(), 0);
    assert_eq!(dbg.breakpoints.armed_ports(), 0);
    assert!(dbg.breakpoints.breakpoints().is_empty());
}

#[test]
fn a_condition_on_a_register_pair_reads_the_pair() {
    // LD HL,$1234 ; LD HL,$4321 ; HALT, with the breakpoint on the second load
    // so the condition sees the first one's result.
    let (mut cpu, mut mem) = machine(&[0x21, 0x34, 0x12, 0x21, 0x21, 0x43, 0x76]);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(ORG + 3);
    dbg.breakpoints
        .set_condition(id, Some(Condition::reg16_eq(Reg16::Hl, 0x1234)));

    assert!(matches!(
        dbg.resume(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { .. }
    ));
    assert_eq!(cpu.regs.hl(), 0x1234);
}
