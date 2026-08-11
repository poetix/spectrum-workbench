//! Macro definition, expansion, repetition and hygiene.

mod common;

use common::{assemble_ok, errors, symbol};
use z80::{Cpu, FlatMemory};

fn binary(source: &str) -> Vec<u8> {
    assemble_ok(source).1.image.to_binary()
}

#[test]
fn a_macro_is_defined_and_invoked_by_name() {
    let source = "\
poke    macro address,value
        ld a,value
        ld ($0000+address),a
        endm

        poke $4000,$FF
";
    assert_eq!(binary(source), [0x3E, 0xFF, 0x32, 0x00, 0x40]);
}

#[test]
fn both_spellings_of_the_definition_work() {
    // `name MACRO params` and `MACRO name params`. The second is why the
    // parser has a special case: it is the one statement whose first operand
    // is separated by a space rather than a comma.
    let with_label = "one macro\n    db 1\n    endm\n    one\n";
    let with_name = "    macro one\n    db 1\n    endm\n    one\n";
    assert_eq!(binary(with_label), [1]);
    assert_eq!(binary(with_name), [1]);

    let parameters = "    macro two a,b\n    db a,b\n    endm\n    two 3,4\n";
    assert_eq!(binary(parameters), [3, 4]);
}

#[test]
fn arguments_are_expressions_and_may_be_registers() {
    // Substitution is by expression, so an argument can be anything an operand
    // can be — a register name, a parenthesised address, or arithmetic.
    let source = "\
load    macro register,source
        ld register,source
        endm

        load b,(hl)
        load a,2*3
";
    assert_eq!(binary(source), [0x46, 0x3E, 6]);
}

#[test]
fn a_macro_may_use_the_symbols_around_it() {
    let source = "\
size    equ 4
fill    macro value
        ds size,value
        endm

        fill $AA
";
    assert_eq!(binary(source), [0xAA; 4]);
}

// -- hygiene ----------------------------------------------------------------

#[test]
fn local_labels_are_unique_to_each_expansion() {
    let source = "\
        org $8000
wait    macro count
        ld b,count
.loop:  djnz .loop
        endm

        wait 3
        wait 4
";
    let (_, mut assembled) = assemble_ok(source);
    // Two expansions, two `.loop` symbols, each hanging off a name unique to
    // its expansion rather than off whatever global label preceded the call.
    let names: Vec<String> = assembled
        .symbols
        .iter_values()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.ends_with(".loop"))
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_ne!(names[0], names[1]);

    // And each `DJNZ` jumped to its own copy, two bytes back.
    let bytes = assembled.image.to_binary();
    assert_eq!(bytes, [0x06, 3, 0x10, 0xFE, 0x06, 4, 0x10, 0xFE]);
    let _ = symbol(&mut assembled, &names[0]);
}

#[test]
fn a_global_label_in_a_macro_used_twice_is_a_duplicate() {
    // Not hygiene's job: a name written without a dot is meant to be global,
    // and using it twice is the mistake the error describes.
    let source = "\
mark    macro
here:   nop
        endm
        mark
        mark
";
    assert_eq!(errors(source), ["`here` is already defined"]);
}

// -- repetition -------------------------------------------------------------

#[test]
fn rept_repeats_its_body() {
    assert_eq!(binary("    rept 3\n    nop\n    endr\n"), [0, 0, 0]);
    assert_eq!(binary("    dup 2\n    db 1,2\n    edup\n"), [1, 2, 1, 2]);
    assert_eq!(binary("    rept 0\n    nop\n    endr\n    db 9\n"), [9]);
}

#[test]
fn rept_can_name_its_counter() {
    assert_eq!(binary("    rept 4,n\n    db n\n    endr\n"), [0, 1, 2, 3]);
    // The counter is a variable, so it can be used in arithmetic.
    assert_eq!(binary("    rept 3,i\n    db i*2+1\n    endr\n"), [1, 3, 5]);
}

#[test]
fn repetitions_nest_and_may_contain_macros() {
    let source = "\
pair    macro value
        db value,value
        endm

        rept 2,outer
        rept 2,inner
        pair outer*2+inner
        endr
        endr
";
    assert_eq!(binary(source), [0, 0, 1, 1, 2, 2, 3, 3]);
}

#[test]
fn a_repetition_count_that_is_not_yet_known_settles_on_a_later_pass() {
    let (_, assembled) = assemble_ok("    rept count\n    nop\n    endr\ncount equ 3\n");
    assert_eq!(assembled.image.to_binary(), [0, 0, 0]);
    assert!(assembled.passes >= 2);
}

#[test]
fn an_unreasonable_repetition_is_refused_rather_than_attempted() {
    assert_eq!(
        errors("    rept 1000000\n    nop\n    endr\n"),
        ["`REPT 1000000` is more than the 65536 this assembler will do"]
    );
    assert_eq!(
        errors("    rept -1\n    nop\n    endr\n"),
        ["`REPT -1` repeats a negative number of times"]
    );
}

// -- nesting and recursion --------------------------------------------------

#[test]
fn macros_nest() {
    let source = "\
inner   macro value
        db value
        endm

outer   macro first,second
        inner first
        inner second
        endm

        outer 1,2
";
    assert_eq!(binary(source), [1, 2]);
}

