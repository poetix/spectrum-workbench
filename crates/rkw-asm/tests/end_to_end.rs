//! Source text to bytes to a running Z80.
//!
//! Everything else in this crate checks the assembler against itself or against
//! the disassembler. These tests check it against the CPU: assemble a program,
//! load the bytes where they were assembled for, run them, and look at what the
//! machine did. If an encoding is wrong in a way both halves of this repository
//! agree on, this is what notices.

mod common;

use common::assemble_ok;
use rkw_asm::Assembled;
use z80::{Cpu, FlatMemory};

/// Where the caller "returns to": running stops when the program's final `RET`
/// takes the PC here.
const RETURN_TO: u16 = 0x0000;

struct Machine {
    cpu: Cpu,
    memory: FlatMemory,
}

/// Assemble, load, and run until the program returns.
fn run(source: &str) -> (Machine, Assembled) {
    let (_, assembled) = assemble_ok(source);
    let origin = assembled.image.origin().expect("something was assembled");

    let mut memory = FlatMemory::new();
    memory.load(origin, &assembled.image.to_binary());

    let mut cpu = Cpu::new();
    cpu.regs.pc = origin;
    // A return address on the stack, so the program's own `RET` is what stops
    // the run rather than a step count.
    cpu.regs.sp = 0xFF00;
    memory.ram[0xFF00] = RETURN_TO as u8;
    memory.ram[0xFF01] = (RETURN_TO >> 8) as u8;

    for _ in 0..100_000 {
        if cpu.regs.pc == RETURN_TO {
            return (Machine { cpu, memory }, assembled);
        }
        cpu.step(&mut memory);
    }
    panic!("program did not return within 100,000 instructions");
}

fn peek(machine: &Machine, address: i64) -> u8 {
    machine.memory.ram[address as usize]
}

#[test]
fn a_program_that_adds_up_a_table() {
    // Exercises a forward reference to a label, a constant defined after it is
    // used, a local label, a backward relative jump, and an indirect read.
    let source = "\
        org $8000
start:  ld hl,table
        ld b,count
        xor a
.add:   add a,(hl)
        inc hl
        djnz .add
        ld (total),a
        ret

table:  db 1,2,3,4,5
count   equ 5
total:  db 0
";
    let (machine, mut assembled) = run(source);
    let total = common::symbol(&mut assembled, "total");

    assert_eq!(peek(&machine, total), 15);
    assert_eq!(machine.cpu.regs.a, 15);
    // B counted down to zero, which is how DJNZ left the loop.
    assert_eq!(machine.cpu.regs.b, 0);
}

#[test]
fn a_program_that_copies_a_string() {
    let source = "\
        org $8000
        ld hl,source
        ld de,destination
        ld bc,length
        ldir
        ret

source: db \"HELLO\"
length  equ $-source
destination:
        ds 5
";
    let (machine, mut assembled) = run(source);
    let destination = common::symbol(&mut assembled, "destination");

    let copied: Vec<u8> = (0..5).map(|i| peek(&machine, destination + i)).collect();
    assert_eq!(copied, b"HELLO");
    // `$-source` is how a source measures itself, and it has to come out as 5.
    assert_eq!(common::symbol(&mut assembled, "length"), 5);
}

#[test]
fn indexed_addressing_reaches_a_structure_field() {
    let source = "\
        org $8000
        ld ix,record
        ld a,(ix+colour)
        add a,(ix+size)
        ld (ix+total),a
        ret

colour  equ 0
size    equ 1
total   equ 2
record: db 7,9,0
";
    let (machine, mut assembled) = run(source);
    let record = common::symbol(&mut assembled, "record");

    assert_eq!(peek(&machine, record + 2), 16);
    assert_eq!(machine.cpu.regs.a, 16);
}

#[test]
fn a_conditional_forward_jump_lands_where_it_says() {
    // The forward `JR` is the case the first pass cannot resolve, so this is
    // also a check that the second pass fixed it up rather than leaving zero.
    let source = "\
        org $8000
        ld a,1
        cp 1
        jr z,equal
        ld a,$FF
        ret
equal:  ld a,$42
        ret
";
    let (machine, _) = run(source);
    assert_eq!(machine.cpu.regs.a, 0x42);
}

#[test]
fn a_call_and_a_stack_round_trip() {
    let source = "\
        org $8000
        ld hl,$1234
        push hl
        pop de
        call swap
        ld ($9000),hl
        ret

swap:   ex de,hl
        ret
";
    let (machine, _) = run(source);
    assert_eq!(machine.cpu.regs.hl(), 0x1234);
    assert_eq!(peek(&machine, 0x9000), 0x34);
    assert_eq!(peek(&machine, 0x9001), 0x12);
}
