//! What the encoder says when an instruction cannot be encoded.
//!
//! Two kinds of failure, kept apart because they mean different things to the
//! pass driver. A *form* error is about the instruction itself — there is no
//! such instruction, whatever the values turn out to be — and is final. A
//! *value* error is about an operand, and on the first pass may only mean the
//! symbol has not been reached yet.

mod common;

use common::assemble_one;
use rkw_asm::encode::{self, EncodeError};
use rkw_asm::{Site, SourceMap, Symbols, parse};

const ORG: u16 = 0x8000;

fn error(text: &str) -> EncodeError {
    assemble_one(text, ORG).expect_err("should not encode")
}

fn message(text: &str) -> String {
    error(text).diagnostic().message
}

/// The length the encoder works out from the syntax alone, with no symbol
/// table in sight.
fn planned_length(text: &str) -> usize {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", format!("    {text}\n"));
    let parsed = parse(&map, file);
    let op = parsed.program.statements[0].op.as_ref().expect("an op");
    encode::plan(op).expect("plans").len()
}

#[test]
fn an_unknown_mnemonic_names_itself() {
    assert_eq!(message("frobnicate 1"), "unknown instruction `frobnicate`");
}

#[test]
fn an_impossible_operand_combination_shows_what_was_written() {
    assert_eq!(message("ld a,b,c"), "no form of `ld` takes `a,b,c`");
    assert_eq!(message("ld (hl),(hl)"), "no form of `ld` takes `(hl),(hl)`");
    // One prefix byte cannot mean IX and IY at once.
    assert_eq!(message("ld ixh,iyl"), "no form of `ld` takes `ixh,iyl`");
    // Nor can a half register be reached while the prefix is busy addressing.
    assert_eq!(
        message("ld (ix+1),ixh"),
        "no form of `ld` takes `(ix+1),ixh`"
    );
    // Only four of the eight conditions have a relative form.
    assert_eq!(message("jr po,$8000"), "no form of `jr` takes `po,$8000`");
    // `ADD IX,HL` does not exist: under the prefix, that slot is IX itself.
    assert_eq!(message("add ix,hl"), "no form of `add` takes `ix,hl`");
}

#[test]
fn operand_fields_that_are_part_of_the_opcode_are_range_checked() {
    assert_eq!(message("bit 8,a"), "8 does not fit in a bit number");
    assert_eq!(message("rst 9"), "`RST 9` is not a restart address");
    assert_eq!(message("im 3"), "there is no interrupt mode 3");
}

#[test]
fn immediates_are_range_checked_against_the_field_they_go_into() {
    assert_eq!(message("ld a,300"), "300 does not fit in one byte");
    assert_eq!(message("ld hl,$10000"), "65536 does not fit in two bytes");
    // A displacement is signed only, so 200 is not 200 but -56.
    assert_eq!(
        message("ld a,(ix+200)"),
        "200 does not fit in a signed byte"
    );
}

#[test]
fn a_relative_jump_out_of_range_names_the_distance() {
    let too_far = error("jr $9000");
    assert_eq!(
        too_far.diagnostic().message,
        "relative jump of 4094 bytes is out of range"
    );
    // Not a forward reference: no later pass is going to make it reachable.
    assert!(!too_far.is_forward_reference());
}

#[test]
fn an_unresolved_operand_is_a_forward_reference_on_the_first_pass() {
    let unresolved = error("jp not_defined_yet");
    assert!(unresolved.is_forward_reference());
    assert!(matches!(unresolved, EncodeError::Value(_)));

    // A wrong instruction is not, however unknown its operands are.
    assert!(!error("frobnicate not_defined_yet").is_forward_reference());
}

#[test]
fn length_is_known_without_evaluating_anything() {
    // The point of the plan/emit split, and the reason the first pass can
    // assign addresses that turn out to be right: none of these operands can be
    // resolved, and all of the lengths are still correct.
    assert_eq!(planned_length("nop"), 1);
    assert_eq!(planned_length("ld a,unknown"), 2);
    assert_eq!(planned_length("ld hl,unknown"), 3);
    assert_eq!(planned_length("ld (ix+unknown),unknown"), 4);
    assert_eq!(planned_length("jr unknown"), 2);
    assert_eq!(planned_length("jp unknown"), 3);
    assert_eq!(planned_length("bit unknown,(iy+unknown)"), 4);
    assert_eq!(planned_length("rst unknown"), 1);
}

#[test]
fn a_symbol_may_supply_any_operand() {
    let mut symbols = Symbols::new();
    let mut map = SourceMap::new();
    let file = map.add("t.asm", "    ld a,(ix+offset)\n");
    let parsed = parse(&map, file);
    let op = parsed.program.statements[0].op.as_ref().expect("an op");

    // Nothing about the encoding depends on the value, only the byte it fills.
    let plan = encode::plan(op).expect("plans");
    assert_eq!(plan.len(), 3);

    let source = map.add("c.asm", "offset equ 5\n");
    let mut defined = parse(&map, source).program.statements.remove(0);
    let label = defined.label.take().expect("a name");
    let value = defined.op.expect("equ").args.remove(0);
    symbols
        .define_const(&label.name, value, Site::default(), label.span, false)
        .expect("defines");

    let bytes = encode::emit(&plan, Site::default(), &mut symbols).expect("emits");
    assert_eq!(bytes, [0xDD, 0x7E, 0x05]);
}
