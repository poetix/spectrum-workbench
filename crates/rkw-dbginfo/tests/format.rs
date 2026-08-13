//! The file format, from the reading side.
//!
//! Written against text rather than against something assembled, because that
//! is the position the debugger is in: it is handed a file by a program it did
//! not run and has to decide what it says.

use rkw_dbginfo::DebugInfo;

#[test]
fn a_version_it_does_not_know_is_refused_rather_than_guessed_at() {
    let error = DebugInfo::parse("rkw-debug\t99\n").expect_err("refuses");
    assert!(error.contains("version 99"), "{error}");

    let error = DebugInfo::parse("something else\n").expect_err("refuses");
    assert!(error.contains("rkw-debug"), "{error}");
}

#[test]
fn an_unknown_record_is_ignored_so_that_adding_one_stays_compatible() {
    let text = concat!(
        "rkw-debug\t1\n",
        "# a comment\n",
        "\n",
        "file\t0\tmain.asm\n",
        "kind\tsomething\tthis reader has never heard of\n",
        "line\t32768\t1\t0\t2\t9\t-\n",
    );
    let info = DebugInfo::parse(text).expect("reads what it knows");
    assert_eq!(info.files, ["main.asm"]);
    assert_eq!(info.lines.len(), 1);
}

#[test]
fn names_containing_tabs_survive() {
    let text = concat!(
        "rkw-debug\t1\n",
        "file\t0\tawkward\\tname.asm\n",
        "symbol\ttab\\there\t1\tconstant\t0\t1\t1\n",
    );
    let info = DebugInfo::parse(text).expect("reads back");
    assert_eq!(info.files, ["awkward\tname.asm"]);
    assert_eq!(info.symbols[0].name, "tab\there");
    // And writing it puts the escapes back.
    assert_eq!(DebugInfo::parse(&info.to_text()).unwrap(), info);
}

#[test]
fn a_malformed_record_says_which_line_is_wrong() {
    let text = "rkw-debug\t1\nfile\t0\tmain.asm\nline\t32768\tnonsense\t0\t2\t9\t-\n";
    let error = DebugInfo::parse(text).expect_err("refuses");
    assert_eq!(error, "line 3: `nonsense` is not a number");
}

#[test]
fn records_are_indexed_however_they_arrived() {
    // A producer is not obliged to sort, so the reader does: both lookups have
    // to work on a file written in whatever order the records were collected.
    let text = concat!(
        "rkw-debug\t1\n",
        "file\t0\tmain.asm\n",
        "line\t32771\t2\t0\t3\t9\t-\n",
        "line\t32768\t3\t0\t2\t9\t-\n",
        "symbol\twidth\t256\tconstant\t0\t1\t1\n",
        "symbol\tmain\t32768\tlabel\t0\t2\t1\n",
    );
    let info = DebugInfo::parse(text).expect("reads back");

    assert_eq!(info.line_at(0x8002).expect("covered").at.line, 2);
    assert_eq!(info.line_at(0x8003).expect("covered").at.line, 3);
    assert_eq!(info.addresses_of(0, 3), [0x8003]);
    assert_eq!(info.symbol("main").expect("defined").value, 0x8000);
    assert_eq!(info.symbol("width").expect("defined").value, 256);
}
