//! T-state counts for a sample of instructions from every encoding group.
//!
//! These are the published cycle counts for a Z80 with no wait states. They
//! are the contract that lets the Spectrum's contended memory be layered on
//! later: contention is expressed as extra T-states inserted at specific
//! points within a machine cycle, so the machine cycles have to be in the right
//! places and the right lengths first.

use z80::{Cpu, FlatMemory};

/// Assemble `bytes` at 0x8000, run one instruction, return the T-states spent.
fn time(bytes: &[u8]) -> u64 {
    let mut mem = FlatMemory::new();
    mem.load(0x8000, bytes);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    cpu.step(&mut mem);
    mem.t
}

/// As `time`, but seeds registers first — needed for conditional instructions.
fn time_with(bytes: &[u8], setup: impl FnOnce(&mut Cpu)) -> u64 {
    let mut mem = FlatMemory::new();
    mem.load(0x8000, bytes);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    setup(&mut cpu);
    cpu.step(&mut mem);
    mem.t
}

#[test]
fn unprefixed_timings() {
    assert_eq!(time(&[0x00]), 4, "NOP");
    assert_eq!(time(&[0x3E, 0x01]), 7, "LD A,n");
    assert_eq!(time(&[0x21, 0x00, 0x40]), 10, "LD HL,nn");
    assert_eq!(time(&[0x7E]), 7, "LD A,(HL)");
    assert_eq!(time(&[0x77]), 7, "LD (HL),A");
    assert_eq!(time(&[0x36, 0xFF]), 10, "LD (HL),n");
    assert_eq!(time(&[0x34]), 11, "INC (HL)");
    assert_eq!(time(&[0x23]), 6, "INC HL");
    assert_eq!(time(&[0x09]), 11, "ADD HL,BC");
    assert_eq!(time(&[0x32, 0x00, 0x40]), 13, "LD (nn),A");
    assert_eq!(time(&[0x22, 0x00, 0x40]), 16, "LD (nn),HL");
    assert_eq!(time(&[0xC5]), 11, "PUSH BC");
    assert_eq!(time(&[0xC1]), 10, "POP BC");
    assert_eq!(time(&[0xC3, 0x00, 0x90]), 10, "JP nn");
    assert_eq!(time(&[0xCD, 0x00, 0x90]), 17, "CALL nn");
    assert_eq!(time(&[0xC9]), 10, "RET");
    assert_eq!(time(&[0xFF]), 11, "RST 38h");
    assert_eq!(time(&[0xE3]), 19, "EX (SP),HL");
    assert_eq!(time(&[0xDB, 0xFE]), 11, "IN A,(n)");
    assert_eq!(time(&[0xD3, 0xFE]), 11, "OUT (n),A");
}

#[test]
fn conditional_timings_differ_by_outcome() {
    // JR NZ,d: 12 taken, 7 not taken.
    assert_eq!(time_with(&[0x20, 0x02], |c| c.regs.f = 0x00), 12, "JR NZ taken");
    assert_eq!(time_with(&[0x20, 0x02], |c| c.regs.f = 0x40), 7, "JR NZ not taken");

    // DJNZ d: 13 taken, 8 not taken.
    assert_eq!(time_with(&[0x10, 0x02], |c| c.regs.b = 2), 13, "DJNZ taken");
    assert_eq!(time_with(&[0x10, 0x02], |c| c.regs.b = 1), 8, "DJNZ not taken");

    // CALL NZ,nn: 17 taken, 10 not taken.
    assert_eq!(time_with(&[0xC4, 0, 0x90], |c| c.regs.f = 0x00), 17, "CALL NZ taken");
    assert_eq!(time_with(&[0xC4, 0, 0x90], |c| c.regs.f = 0x40), 10, "CALL NZ not taken");

    // RET NZ: 11 taken, 5 not taken.
    assert_eq!(time_with(&[0xC0], |c| c.regs.f = 0x00), 11, "RET NZ taken");
    assert_eq!(time_with(&[0xC0], |c| c.regs.f = 0x40), 5, "RET NZ not taken");
}

