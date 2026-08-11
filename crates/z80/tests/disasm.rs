//! Disassembler tests.
//!
//! The important one is [`lengths_agree_with_the_cpu`]: it walks every opcode
//! in every prefix page and checks that the number of bytes the disassembler
//! claims to have consumed is the number the CPU actually consumed. Operand
//! order is where a hand-written disassembler goes wrong — `DD 36 d n` reads
//! its displacement before its immediate, `DD CB d op` reads its displacement
//! before its opcode — and a length mismatch is exactly the symptom.

use z80::disasm::{Flow, disassemble, disassemble_range};
use z80::{Cpu, FlatMemory};

const ORG: u16 = 0x8000;

fn disasm(bytes: &[u8]) -> z80::Instruction {
    let mut mem = FlatMemory::new();
    mem.load(ORG, bytes);
    disassemble(&mem, ORG)
}

fn text(bytes: &[u8]) -> String {
    disasm(bytes).text
}

/// The result of running one instruction: how far PC moved, and where it
/// ended up.
struct Run {
    advance: u16,
    pc: u16,
}

fn run(bytes: &[u8], f: u8, b: u8) -> Run {
    let mut mem = FlatMemory::new();
    mem.load(ORG, bytes);
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    cpu.regs.sp = 0xFF00;
    cpu.regs.f = f;
    cpu.regs.set_bc((u16::from(b) << 8) | 0x01);
    cpu.regs.set_hl(0x4000);
    cpu.regs.set_de(0x4100);
    cpu.regs.ix = 0x4000;
    cpu.regs.iy = 0x4000;
    cpu.step(&mut mem);
    Run {
        advance: cpu.regs.pc.wrapping_sub(ORG),
        pc: cpu.regs.pc,
    }
}

/// No single flag state leaves every condition false — `Z` and `NZ` are
/// complementary — so each instruction is run twice under opposite flags, with
/// `B` chosen so `DJNZ` also falls both ways. Exactly one of the two runs does
/// not branch, and that one advances PC by the encoded length.
fn both_ways(bytes: &[u8]) -> [Run; 2] {
    [run(bytes, 0x00, 1), run(bytes, 0xFF, 2)]
}

/// Every encoding whose length can be observed by running it. Unconditional
/// transfers move PC somewhere unrelated to their length, and the repeating
/// block instructions deliberately do not advance at all; both are covered
/// separately.
fn testable(insn: &z80::Instruction) -> bool {
    match insn.flow {
        Flow::Normal => true,
        Flow::Jump { conditional, .. }
        | Flow::Return { conditional }
        | Flow::Call { conditional, .. } => conditional,
        Flow::Halt | Flow::Rst(_) | Flow::Repeat => false,
    }
}

fn check_encoding(bytes: &[u8], checked: &mut usize) {
    let insn = disasm(bytes);
    if !testable(&insn) {
        return;
    }
    let runs = both_ways(bytes);
    let len = u16::from(insn.len);
    assert!(
        runs.iter().any(|r| r.advance == len),
        "length mismatch for {:02X?}: disassembled as {} ({len} bytes), \
         but the CPU advanced PC by {} and {}",
        &bytes[..insn.len as usize],
        insn.text,
        runs[0].advance,
        runs[1].advance,
    );

    // A conditional branch with a statically known target must actually reach
    // it on the run where the condition holds. This is what catches a `JR`
    // displacement measured from the wrong base.
    if let Flow::Jump {
        target: Some(t),
        conditional: true,
    } = insn.flow
    {
        assert!(
            runs.iter().any(|r| r.pc == t),
            "{} claims target {t:04X}, but the CPU reached {:04X} / {:04X}",
            insn.text,
            runs[0].pc,
            runs[1].pc,
        );
    }

    *checked += 1;
}

#[test]
fn lengths_agree_with_the_cpu() {
    let mut checked = 0usize;

    // Unprefixed, and the CB / ED pages.
    for prefix in [vec![], vec![0xCB], vec![0xED]] {
        for op in 0x00..=0xFFu8 {
            let mut bytes = prefix.clone();
            bytes.push(op);
            bytes.extend_from_slice(&[0x34, 0x12]); // operands for anything that takes them
            check_encoding(&bytes, &mut checked);
        }
    }

    // The DD / FD pages, including their nested CB sub-page.
    for prefix in [0xDDu8, 0xFD] {
        for op in 0x00..=0xFFu8 {
            check_encoding(&[prefix, op, 0x34, 0x12], &mut checked);

            // DD CB d op is always four bytes, and always testable.
            let bytes = [prefix, 0xCB, 0x05, op];
            let insn = disasm(&bytes);
            assert_eq!(insn.len, 4, "every DD CB form is four bytes");
            check_encoding(&bytes, &mut checked);
        }
    }

    assert!(checked > 1200, "only checked {checked} encodings");
}

