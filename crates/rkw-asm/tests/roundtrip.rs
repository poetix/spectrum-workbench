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
//! What it does not check is that the operands *mean* the same thing; nothing
//! in this crate knows that yet. Ticket 0003 closes the loop by assembling the
//! text back to the bytes it was decoded from.

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
