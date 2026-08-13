//! Round-trip against the disassembler.
//!
//! The disassembler emits sjasmplus-compatible text for every opcode in every
//! prefix page, including the undocumented ones, so it is a ready-made corpus
//! of everything the assembler has to accept. Each instruction is disassembled,
//! parsed, and printed back; the printed form has to be the text it came from.
//!
//! That is a stronger check than "it parses". Printing reproduces the source
//! spelling of every literal and operator, so a difference means the tree lost
//! something — an operand merged, a displacement's sign dropped, a hex literal
//! silently renormalised — rather than merely that the parse succeeded.
//!
//! The second test closes the loop through the encoder: each instruction is
//! assembled from that text and disassembled again, and has to come back the
//! same. Text to bytes to text, checked against a CPU core validated by Fuse
//! and `zexall`, is as close to an independent check of the encoder as this
//! repository can produce.

mod common;

use common::assemble_one;
use rkw_asm::{SourceMap, parse};
use z80::{FlatMemory, disassemble};

const ORG: u16 = 0x8000;

/// The disassembly of every opcode in every page.
///
/// Two passes over the trailing bytes: `0x05` gives positive displacements and
/// small immediates, `0xFB` gives negative ones, so `(IX-$05)` is covered as
/// well as `(IX+$05)`.
fn every_instruction() -> Vec<String> {
    const PREFIXES: [&[u8]; 7] = [
        &[],
        &[0xCB],
        &[0xED],
        &[0xDD],
        &[0xFD],
        &[0xDD, 0xCB],
        &[0xFD, 0xCB],
    ];

    let mut out = Vec::new();
    for filler in [0x05u8, 0xFB] {
        for prefix in PREFIXES {
            for opcode in 0..=0xFFu8 {
                let mut bytes = prefix.to_vec();
                // `DD CB` and `FD CB` put the displacement before the opcode.
                if prefix.len() == 2 {
                    bytes.push(filler);
                }
                bytes.push(opcode);
                bytes.extend_from_slice(&[filler; 4]);

                let mut mem = FlatMemory::new();
                mem.load(ORG, &bytes);
                out.push(disassemble(&mem, ORG).text);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn every_disassembled_instruction_parses_back_to_itself() {
    let mut map = SourceMap::new();
    for text in every_instruction() {
        // Indented, as an instruction would be written: a mnemonic in column 1
        // is only distinguishable from a label by the name table, and this
        // test is not about that.
        let file = map.add("disasm.asm", format!("    {text}\n"));
        let parsed = parse(&map, file);

        assert!(
            parsed.diagnostics.is_empty(),
            "{text:?} did not parse:\n{}",
            map.render_all(&parsed.diagnostics)
        );
        assert_eq!(
            parsed.program.statements.len(),
            1,
            "{text:?} did not parse as one statement"
        );
        let stmt = &parsed.program.statements[0];
        assert!(stmt.label.is_none(), "{text:?} parsed a label");
        assert_eq!(
            stmt.op.as_ref().expect("an operation").to_string(),
            text,
            "printed form differs from the disassembly"
        );
    }
}

#[test]
fn the_corpus_covers_the_awkward_forms() {
    // A guard on the generator rather than on the parser: if a change to the
    // disassembler or to the filler bytes stopped producing these, the test
    // above would still pass while checking much less.
    let all = every_instruction();
    for expected in [
        "EX AF,AF'",        // the shadow-register apostrophe
        "LD A,(IX+$05)",    // positive displacement
        "LD A,(IX-$05)",    // negative displacement
        "LD (IX+$05),$05",  // displacement and immediate in one instruction
        "RES 0,(IX+$05),B", // the undocumented three-operand form
        "IN (C)",           // an operand that is only a parenthesised register
        "OUT (C),0",
        "JP (HL)",
        "SUB $05",
        "ADD A,$05",
        "RST $38",
        "IM 0",
        "NOP",
    ] {
        assert!(
            all.iter().any(|t| t == expected),
            "corpus lacks {expected:?}"
        );
    }
}

#[test]
fn every_instruction_assembles_to_an_encoding_that_disassembles_back() {
    // Not a byte-for-byte comparison with the opcode the text came from: some
    // encodings are not canonical. `DD 40` is `LD B,B` with a prefix that
    // changes nothing, and the ED page has two-byte no-ops; both disassemble to
    // an instruction that assembles to the shorter form. Requiring the text to
    // survive the trip is the strongest check that holds for all of them.
    for text in every_instruction() {
        let bytes = match assemble_one(&text, ORG) {
            Ok(bytes) => bytes,
            Err(e) => panic!("{text:?} did not assemble: {:?}", e.diagnostic().message),
        };

        let mut mem = FlatMemory::new();
        mem.load(ORG, &bytes);
        let decoded = disassemble(&mem, ORG);

        assert_eq!(decoded.text, text, "assembled to {bytes:02X?}");
        assert_eq!(
            usize::from(decoded.len),
            bytes.len(),
            "{text:?} assembled to {bytes:02X?}, which the CPU reads as {} bytes",
            decoded.len
        );
    }
}

#[test]
fn the_encoding_of_a_known_instruction_is_the_documented_one() {
    // A handful of hand-checked encodings, so that a systematic error shared by
    // the assembler and the disassembler cannot pass the round trip unnoticed.
    for (text, expected) in [
        ("NOP", &[0x00][..]),
        ("LD A,$2A", &[0x3E, 0x2A]),
        ("LD B,A", &[0x47]),
        ("LD HL,$1234", &[0x21, 0x34, 0x12]),
        ("LD (HL),$FF", &[0x36, 0xFF]),
        ("LD A,(IX+$05)", &[0xDD, 0x7E, 0x05]),
        ("LD (IY-$03),B", &[0xFD, 0x70, 0xFD]),
        ("LD (IX+$05),$FF", &[0xDD, 0x36, 0x05, 0xFF]),
        ("ADD A,B", &[0x80]),
        ("SUB $10", &[0xD6, 0x10]),
        ("ADD HL,DE", &[0x19]),
        ("ADC HL,BC", &[0xED, 0x4A]),
        ("SBC HL,SP", &[0xED, 0x72]),
        ("ADD IX,IX", &[0xDD, 0x29]),
        ("INC (IX+$01)", &[0xDD, 0x34, 0x01]),
        ("BIT 7,(HL)", &[0xCB, 0x7E]),
        ("SET 0,(IX+$02)", &[0xDD, 0xCB, 0x02, 0xC6]),
        ("RES 1,(IY+$02),C", &[0xFD, 0xCB, 0x02, 0x89]),
        ("SLL A", &[0xCB, 0x37]),
        ("JP $8000", &[0xC3, 0x00, 0x80]),
        ("JP NZ,$8000", &[0xC2, 0x00, 0x80]),
        ("JP (IX)", &[0xDD, 0xE9]),
        ("CALL $1234", &[0xCD, 0x34, 0x12]),
        ("RET PO", &[0xE0]),
        ("RST $38", &[0xFF]),
        ("PUSH AF", &[0xF5]),
        ("EX AF,AF'", &[0x08]),
        ("EX (SP),IY", &[0xFD, 0xE3]),
        ("IN A,($FE)", &[0xDB, 0xFE]),
        ("IN (C)", &[0xED, 0x70]),
        ("OUT (C),0", &[0xED, 0x71]),
        ("OUT (C),D", &[0xED, 0x51]),
        ("IM 2", &[0xED, 0x5E]),
        ("LD A,I", &[0xED, 0x57]),
        ("LD ($4000),BC", &[0xED, 0x43, 0x00, 0x40]),
        ("LD ($4000),HL", &[0x22, 0x00, 0x40]),
        ("LD SP,IX", &[0xDD, 0xF9]),
        ("LDIR", &[0xED, 0xB0]),
        ("LD IXH,$05", &[0xDD, 0x26, 0x05]),
        ("LD B,IYL", &[0xFD, 0x45]),
    ] {
        assert_eq!(
            assemble_one(text, ORG).expect("assembles"),
            expected,
            "{text}"
        );
    }
}

/// sjasmplus documents all three as accepted spellings of `EX AF,AF'`, and
/// sources do use them — the apostrophe is awkward to type and awkward to lex.
/// Ticket 0030.
#[test]
fn the_shadow_exchange_has_four_spellings() {
    for text in ["EX AF,AF'", "EX AF,AF", "EX AF", "EXA"] {
        assert_eq!(
            assemble_one(text, ORG).expect("assembles"),
            [0x08],
            "{text}"
        );
    }
}

#[test]
fn a_relative_jump_is_encoded_as_a_distance() {
    // `JR $8002` from $8000 goes nowhere: the CPU has already advanced past the
    // two bytes by the time it adds the displacement.
    assert_eq!(assemble_one("JR $8002", ORG).unwrap(), [0x18, 0x00]);
    assert_eq!(assemble_one("JR $8000", ORG).unwrap(), [0x18, 0xFE]);
    assert_eq!(assemble_one("DJNZ $7F82", ORG).unwrap(), [0x10, 0x80]);
    assert_eq!(assemble_one("JR NZ,$8081", ORG).unwrap(), [0x20, 0x7F]);
}
