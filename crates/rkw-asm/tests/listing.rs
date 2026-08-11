//! The listing and the symbol table dumps.

mod common;

use common::assemble_ok;
use rkw_asm::DebugInfo;
use rkw_asm::listing::{listing, symbols_by_address, symbols_by_name};

#[test]
fn a_listing_shows_addresses_bytes_and_the_source_line() {
    let source = "\
; count down from four
        org $8000
wait:   ld b,4
.loop:  djnz .loop
        ret
";
    let (map, assembled) = assemble_ok(source);
    assert_eq!(
        listing(&map, &assembled),
        concat!(
            "; ---- program.asm ----\n",
            "                  ; count down from four\n",
            "                          org $8000\n",
            "8000  06 04       wait:   ld b,4\n",
            "8002  10 FE       .loop:  djnz .loop\n",
            "8004  C9                  ret\n",
        )
    );
}

#[test]
fn long_data_continues_underneath_rather_than_running_off_the_side() {
    let (map, assembled) = assemble_ok("    org $8000\n    db 1,2,3,4,5,6,7\n");
    assert_eq!(
        listing(&map, &assembled),
        concat!(
            "; ---- program.asm ----\n",
            "                      org $8000\n",
            "8000  01 02 03 04     db 1,2,3,4,5,6,7\n",
            "8004  05 06 07\n",
        )
    );
}

#[test]
fn macro_expansions_are_shown_under_the_line_that_asked_for_them() {
    let source = "\
        org $8000
pair    macro value
        db value,value
        endm
        pair 7
";
    let (map, assembled) = assemble_ok(source);
    let text = listing(&map, &assembled);
    // The invocation line itself emits nothing directly; the body statement is
    // printed under it, marked, showing the line the reader has to look at.
    assert!(
        text.contains("                          pair 7\n"),
        "{text}"
    );
    assert!(text.contains("8000  07 07       > "), "{text}");
    assert!(text.contains("> "), "{text}");
}

#[test]
fn nesting_is_marked_by_depth() {
    let source = "\
        org $8000
inner   macro value
        db value
        endm
outer   macro
        inner 1
        inner 2
        endm
        outer
";
    let (map, assembled) = assemble_ok(source);
    let text = listing(&map, &assembled);
    // Two levels deep, so two markers.
    assert!(
        text.contains("8000  01          >>         db value"),
        "{text}"
    );
    assert!(
        text.contains("8001  02          >>         db value"),
        "{text}"
    );
}

#[test]
fn every_file_appears_in_the_listing() {
    let source = "    org $8000\n    nop\n";
    let (map, assembled) = assemble_ok(source);
    let text = listing(&map, &assembled);
    assert!(text.starts_with("; ---- program.asm ----\n"));
}

#[test]
fn the_symbol_table_can_be_read_by_name_or_by_address() {
    let source = "\
        org $8000
width   equ 32
start:  nop
count   defl 3
later:  nop
";
    let (map, mut assembled) = assemble_ok(source);
    let info = DebugInfo::new(&map, &mut assembled);

    assert_eq!(
        symbols_by_name(&info),
        concat!(
            "count                    $0003  variable  program.asm:4\n",
            "later                    $8001  label     program.asm:5\n",
            "start                    $8000  label     program.asm:3\n",
            "width                    $0020  constant  program.asm:2\n",
        )
    );
    assert_eq!(
        symbols_by_address(&info),
        concat!(
            "count                    $0003  variable  program.asm:4\n",
            "width                    $0020  constant  program.asm:2\n",
            "start                    $8000  label     program.asm:3\n",
            "later                    $8001  label     program.asm:5\n",
        )
    );
}
