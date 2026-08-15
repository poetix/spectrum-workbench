//! Step, step over, step out, run to cursor.
//!
//! The interesting cases are the ones where "the address after the call" is
//! not enough on its own: a conditional call that is not taken, a recursive
//! routine that reaches the same return address at several depths, and a block
//! instruction that is its own loop and so never advances past itself.

mod common;

use common::{BUDGET, ORG, STACK, machine};
use rkw_debug::{Debugger, StopReason};

/// A recursive countdown, which is the shape that makes the stack-pointer
/// guard matter.
///
/// ```text
/// 8000  06 03     LD B,3
/// 8002  CD 10 80  CALL down
/// 8005  76        HALT
/// 8010  05        down: DEC B
/// 8011  C8        RET Z
/// 8012  CD 10 80  CALL down
/// 8015  C9        RET
/// ```
///
/// `8015` is reached three times, at three different stack depths.
fn recursive() -> (z80::Cpu, z80::FlatMemory) {
    let (cpu, mut mem) = machine(&[0x06, 0x03, 0xCD, 0x10, 0x80, 0x76]);
    mem.load(ORG + 0x10, &[0x05, 0xC8, 0xCD, 0x10, 0x80, 0xC9]);
    (cpu, mem)
}

const DOWN: u16 = ORG + 0x10;
const INNER_CALL: u16 = ORG + 0x12;
const AFTER_INNER_CALL: u16 = ORG + 0x15;

#[test]
fn a_step_is_one_instruction() {
    let (mut cpu, mut mem) = machine(&[0x3E, 0x2A, 0x00, 0x76]);
    let mut dbg = Debugger::new();

    assert_eq!(dbg.step(&mut cpu, &mut mem), StopReason::Step);
    assert_eq!(cpu.regs.pc, ORG + 2);
    assert_eq!(cpu.regs.a, 0x2A);
}

#[test]
fn a_step_goes_into_a_call() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();

    dbg.step(&mut cpu, &mut mem); // LD B,3
    dbg.step(&mut cpu, &mut mem); // CALL
    assert_eq!(cpu.regs.pc, DOWN, "a step steps in");
    assert_eq!(cpu.regs.sp, STACK.wrapping_sub(2), "and the call pushed");
}

#[test]
fn a_step_over_treats_a_call_as_one_instruction() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();

    dbg.step(&mut cpu, &mut mem); // LD B,3
    assert_eq!(
        dbg.step_over(&mut cpu, &mut mem, BUDGET),
        StopReason::Step,
        "the whole recursion ran"
    );
    assert_eq!(cpu.regs.pc, ORG + 5);
    assert_eq!(cpu.regs.b, 0, "all three levels ran");
    assert_eq!(cpu.regs.sp, STACK, "the stack came back");
}

/// The one the stack-pointer guard exists for. Stepping over the inner call,
/// two levels down, must not stop when the *deeper* instance returns to the
/// same address.
#[test]
fn a_step_over_does_not_stop_inside_a_recursion() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();

    assert_eq!(
        dbg.run_to(&mut cpu, &mut mem, INNER_CALL, BUDGET),
        StopReason::Step
    );
    let sp_at_call = cpu.regs.sp;
    assert_eq!(cpu.regs.b, 2, "one level down");

    assert_eq!(dbg.step_over(&mut cpu, &mut mem, BUDGET), StopReason::Step);
    assert_eq!(cpu.regs.pc, AFTER_INNER_CALL);
    assert_eq!(
        cpu.regs.sp, sp_at_call,
        "stopped in the frame that stepped, not in the one below it"
    );
    assert_eq!(cpu.regs.b, 0, "the deeper levels all ran");
}

#[test]
fn a_step_over_of_an_untaken_conditional_call_is_just_a_step() {
    // LD A,0 ; OR A (sets Z) ; CALL NZ,$8010 ; HALT
    let (mut cpu, mut mem) = machine(&[0x3E, 0x00, 0xB7, 0xC4, 0x10, 0x80, 0x76]);
    mem.load(ORG + 0x10, &[0x3E, 0x99, 0xC9]); // LD A,$99 ; RET
    let mut dbg = Debugger::new();

    dbg.step(&mut cpu, &mut mem);
    dbg.step(&mut cpu, &mut mem);
    assert_eq!(dbg.step_over(&mut cpu, &mut mem, BUDGET), StopReason::Step);
    assert_eq!(cpu.regs.pc, ORG + 6, "past the call");
    assert_eq!(cpu.regs.a, 0, "which was not taken");
    assert_eq!(cpu.regs.sp, STACK, "and pushed nothing");
}

#[test]
fn a_step_over_of_a_rst_comes_back() {
    // RST $10, with a routine at $0010 that sets A and returns.
    let (mut cpu, mut mem) = machine(&[0xD7, 0x76]);
    mem.load(0x0010, &[0x3E, 0x55, 0xC9]);
    let mut dbg = Debugger::new();

    assert_eq!(dbg.step_over(&mut cpu, &mut mem, BUDGET), StopReason::Step);
    assert_eq!(cpu.regs.pc, ORG + 1);
    assert_eq!(cpu.regs.a, 0x55, "the restart ran");
}

