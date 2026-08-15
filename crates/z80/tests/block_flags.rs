//! The flags a block instruction leaves behind when it is going round again.
//!
//! A block instruction that repeats spends an extra machine cycle putting `PC`
//! back, and in 2018 David Banks worked out that the CPU rewrites flags during
//! it: bits 5 and 3 come from the address it is returning to, and the I/O forms
//! recompute `H` and `P/V` as well.
//!
//! # Why these are here as well as in `z80test`
//!
//! raxoft's suite reaches this through `LDIR->NOP'` and `INIR->NOP'`, which
//! arrange for the instruction to overwrite its own second byte so that the
//! repeat turns it into a `NOP` — leaving the repeating iteration's flags as
//! the last thing written. That covers `LDIR`, `LDDR`, `INIR` and `INDR`.
//!
//! It does not cover `CPIR`, `CPDR`, `OTIR` or `OTDR`, and cannot: those are
//! tested only in forms that run to completion, and the final iteration is
//! never the repeating one, so whatever the repeats wrote is overwritten before
//! anything looks. Those four are implemented on the authority of the reference
//! alone, and these tests are what pins them — so that a later change has
//! something to fail against rather than four silently untested branches.

use z80::{Cpu, FlatMemory, Stop, flag};

/// Run one step of an instruction at `addr`, in a machine with `PC` set there.
fn step_at(
    addr: u16,
    code: &[u8],
    setup: impl FnOnce(&mut Cpu, &mut FlatMemory),
) -> (Cpu, FlatMemory) {
    let mut mem = FlatMemory::new();
    mem.load(addr, code);
    let mut cpu = Cpu::new();
    cpu.regs.pc = addr;
    setup(&mut cpu, &mut mem);
    assert_eq!(cpu.step(&mut mem), Stop::Instruction);
    (cpu, mem)
}

/// Bits 5 and 3 of a byte, which is where the repeat rule puts `PCH`'s.
fn xy(value: u8) -> u8 {
    value & flag::XY
}

#[test]
fn a_repeating_ldir_takes_its_undocumented_bits_from_the_address_it_goes_back_to() {
    // The instruction sits at 0x2800, so PCH is 0x28 — bits 5 and 3 both set,
    // which is a value no byte moved by this copy could produce, so the test
    // cannot pass by accident.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xB0], |cpu, mem| {
        mem.load(0x9000, &[0x00]);
        cpu.regs.a = 0x00;
        cpu.regs.set_hl(0x9000);
        cpu.regs.set_de(0xA000);
        cpu.regs.set_bc(2); // one left after this, so it repeats
    });
    assert_eq!(cpu.regs.pc, 0x2800, "back on the ED");
    assert_eq!(xy(cpu.regs.f), 0x28);
    assert!(cpu.regs.flag(flag::P), "P/V is set while BC is not zero");
    assert!(!cpu.regs.flag(flag::H));
    assert!(!cpu.regs.flag(flag::N));
    assert_eq!(cpu.regs.wz, 0x2801, "MEMPTR is the repeated PC plus one");
    assert_eq!(cpu.regs.q, cpu.regs.f);

    // The last iteration is an ordinary LDI and takes them from A + the byte.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xB0], |cpu, mem| {
        mem.load(0x9000, &[0x00]);
        cpu.regs.a = 0x08; // A + value = 0x08: bit 3 set, bit 1 clear
        cpu.regs.set_hl(0x9000);
        cpu.regs.set_de(0xA000);
        cpu.regs.set_bc(1);
    });
    assert_eq!(cpu.regs.pc, 0x2802, "past the instruction");
    assert_eq!(xy(cpu.regs.f), flag::X);
    assert!(!cpu.regs.flag(flag::P), "BC reached zero");
}

#[test]
fn a_repeating_lddr_does_the_same() {
    let (cpu, _) = step_at(0x2000, &[0xED, 0xB8], |cpu, mem| {
        mem.load(0x9000, &[0xFF]);
        cpu.regs.a = 0xFF;
        cpu.regs.set_hl(0x9000);
        cpu.regs.set_de(0xA000);
        cpu.regs.set_bc(2);
    });
    assert_eq!(cpu.regs.pc, 0x2000);
    assert_eq!(xy(cpu.regs.f), 0x20, "PCH is 0x20: bit 5 set, bit 3 clear");
    assert_eq!(cpu.regs.wz, 0x2001);
}

#[test]
fn a_repeating_cpir_takes_them_from_pch_too() {
    // Not covered by z80test, which only runs CPIR to completion. CPIR repeats
    // while BC is non-zero *and* the compare did not match.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xB1], |cpu, mem| {
        mem.load(0x9000, &[0x01]);
        cpu.regs.a = 0x40; // no match
        cpu.regs.set_hl(0x9000);
        cpu.regs.set_bc(2);
    });
    assert_eq!(cpu.regs.pc, 0x2800);
    assert_eq!(xy(cpu.regs.f), 0x28);
    assert!(cpu.regs.flag(flag::N), "CP leaves N set");
    assert!(!cpu.regs.flag(flag::Z));
    assert_eq!(cpu.regs.wz, 0x2801);

    // A match stops it, and then the bits come from the subtraction as usual.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xB1], |cpu, mem| {
        mem.load(0x9000, &[0x40]);
        cpu.regs.a = 0x40; // match
        cpu.regs.set_hl(0x9000);
        cpu.regs.set_bc(2);
    });
    assert_eq!(cpu.regs.pc, 0x2802);
    assert!(cpu.regs.flag(flag::Z));
    assert_eq!(xy(cpu.regs.f), 0, "A - value is zero, so no X or Y");
}

