//! Directive tests: layout, data, symbols, files and conditional assembly.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{assemble, assemble_ok, errors, symbol};
use rkw_asm::SourceMap;

/// The bytes a source assembles to, as one contiguous run.
fn binary(source: &str) -> Vec<u8> {
    assemble_ok(source).1.image.to_binary()
}

// -- layout -----------------------------------------------------------------

#[test]
fn org_places_the_code_and_starts_a_new_section() {
    let (_, assembled) = assemble_ok("    org $8000\n    nop\n    dw $-$$\n");
    assert_eq!(assembled.image.origin(), Some(0x8000));
    // `$$` is the start of the section `ORG` opened, so `$-$$` is one byte in.
    assert_eq!(assembled.image.to_binary(), [0x00, 0x01, 0x00]);
}

#[test]
fn two_origins_produce_two_segments_rather_than_a_hole() {
    let (_, assembled) = assemble_ok("    org $8000\n    db 1\n    org $9000\n    db 2\n");
    let segments = assembled.image.segments();
    assert_eq!(segments.len(), 2);
    assert_eq!(
        (segments[0].origin, &segments[0].bytes[..]),
        (0x8000, &[1][..])
    );
    assert_eq!(
        (segments[1].origin, &segments[1].bytes[..]),
        (0x9000, &[2][..])
    );
    // A raw binary spans the gap, which is why the segments exist separately.
    assert_eq!(assembled.image.to_binary().len(), 0x1001);
}

#[test]
fn align_pads_to_the_next_boundary() {
    let (mut map, mut assembled) =
        assemble_ok("    org $8001\n    db 1\n    align 4\nafter:\n    db 2\n");
    let _ = &mut map;
    assert_eq!(symbol(&mut assembled, "after"), 0x8004);
    assert_eq!(assembled.image.to_binary(), [1, 0, 0, 2]);

    // Already aligned means no padding at all, not a whole boundary of it.
    let (_, mut aligned) = assemble_ok("    org $8000\n    db 1\n    align 1\nhere:\n");
    assert_eq!(symbol(&mut aligned, "here"), 0x8001);

    assert_eq!(errors("    align 3\n"), ["`ALIGN 3` is not a power of two"]);
}

#[test]
fn ds_reserves_space_and_can_fill_it() {
    assert_eq!(binary("    db 1\n    ds 3\n    db 2\n"), [1, 0, 0, 0, 2]);
    assert_eq!(binary("    ds 3,$FF\n"), [0xFF, 0xFF, 0xFF]);
    assert_eq!(
        errors("    ds -1\n"),
        ["`DS -1` reserves a negative amount"]
    );
}

// -- data -------------------------------------------------------------------

#[test]
fn byte_and_word_data() {
    assert_eq!(binary("    db 1,2,$FF\n"), [1, 2, 0xFF]);
    assert_eq!(binary("    defb -1\n"), [0xFF]);
    assert_eq!(binary("    dw $1234,0\n"), [0x34, 0x12, 0x00, 0x00]);
    assert_eq!(binary("    defw $FFFF\n"), [0xFF, 0xFF]);
    // Mixed expressions and characters, which is how a table is usually
    // written.
    assert_eq!(binary("    db 'a','a'+1,2*3\n"), [97, 98, 6]);
}

#[test]
fn string_data_keeps_its_escapes_and_suffixes() {
    assert_eq!(binary("    db \"HI\"\n"), b"HI");
    // Escapes are processed between double quotes only.
    assert_eq!(binary("    db \"a\\nb\"\n"), [97, 10, 98]);
    assert_eq!(binary("    db 'a\\nb'\n"), b"a\\nb");
    // The `z` and `c` suffixes are part of the literal.
    assert_eq!(binary("    db \"HI\"z\n"), [b'H', b'I', 0]);
    assert_eq!(binary("    db \"HI\"c\n"), [b'H', b'I' | 0x80]);
    // DZ terminates the whole statement rather than each literal.
    assert_eq!(binary("    dz \"AB\",\"CD\"\n"), [65, 66, 67, 68, 0]);
}

#[test]
fn data_that_does_not_fit_is_reported_and_still_takes_its_space() {
    // Reporting and then carrying on is what keeps the addresses after it
    // right, so one bad byte does not move every label below it.
    let mut map = SourceMap::new();
    let file = map.add("t.asm", "    db 300\nafter:\n");
    let mut assembled = assemble(&mut map, file);
    assert_eq!(
        assembled.diagnostics[0].message,
        "300 does not fit in one byte"
    );
    assert_eq!(symbol(&mut assembled, "after"), 1);
}

// -- symbols ----------------------------------------------------------------