#[test]
fn a_runaway_recursive_macro_reports_its_chain_rather_than_overflowing() {
    let source = "\
forever macro
        forever
        endm
        forever
";
    let mut map = rkw_asm::SourceMap::new();
    let file = map.add("t.asm", source);
    let assembled = common::assemble(&mut map, file);

    let diagnostic = &assembled.diagnostics[0];
    assert_eq!(
        diagnostic.message,
        "`forever` is expanded more than 32 deep"
    );
    // The chain is on the diagnostic, two notes per level: where it was
    // invoked, and where it is defined.
    assert_eq!(diagnostic.related.len(), 64);
    assert_eq!(diagnostic.related[0].1, "in this expansion of `forever`");
}

// -- errors -----------------------------------------------------------------

#[test]
fn an_error_inside_an_expansion_names_both_sites() {
    let source = "\
shift   macro value
        ld a,value*100
        endm

        shift 4
";
    let mut map = rkw_asm::SourceMap::new();
    let file = map.add("t.asm", source);
    let assembled = common::assemble(&mut map, file);

    let diagnostic = &assembled.diagnostics[0];
    assert_eq!(diagnostic.message, "400 does not fit in one byte");
    // The error itself points at the expression in the macro body, which is
    // where the mistake is written...
    assert_eq!(map.location(diagnostic.span).line, 2);
    // ...and the notes point at the call that supplied the value, and at the
    // definition it went into.
    assert_eq!(diagnostic.related.len(), 2);
    assert_eq!(map.location(diagnostic.related[0].0).line, 5);
    assert_eq!(diagnostic.related[0].1, "in this expansion of `shift`");
    assert_eq!(map.location(diagnostic.related[1].0).line, 1);
    assert_eq!(diagnostic.related[1].1, "`shift` is defined here");
}

#[test]
fn the_wrong_number_of_arguments_says_how_many_were_expected() {
    let source = "pair macro a,b\n    db a,b\n    endm\n    pair 1\n";
    assert_eq!(errors(source), ["`pair` takes 2 arguments, not 1"]);

    let source = "none macro\n    nop\n    endm\n    none 1\n";
    assert_eq!(errors(source), ["`none` takes 0 arguments, not 1"]);
}

#[test]
fn a_macro_may_not_be_called_after_an_instruction_or_defined_twice() {
    assert_eq!(
        errors("ld macro\n    nop\n    endm\n"),
        ["`ld` is an instruction, so a macro cannot be called that"]
    );
    let twice = "one macro\n    nop\n    endm\none macro\n    nop\n    endm\n";
    assert_eq!(errors(twice), ["`one` is already a macro"]);
}

#[test]
fn unclosed_and_unopened_blocks_are_reported() {
    assert_eq!(errors("one macro\n    nop\n"), ["`macro` is never closed"]);
    assert_eq!(errors("    rept 2\n    nop\n"), ["`rept` is never closed"]);
    assert_eq!(errors("    endm\n"), ["`ENDM` closes nothing"]);
    assert_eq!(errors("    endr\n"), ["`ENDR` closes nothing"]);
}

#[test]
fn a_macro_inside_a_skipped_branch_is_stepped_over_as_a_unit() {
    // The `ENDM` must not be read as closing something else, and the body must
    // not be assembled on its own.
    let source = "\
    if 0
skip    macro
        db 1
        endm
    rept 3
        db 2
        endr
    endif
        db 9
";
    assert_eq!(binary(source), [9]);
}

// -- what the listing needs -------------------------------------------------

#[test]
fn every_expansion_is_recorded_with_its_nesting() {
    let source = "\
inner   macro value
        db value
        endm

outer   macro
        inner 1
        inner 2
        endm

        outer
";
    let (_, assembled) = assemble_ok(source);
    let described: Vec<(&str, Option<usize>)> = assembled
        .expansions
        .iter()
        .map(|expansion| (expansion.name.as_str(), expansion.parent))
        .collect();
    assert_eq!(
        described,
        [("outer", None), ("inner", Some(0)), ("inner", Some(0))]
    );

    // Each byte-emitting statement says which expansion produced it, which is
    // what lets the listing indent them.
    let lines: Vec<Option<usize>> = assembled.lines.iter().map(|line| line.expansion).collect();
    assert_eq!(lines, [Some(1), Some(2)]);
}

// -- end to end -------------------------------------------------------------

#[test]
fn a_program_built_from_macros_runs() {
    let source = "\
        org $8000

; Add `count` copies of `value` into the accumulator.
accumulate macro value,count
        ld b,count
.loop:  add a,value
        djnz .loop
        endm

start:  xor a
        accumulate 3,4
        accumulate 10,2
        ld (total),a
        ret

total:  db 0
";
    let (_, mut assembled) = assemble_ok(source);
    let origin = assembled.image.origin().expect("assembled something");

    let mut memory = FlatMemory::new();
    memory.load(origin, &assembled.image.to_binary());
    let mut cpu = Cpu::new();
    cpu.regs.pc = origin;
    cpu.regs.sp = 0xFF00;
    memory.ram[0xFF00] = 0;
    memory.ram[0xFF01] = 0;

    for _ in 0..10_000 {
        if cpu.regs.pc == 0 {
            break;
        }
        cpu.step(&mut memory);
    }

    let total = symbol(&mut assembled, "total") as usize;
    assert_eq!(cpu.regs.pc, 0, "the program returned");
    assert_eq!(memory.ram[total], 3 * 4 + 10 * 2);
}