#[test]
fn a_repeating_otir_recomputes_half_carry_and_parity_as_well() {
    // The I/O block instructions do everything LDIR does and then adjust H and
    // P/V from the byte-plus-index sum, with B moved one further in whichever
    // direction bit 7 of the byte says. `OTIR` is not covered by z80test at
    // all; this is the reference's rule, pinned.
    //
    // value 0x00 + L 0x00 does not carry, so H and C stay clear and P/V is the
    // parity of ((sum & 7) ^ B) ^ (B & 7).
    let (cpu, mem) = step_at(0x2800, &[0xED, 0xB3], |cpu, mem| {
        mem.load(0x9000, &[0x00]);
        cpu.regs.set_hl(0x9000);
        cpu.regs.b = 0x03;
        cpu.regs.c = 0xFE;
    });
    assert_eq!(cpu.regs.pc, 0x2800);
    assert_eq!(xy(cpu.regs.f), 0x28, "PCH, not B");
    assert_eq!(cpu.regs.b, 0x02);
    assert!(!cpu.regs.flag(flag::C));
    assert!(!cpu.regs.flag(flag::H));
    assert!(!cpu.regs.flag(flag::N), "bit 7 of the byte is clear");
    assert_eq!(cpu.regs.wz, 0x2801, "MEMPTR is PC + 1 on a repeat");
    assert_eq!(mem.port_writes.last(), Some(&(0x02FE, 0x00)));

    // A byte that carries out of the sum sets C, and then H comes from the low
    // nibble of B rather than from the sum.
    // The index added to the byte is L *after* HL has moved, so this reads
    // 0x90FE and adds the 0xFF that L has become.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xB3], |cpu, mem| {
        mem.load(0x90FE, &[0xFF]);
        cpu.regs.set_hl(0x90FE);
        cpu.regs.b = 0x03;
        cpu.regs.c = 0xFE;
    });
    assert_eq!(cpu.regs.l, 0xFF);
    assert!(cpu.regs.flag(flag::C), "0xFF + L 0xFF carries");
    assert!(cpu.regs.flag(flag::N), "bit 7 of the byte is set");
    assert_eq!(xy(cpu.regs.f), 0x28);
}

#[test]
fn the_non_repeating_forms_are_untouched_by_any_of_this() {
    // OUTI is the same silicon minus the extra machine cycle, and its flags
    // still come from B. A regression here would mean the repeat rule had
    // leaked into the single-shot forms.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xA3], |cpu, mem| {
        mem.load(0x9000, &[0x00]);
        cpu.regs.set_hl(0x9000);
        cpu.regs.b = 0x2A;
        cpu.regs.c = 0xFE;
    });
    assert_eq!(cpu.regs.pc, 0x2802);
    assert_eq!(cpu.regs.b, 0x29);
    assert_eq!(xy(cpu.regs.f), xy(0x29), "X and Y come from B");

    // And LDI, likewise, from A + the byte moved.
    let (cpu, _) = step_at(0x2800, &[0xED, 0xA0], |cpu, mem| {
        mem.load(0x9000, &[0x08]);
        cpu.regs.a = 0x00;
        cpu.regs.set_hl(0x9000);
        cpu.regs.set_de(0xA000);
        cpu.regs.set_bc(5);
    });
    assert_eq!(cpu.regs.pc, 0x2802);
    assert_eq!(xy(cpu.regs.f), flag::X, "A + 0x08 has bit 3 set");
}

#[test]
fn a_repeat_that_runs_to_the_end_leaves_the_last_iterations_flags() {
    // Why z80test needs the self-modifying `->NOP'` trick at all: run OTIR to
    // completion and the repeating iterations' flags are all overwritten by the
    // final one, which is an ordinary OUTI.
    let mut mem = FlatMemory::new();
    mem.load(0x2800, &[0xED, 0xB3]);
    mem.load(0x9000, &[0x11, 0x22, 0x33]);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x2800;
    cpu.regs.set_hl(0x9000);
    cpu.regs.b = 3;
    cpu.regs.c = 0xFE;

    for _ in 0..3 {
        cpu.step(&mut mem);
    }
    assert_eq!(cpu.regs.b, 0);
    assert_eq!(cpu.regs.pc, 0x2802);
    assert!(cpu.regs.flag(flag::Z), "B reached zero");
    assert_eq!(
        xy(cpu.regs.f),
        0,
        "from B, which is zero — nothing of PCH left"
    );
    assert_eq!(mem.port_writes.len(), 3);
}
