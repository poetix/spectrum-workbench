//! Debug information: both query directions, and the file format.

mod common;

use common::assemble_ok;
use rkw_asm::debug::{self, DebugInfo, Kind};

fn info(source: &str) -> DebugInfo {
    let (map, mut assembled) = assemble_ok(source);
    debug::info(&map, &mut assembled)
}

#[test]
fn an_address_maps_back_to_the_line_that_produced_it() {
    let source = "\
        org $8000
start:  ld hl,$1234
        nop
";
    let info = info(source);

    // Every byte of a three-byte instruction belongs to the line that wrote it,
    // not only its first.
    for address in 0x8000..=0x8002 {
        let line = info.line_at(address).expect("covered");
        assert_eq!(line.at.line, 2, "address {address:04X}");
        assert_eq!(line.address, 0x8000);
        assert_eq!(line.length, 3);
    }
    assert_eq!(info.line_at(0x8003).expect("covered").at.line, 3);

    // Outside the program there is nothing to say.
    assert!(info.line_at(0x7FFF).is_none());
    assert!(info.line_at(0x8004).is_none());
}

#[test]
fn a_line_maps_to_every_address_it_produced() {
    // The one-to-many direction: the `db` inside the macro is one line of
    // source and three places in memory, and a breakpoint on it means all
    // three.
    let source = "\
        org $8000
mark    macro value
        db value
        endm
        mark 1
        mark 2
        mark 3
";
    let info = info(source);
    let file = info.file_index("program.asm").expect("the file is listed");

    assert_eq!(info.addresses_of(file, 3), [0x8000, 0x8001, 0x8002]);
    // The invocation lines produced no bytes of their own.
    assert_eq!(info.addresses_of(file, 5), []);
    assert_eq!(info.addresses_of(file, 99), []);
}

#[test]
fn each_address_knows_which_expansion_it_came_from() {
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
    let info = info(source);

    let first = info.line_at(0x8000).expect("covered");
    let expansion = first.expansion.expect("inside a macro");
    let chain = info.expansion_chain(expansion);
    // Innermost first: `inner`, then the `outer` that called it.
    assert_eq!(
        chain.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["inner", "outer"]
    );
    assert_eq!(chain[0].defined_at.line, 2);
    assert_eq!(chain[0].invoked_at.line, 6);
    assert_eq!(chain[1].invoked_at.line, 9);
}

#[test]
fn symbols_carry_their_kind_and_where_they_were_defined() {
    let source = "\
        org $8000
width   equ 32
start:  nop
count   defl 7
";
    let info = info(source);

    let start = info.symbol("start").expect("start is defined");
    assert_eq!(
        (start.value, start.kind, start.at.line),
        (0x8000, Kind::Label, 3)
    );

    let width = info.symbol("width").expect("width is defined");
    assert_eq!(
        (width.value, width.kind, width.at.line),
        (32, Kind::Constant, 2)
    );

    let count = info.symbol("count").expect("count is defined");
    assert_eq!(count.kind, Kind::Variable);

    assert!(info.symbol("absent").is_none());
}

#[test]
fn a_symbol_from_an_included_file_names_that_file() {
    // Positions are file-relative, so debug info from a program built out of
    // several files still points at the right one.
    let dir = std::env::temp_dir().join("rkw-asm-test-debug-include");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("can create a directory");
    std::fs::write(
        dir.join("main.asm"),
        "    org $8000\n    include \"lib.asm\"\n",
    )
    .expect("can write");
    std::fs::write(dir.join("lib.asm"), "helper: nop\n").expect("can write");

    let mut map = rkw_asm::SourceMap::new();
    let file = map.load(dir.join("main.asm")).expect("can read");
    let mut assembled = common::assemble(&mut map, file);
    assert!(assembled.diagnostics.is_empty());
    let info = debug::info(&map, &mut assembled);

    let helper = info.symbol("helper").expect("helper is defined");
    assert!(
        info.file_name(helper.at.file).ends_with("lib.asm"),
        "{}",
        info.file_name(helper.at.file)
    );
    assert_eq!(helper.at.line, 1);
    assert_eq!(
        info.line_at(0x8000).expect("covered").at.file,
        helper.at.file
    );
}

// -- the file format --------------------------------------------------------

#[test]
fn the_format_survives_a_round_trip() {
    let source = "\
        org $8000
pair    macro value
        db value,value
        endm
width   equ 32
start:  ld hl,$1234
        pair 7
";
    let original = info(source);
    let text = original.to_text();
    let parsed = DebugInfo::parse(&text).expect("reads back");

    assert_eq!(parsed.files, original.files);
    assert_eq!(parsed.lines, original.lines);
    assert_eq!(parsed.symbols, original.symbols);
    assert_eq!(parsed.expansions, original.expansions);
    // The derived index is rebuilt, not stored, so it has to come back too.
    let file = parsed
        .file_index("program.asm")
        .expect("the file is listed");
    assert_eq!(parsed.addresses_of(file, 3), original.addresses_of(file, 3));
}

#[test]
fn the_format_is_the_one_that_is_documented() {
    let (map, mut assembled) = assemble_ok("    org $8000\nstart:  nop\n");
    let info = debug::info(&map, &mut assembled);
    assert_eq!(
        info.to_text(),
        concat!(
            "rkw-debug\t1\n",
            "file\t0\tprogram.asm\n",
            "line\t32768\t1\t0\t2\t1\t-\n",
            "symbol\tstart\t32768\tlabel\t0\t2\t1\n",
        )
    );
}