#[test]
fn cb_group_timings() {
    assert_eq!(time(&[0xCB, 0x00]), 8, "RLC B");
    assert_eq!(time(&[0xCB, 0x06]), 15, "RLC (HL)");
    assert_eq!(time(&[0xCB, 0x40]), 8, "BIT 0,B");
    assert_eq!(time(&[0xCB, 0x46]), 12, "BIT 0,(HL)");
    assert_eq!(time(&[0xCB, 0x86]), 15, "RES 0,(HL)");
}

#[test]
fn indexed_timings() {
    assert_eq!(time(&[0xDD, 0x21, 0x00, 0x40]), 14, "LD IX,nn");
    assert_eq!(time(&[0xDD, 0x7E, 0x05]), 19, "LD A,(IX+d)");
    assert_eq!(time(&[0xDD, 0x77, 0x05]), 19, "LD (IX+d),A");
    assert_eq!(time(&[0xDD, 0x36, 0x05, 0xFF]), 19, "LD (IX+d),n");
    assert_eq!(time(&[0xDD, 0x34, 0x05]), 23, "INC (IX+d)");
    assert_eq!(time(&[0xDD, 0x09]), 15, "ADD IX,BC");
    assert_eq!(time(&[0xDD, 0xE3]), 23, "EX (SP),IX");
    assert_eq!(time(&[0xDD, 0xE5]), 15, "PUSH IX");
    assert_eq!(time(&[0xDD, 0xCB, 0x05, 0x06]), 23, "RLC (IX+d)");
    assert_eq!(time(&[0xDD, 0xCB, 0x05, 0x46]), 20, "BIT 0,(IX+d)");
}

#[test]
fn ed_group_timings() {
    assert_eq!(time(&[0xED, 0x44]), 8, "NEG");
    assert_eq!(time(&[0xED, 0x42]), 15, "SBC HL,BC");
    assert_eq!(time(&[0xED, 0x4A]), 15, "ADC HL,BC");
    assert_eq!(time(&[0xED, 0x43, 0x00, 0x40]), 20, "LD (nn),BC");
    assert_eq!(time(&[0xED, 0x4B, 0x00, 0x40]), 20, "LD BC,(nn)");
    assert_eq!(time(&[0xED, 0x47]), 9, "LD I,A");
    assert_eq!(time(&[0xED, 0x57]), 9, "LD A,I");
    assert_eq!(time(&[0xED, 0x6F]), 18, "RLD");
    assert_eq!(time(&[0xED, 0x40]), 12, "IN B,(C)");
    assert_eq!(time(&[0xED, 0x41]), 12, "OUT (C),B");
    assert_eq!(time(&[0xED, 0x45]), 14, "RETN");
    assert_eq!(time(&[0xED, 0x56]), 8, "IM 1");
}

#[test]
fn block_instruction_timings() {
    assert_eq!(time(&[0xED, 0xA0]), 16, "LDI");
    assert_eq!(time(&[0xED, 0xA1]), 16, "CPI");
    assert_eq!(time(&[0xED, 0xA2]), 16, "INI");
    assert_eq!(time(&[0xED, 0xA3]), 16, "OUTI");

    // The repeating forms cost five more T-states on every iteration that
    // loops, and the plain amount on the last one.
    assert_eq!(time_with(&[0xED, 0xB0], |c| c.regs.set_bc(2)), 21, "LDIR looping");
    assert_eq!(time_with(&[0xED, 0xB0], |c| c.regs.set_bc(1)), 16, "LDIR last pass");
    assert_eq!(time_with(&[0xED, 0xB2], |c| c.regs.b = 2), 21, "INIR looping");
    assert_eq!(time_with(&[0xED, 0xB2], |c| c.regs.b = 1), 16, "INIR last pass");
}

#[test]
fn halt_costs_four_t_states_per_idle_pass() {
    let mut mem = FlatMemory::new();
    mem.load(0x8000, &[0x76]); // HALT
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;

    cpu.step(&mut mem);
    assert_eq!(mem.t, 4, "the HALT instruction itself");
    assert!(cpu.regs.halted);
    assert_eq!(cpu.regs.pc, 0x8000, "PC stays on the HALT");

    cpu.step(&mut mem);
    assert_eq!(mem.t, 8, "each idle pass re-fetches the HALT");
}