#[test]
fn a_step_over_runs_a_block_instruction_to_completion() {
    // LD HL,$9000 ; LD DE,$9100 ; LD BC,4 ; LDIR ; HALT
    let (mut cpu, mut mem) = machine(&[
        0x21, 0x00, 0x90, 0x11, 0x00, 0x91, 0x01, 0x04, 0x00, 0xED, 0xB0, 0x76,
    ]);
    mem.load(0x9000, &[1, 2, 3, 4]);
    let mut dbg = Debugger::new();

    for _ in 0..3 {
        dbg.step(&mut cpu, &mut mem);
    }
    assert_eq!(cpu.regs.pc, ORG + 9, "sitting on the LDIR");

    // A plain step runs one iteration and leaves PC where it was, which is
    // exactly why step-over has to do something else.
    assert_eq!(dbg.step(&mut cpu, &mut mem), StopReason::Step);
    assert_eq!(cpu.regs.pc, ORG + 9);
    assert_eq!(cpu.regs.bc(), 3);

    assert_eq!(dbg.step_over(&mut cpu, &mut mem, BUDGET), StopReason::Step);
    assert_eq!(cpu.regs.pc, ORG + 11, "past the LDIR");
    assert_eq!(cpu.regs.bc(), 0);
    assert_eq!(&mem.ram[0x9100..0x9104], &[1, 2, 3, 4]);
}

#[test]
fn a_step_out_returns_to_the_caller() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();

    assert_eq!(
        dbg.run_to(&mut cpu, &mut mem, DOWN, BUDGET),
        StopReason::Step
    );
    assert_eq!(cpu.regs.sp, STACK.wrapping_sub(2));

    assert_eq!(dbg.step_out(&mut cpu, &mut mem, BUDGET), StopReason::Step);
    assert_eq!(cpu.regs.pc, ORG + 5, "back in the caller");
    assert_eq!(cpu.regs.sp, STACK);
    assert_eq!(cpu.regs.b, 0, "the whole routine ran");
}

#[test]
fn a_step_out_from_a_nested_frame_returns_one_level() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();

    // Two levels down: the first call, then the inner one.
    dbg.run_to(&mut cpu, &mut mem, INNER_CALL, BUDGET);
    dbg.step(&mut cpu, &mut mem);
    assert_eq!(cpu.regs.pc, DOWN);
    assert_eq!(cpu.regs.sp, STACK.wrapping_sub(4), "two frames deep");

    assert_eq!(dbg.step_out(&mut cpu, &mut mem, BUDGET), StopReason::Step);
    assert_eq!(cpu.regs.pc, AFTER_INNER_CALL, "one level, not all the way");
    assert_eq!(cpu.regs.sp, STACK.wrapping_sub(2));
}

#[test]
fn run_to_a_cursor_inside_a_loop_runs_a_lap() {
    // LD B,3 ; loop: DEC B ; JR NZ,loop ; HALT
    let (mut cpu, mut mem) = machine(&[0x06, 0x03, 0x05, 0x20, 0xFD, 0x76]);
    let mut dbg = Debugger::new();
    let top = ORG + 2;

    assert_eq!(
        dbg.run_to(&mut cpu, &mut mem, top, BUDGET),
        StopReason::Step
    );
    assert_eq!(cpu.regs.b, 3, "arrived at the top for the first time");

    // Already there: running to it again goes round rather than returning at
    // once.
    assert_eq!(
        dbg.run_to(&mut cpu, &mut mem, top, BUDGET),
        StopReason::Step
    );
    assert_eq!(cpu.regs.b, 2);
}

#[test]
fn a_breakpoint_can_interrupt_a_step_over() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(DOWN);

    dbg.step(&mut cpu, &mut mem); // LD B,3
    assert_eq!(
        dbg.step_over(&mut cpu, &mut mem, BUDGET),
        StopReason::Breakpoint { id, addr: DOWN },
        "the user's breakpoint wins over the pending step"
    );

    // And the abandoned step-over leaves nothing armed behind it, so the next
    // resume is not stopped by a landing site nobody is waiting for.
    assert_eq!(dbg.breakpoints.armed_exec_addresses(), 1);
}

#[test]
fn a_step_over_that_runs_out_of_budget_says_so() {
    let (mut cpu, mut mem) = recursive();
    let mut dbg = Debugger::new();

    dbg.step(&mut cpu, &mut mem);
    assert_eq!(
        dbg.step_over(&mut cpu, &mut mem, 3),
        StopReason::OutOfBudget
    );
    assert_eq!(
        dbg.breakpoints.armed_exec_addresses(),
        1,
        "the landing site is still armed, because the step is still pending"
    );
}