#[test]
fn block_instruction_lengths() {
    // Each repeating form needs its own counter set to expire on this pass, or
    // it re-executes in place and PC does not move. LDxR and CPxR watch all of
    // BC; INxR and OTxR watch B alone.
    for (op, b) in [
        (0xA0u8, 1u8), // LDI
        (0xA1, 1),     // CPI
        (0xA2, 1),     // INI
        (0xA3, 1),     // OUTI
        (0xA8, 1),     // LDD
        (0xA9, 1),     // CPD
        (0xAA, 1),     // IND
        (0xAB, 1),     // OUTD
        (0xB0, 0),     // LDIR, with BC = 1
        (0xB1, 0),     // CPIR
        (0xB2, 1),     // INIR, with B = 1
        (0xB3, 1),     // OTIR
        (0xB8, 0),     // LDDR
        (0xB9, 0),     // CPDR
        (0xBA, 1),     // INDR
        (0xBB, 1),     // OTDR
    ] {
        let bytes = [0xED, op];
        let insn = disasm(&bytes);
        assert_eq!(insn.len, 2, "{} should be two bytes", insn.text);
        assert_eq!(
            run(&bytes, 0x00, b).advance,
            2,
            "{} did not advance past itself on its final pass",
            insn.text
        );
    }
}

#[test]
fn common_instructions_read_correctly() {
    assert_eq!(text(&[0x00]), "NOP");
    assert_eq!(text(&[0x3E, 0x2A]), "LD A,$2A");
    assert_eq!(text(&[0x21, 0x00, 0x40]), "LD HL,$4000");
    assert_eq!(text(&[0x36, 0xFF]), "LD (HL),$FF");
    assert_eq!(text(&[0x77]), "LD (HL),A");
    assert_eq!(text(&[0x7E]), "LD A,(HL)");
    assert_eq!(text(&[0xC3, 0x00, 0x90]), "JP $9000");
    assert_eq!(text(&[0xCD, 0x00, 0x90]), "CALL $9000");
    assert_eq!(text(&[0xC4, 0x00, 0x90]), "CALL NZ,$9000");
    assert_eq!(text(&[0xEF]), "RST $28");
    assert_eq!(text(&[0xD9]), "EXX");
    assert_eq!(text(&[0x08]), "EX AF,AF'");
    assert_eq!(text(&[0xB8]), "CP B");
    assert_eq!(text(&[0xFE, 0x0D]), "CP $0D");
    assert_eq!(text(&[0x86]), "ADD A,(HL)");
}

#[test]
fn relative_jumps_show_absolute_targets() {
    // JR +2 from 0x8000: the displacement is measured from 0x8002.
    assert_eq!(text(&[0x18, 0x02]), "JR $8004");
    // A backwards branch, the shape of every assembly loop.
    assert_eq!(text(&[0x10, 0xFE]), "DJNZ $8000");
    assert_eq!(text(&[0x20, 0xFC]), "JR NZ,$7FFE");
}

#[test]
fn indexed_operands_carry_their_sign() {
    assert_eq!(text(&[0xDD, 0x7E, 0x05]), "LD A,(IX+$05)");
    assert_eq!(text(&[0xDD, 0x7E, 0xFB]), "LD A,(IX-$05)");
    assert_eq!(text(&[0xFD, 0x36, 0x02, 0xFF]), "LD (IY+$02),$FF");
    assert_eq!(text(&[0xDD, 0x34, 0x00]), "INC (IX+$00)");
    assert_eq!(text(&[0xDD, 0xE5]), "PUSH IX");
    assert_eq!(text(&[0xDD, 0x21, 0x00, 0x40]), "LD IX,$4000");
    // A memory operand leaves the other operand unprefixed: this is LD (IX+d),H,
    // not LD (IX+d),IXH.
    assert_eq!(text(&[0xDD, 0x74, 0x03]), "LD (IX+$03),H");
}