#[test]
fn equ_defl_and_module_scoping() {
    let (_, mut assembled) = assemble_ok(
        "size    equ 40\ncount   defl 1\ncount   defl count+1\n    module video\nclear:  ret\n    endmodule\n",
    );
    assert_eq!(symbol(&mut assembled, "size"), 40);
    assert_eq!(symbol(&mut assembled, "count"), 2);
    assert_eq!(symbol(&mut assembled, "video.clear"), 0);
}

#[test]
fn equ_may_not_be_redefined_but_defl_may() {
    assert_eq!(errors("x equ 1\nx equ 2\n"), ["`x` is already defined"]);
    assert!(errors("x defl 1\nx defl 2\n").is_empty());
    assert_eq!(errors("    equ 1\n"), ["`equ` needs a name to its left"]);
}

// -- conditionals -----------------------------------------------------------

#[test]
fn conditional_assembly_chooses_a_branch() {
    assert_eq!(
        binary("    if 1\n    db 1\n    else\n    db 2\n    endif\n"),
        [1]
    );
    assert_eq!(
        binary("    if 0\n    db 1\n    else\n    db 2\n    endif\n"),
        [2]
    );
    assert_eq!(binary("    if 0\n    db 1\n    endif\n    db 9\n"), [9]);
}

#[test]
fn conditionals_nest() {
    let source = "\
    if 1
      if 0
        db 1
      else
        db 2
      endif
      db 3
    else
      db 4
    endif
";
    assert_eq!(binary(source), [2, 3]);

    // The skipped branch's inner conditional must still balance, or its ENDIF
    // closes the outer one.
    let source = "\
    if 0
      if 1
        db 1
      endif
    else
      db 2
    endif
    db 3
";
    assert_eq!(binary(source), [2, 3]);
}

#[test]
fn elseif_takes_the_first_true_branch_only() {
    let source = "\
    if 0
      db 1
    elseif 1
      db 2
    elseif 1
      db 3
    else
      db 4
    endif
";
    assert_eq!(binary(source), [2]);
}

#[test]
fn ifdef_and_ifndef_ask_the_symbol_table() {
    let source = "\
defined equ 1
    ifdef defined
      db 1
    endif
    ifdef absent
      db 2
    endif
    ifndef absent
      db 3
    endif
";
    assert_eq!(binary(source), [1, 3]);
}

#[test]
fn a_symbol_only_defined_inside_a_skipped_branch_is_not_defined() {
    let source = "\
    if 0
inside: db 1
    endif
    ifdef inside
      db 2
    endif
    db 3
";
    assert_eq!(binary(source), [3]);
}

#[test]
fn unbalanced_conditionals_are_reported() {
    assert_eq!(
        errors("    if 1\n    db 1\n"),
        ["this conditional is never closed"]
    );
    assert_eq!(errors("    endif\n"), ["`ENDIF` with no `IF`"]);
    assert_eq!(errors("    else\n"), ["`ELSE` with no `IF`"]);
}

#[test]
fn a_condition_on_a_forward_reference_settles_on_a_later_pass() {
    // Unresolvable on the first pass, so the branch is skipped; defining the
    // symbol moves the addresses, which is what asks for another pass.
    let source = "\
    if later = 1
      db $AA
    else
      db $BB
    endif
later equ 1
";
    let (_, assembled) = assemble_ok(source);
    assert_eq!(assembled.image.to_binary(), [0xAA]);
    assert!(assembled.passes >= 2);
}

// -- files ------------------------------------------------------------------

/// A fresh directory under the system temporary directory.
fn directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rkw-asm-test-{name}"));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("can create a temporary directory");
    path
}

fn write(path: &Path, name: &str, contents: &str) -> PathBuf {
    let file = path.join(name);
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).expect("can create a directory");
    }
    fs::write(&file, contents).expect("can write a file");
    file
}

/// Assemble a file from disk.
fn assemble_file(path: &Path) -> (SourceMap, rkw_asm::Assembled) {
    let mut map = SourceMap::new();
    let file = map.load(path).expect("can read the source");
    let assembled = assemble(&mut map, file);
    (map, assembled)
}

#[test]
fn include_resolves_relative_to_the_including_file() {
    // The point of the test: `other.asm` names `third.asm` without a path, and
    // it is in `sub/`, not next to the root file or in the working directory.
    let dir = directory("include-relative");
    write(
        &dir,
        "main.asm",
        "    db 1\n    include \"sub/other.asm\"\n    db 4\n",
    );
    write(
        &dir,
        "sub/other.asm",
        "    db 2\n    include \"third.asm\"\n",
    );
    write(&dir, "sub/third.asm", "    db 3\n");

    let (map, assembled) = assemble_file(&dir.join("main.asm"));
    assert!(
        assembled.diagnostics.is_empty(),
        "{}",
        map.render_all(&assembled.diagnostics)
    );
    assert_eq!(assembled.image.to_binary(), [1, 2, 3, 4]);
}