#[test]
fn undocumented_forms_are_decoded_and_flagged() {
    let sll = disasm(&[0xCB, 0x30]);
    assert_eq!(sll.text, "SLL B");
    assert!(sll.undocumented);

    let ixh = disasm(&[0xDD, 0x24]);
    assert_eq!(ixh.text, "INC IXH");
    assert!(ixh.undocumented);

    // DD CB d 06 is the documented RLC (IX+d)...
    let plain = disasm(&[0xDD, 0xCB, 0x05, 0x06]);
    assert_eq!(plain.text, "RLC (IX+$05)");
    assert!(!plain.undocumented);

    // ...and DD CB d 00 does the same thing but also copies the result into B.
    let copying = disasm(&[0xDD, 0xCB, 0x05, 0x00]);
    assert_eq!(copying.text, "RLC (IX+$05),B");
    assert!(copying.undocumented);

    let noni = disasm(&[0xED, 0x00]);
    assert_eq!(noni.text, "NOP");
    assert_eq!(noni.len, 2);
    assert!(noni.undocumented);

    assert_eq!(text(&[0xED, 0x70]), "IN (C)");
    assert_eq!(text(&[0xED, 0x71]), "OUT (C),0");
}

#[test]
fn ed_page_and_block_instructions() {
    assert_eq!(text(&[0xED, 0xB0]), "LDIR");
    assert_eq!(text(&[0xED, 0xA1]), "CPI");
    assert_eq!(text(&[0xED, 0xB3]), "OTIR");
    assert_eq!(text(&[0xED, 0x52]), "SBC HL,DE");
    assert_eq!(text(&[0xED, 0x4B, 0x00, 0x40]), "LD BC,($4000)");
    assert_eq!(text(&[0xED, 0x43, 0x00, 0x40]), "LD ($4000),BC");
    assert_eq!(text(&[0xED, 0x5E]), "IM 2");
    assert_eq!(text(&[0xED, 0x4D]), "RETI");
    assert_eq!(text(&[0xED, 0x67]), "RRD");
}

#[test]
fn flow_supports_step_over_and_run_to_return() {
    // Step over a CALL means breakpointing the following address.
    let call = disasm(&[0xCD, 0x00, 0x90]);
    assert_eq!(
        call.flow,
        Flow::Call {
            target: Some(0x9000),
            conditional: false
        }
    );
    assert_eq!(call.addr + u16::from(call.len), 0x8003);

    // An unconditional jump does not fall through, so a linear disassembler
    // should stop rather than decode whatever data follows.
    let jp = disasm(&[0xC3, 0x00, 0x90]);
    assert!(!jp.flow.falls_through());
    assert!(disasm(&[0xC2, 0x00, 0x90]).flow.falls_through(), "JP NZ does");

    // JP (HL) has no statically known target.
    assert_eq!(
        disasm(&[0xE9]).flow,
        Flow::Jump {
            target: None,
            conditional: false
        }
    );

    assert_eq!(
        disasm(&[0xC9]).flow,
        Flow::Return {
            conditional: false
        }
    );
    assert_eq!(disasm(&[0xC0]).flow, Flow::Return { conditional: true });
    assert_eq!(disasm(&[0x76]).flow, Flow::Halt);
    assert_eq!(disasm(&[0xED, 0xB0]).flow, Flow::Repeat);
}

#[test]
fn range_disassembly_and_listing_format() {
    let mut mem = FlatMemory::new();
    // The opening of a routine that clears the Spectrum's attribute area.
    mem.load(
        ORG,
        &[
            0x21, 0x00, 0x58, // LD HL,$5800
            0x11, 0x01, 0x58, // LD DE,$5801
            0x01, 0xFF, 0x02, // LD BC,$02FF
            0x36, 0x38, // LD (HL),$38
            0xED, 0xB0, // LDIR
            0xC9, // RET
        ],
    );

    let insns = disassemble_range(&mem, ORG, 6);
    let texts: Vec<&str> = insns.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "LD HL,$5800",
            "LD DE,$5801",
            "LD BC,$02FF",
            "LD (HL),$38",
            "LDIR",
            "RET",
        ]
    );

    assert_eq!(insns[0].listing_line(), "8000  21 00 58     LD HL,$5800");
    assert_eq!(insns[4].listing_line(), "800B  ED B0        LDIR");
}