#[test]
fn a_symbol_from_an_included_file_is_usable_and_keeps_its_own_file_in_errors() {
    let dir = directory("include-symbols");
    write(
        &dir,
        "main.asm",
        "    include \"lib.asm\"\n    ld a,answer\n    db oops\n",
    );
    write(&dir, "lib.asm", "answer equ 42\noops   equ 300\n");

    let (map, assembled) = assemble_file(&dir.join("main.asm"));
    assert_eq!(assembled.image.byte_at(1), 42);
    // The value is out of range, and the error points at the use in main.asm
    // rather than at the definition in lib.asm.
    assert_eq!(
        assembled.diagnostics[0].message,
        "300 does not fit in one byte"
    );
    let reported = map.location(assembled.diagnostics[0].span);
    assert!(reported.file.ends_with("main.asm"), "{reported}");
}

/// `INCLUDE MORE.I` as well as `INCLUDE "MORE.I"`, both of which sjasmplus
/// documents. Ticket 0030 — the unquoted form was rejected outright, which is
/// the spelling most real sources use.
#[test]
fn an_include_filename_need_not_be_quoted() {
    let dir = directory("include-unquoted");
    write(&dir, "main.asm", "    db 1\n    include other.asm\n");
    write(&dir, "other.asm", "    db 2\n");

    let (map, assembled) = assemble_file(&dir.join("main.asm"));
    assert!(
        assembled.diagnostics.is_empty(),
        "{}",
        map.render_all(&assembled.diagnostics)
    );
    assert_eq!(assembled.image.to_binary(), [1, 2]);
}

#[test]
fn an_include_cycle_is_reported_with_its_chain() {
    let dir = directory("include-cycle");
    write(&dir, "a.asm", "    db 1\n    include \"b.asm\"\n");
    write(&dir, "b.asm", "    db 2\n    include \"a.asm\"\n");

    let (_, assembled) = assemble_file(&dir.join("a.asm"));
    let diagnostic = &assembled.diagnostics[0];
    assert!(
        diagnostic.message.ends_with("is already being included"),
        "{}",
        diagnostic.message
    );
    // The chain: a.asm, then b.asm which asked for it again.
    assert_eq!(diagnostic.related.len(), 2);
}

#[test]
fn a_missing_include_names_the_file_it_looked_for() {
    let dir = directory("include-missing");
    write(&dir, "main.asm", "    include \"nowhere.asm\"\n");

    let (_, assembled) = assemble_file(&dir.join("main.asm"));
    assert!(
        assembled.diagnostics[0]
            .message
            .starts_with("cannot read `"),
        "{}",
        assembled.diagnostics[0].message
    );
}

#[test]
fn incbin_takes_an_offset_and_a_length() {
    let dir = directory("incbin");
    fs::write(dir.join("data.bin"), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).expect("can write");
    write(&dir, "all.asm", "    incbin \"data.bin\"\n");
    write(&dir, "part.asm", "    incbin \"data.bin\",2,3\n");
    write(&dir, "rest.asm", "    incbin \"data.bin\",7\n");
    write(&dir, "toofar.asm", "    incbin \"data.bin\",2,99\n");

    assert_eq!(
        assemble_file(&dir.join("all.asm")).1.image.to_binary(),
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
    );
    assert_eq!(
        assemble_file(&dir.join("part.asm")).1.image.to_binary(),
        [2, 3, 4]
    );
    assert_eq!(
        assemble_file(&dir.join("rest.asm")).1.image.to_binary(),
        [7, 8, 9]
    );
    assert!(
        assemble_file(&dir.join("toofar.asm")).1.diagnostics[0]
            .message
            .ends_with("has 8 bytes after offset 2, not 99")
    );
}

// -- output -----------------------------------------------------------------

#[test]
fn end_stops_assembly_where_it_appears() {
    assert_eq!(binary("    db 1\n    end\n    db 2\n"), [1]);
}

#[test]
fn a_raw_binary_is_written_without_a_header() {
    let dir = directory("binary-output");
    let (_, assembled) = assemble_ok("    org $8000\n    db 1,2,3\n");
    let path = dir.join("out.bin");
    assembled.image.write_binary(&path).expect("can write");

    assert_eq!(fs::read(&path).expect("can read"), [1, 2, 3]);
    // The origin is not in the file, so it is reported separately.
    assert_eq!(assembled.image.origin(), Some(0x8000));
}

#[test]
fn every_statement_that_emitted_bytes_records_where_they_went() {
    // The raw material for the listing and the debug info in ticket 0006.
    let (map, assembled) = assemble_ok("    org $8000\nstart:\n    ld hl,$1234\n    db 1,2,3\n");
    let described: Vec<(u32, u16, u16)> = assembled
        .lines
        .iter()
        .map(|line| (map.location(line.span).line, line.address, line.length))
        .collect();
    assert_eq!(described, [(3, 0x8000, 3), (4, 0x8003, 3)]);
}
