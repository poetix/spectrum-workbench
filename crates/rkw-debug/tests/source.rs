//! Source-level debugging through the command layer: breaking on a line,
//! naming addresses by symbol, and knowing where a stop is in source.
//!
//! Written against a hand-made sidecar rather than against the assembler,
//! because that is the debugger's actual position — it is handed a file by a
//! program it did not run — and because it keeps this crate's tests free of a
//! dependency on the toolchain that produced the program (ADR-0019).

mod common;

use common::{ORG, STACK};
use rkw_debug::StopReason;
use rkw_debug::cmd::exec::{Armed, ExecError, Outcome, Session};
// Through the command layer's own re-exports: a front end depends on one
// crate, not on the format's as well.
use rkw_debug::cmd::{DebugInfo, Error, ResolveError, Sources};
use rkw_debug::emu::Config;
use z80::{Cpu, FlatMemory};

/// ```text
/// 8000  3E 2A     LD A,42      main.asm:4
/// 8002  00        NOP          mac.asm:3, inside `twice` invoked at main.asm:6
/// 8003  00        NOP          mac.asm:3, inside `twice` invoked at main.asm:7
/// 8004  76        HALT         main.asm:9
/// ```
const PROGRAM: &[u8] = &[0x3E, 0x2A, 0x00, 0x00, 0x76];

const SIDECAR: &str = concat!(
    "rkw-debug\t1\n",
    "file\t0\tsrc/main.asm\n",
    "file\t1\tsrc/mac.asm\n",
    "expansion\t0\ttwice\t0\t6\t9\t1\t2\t1\t-\n",
    "expansion\t1\ttwice\t0\t7\t9\t1\t2\t1\t-\n",
    "line\t32768\t2\t0\t4\t9\t-\n",
    "line\t32770\t1\t1\t3\t9\t0\n",
    "line\t32771\t1\t1\t3\t9\t1\n",
    "line\t32772\t1\t0\t9\t9\t-\n",
    "symbol\tdone\t32772\tlabel\t0\t9\t1\n",
    "symbol\tmain\t32768\tlabel\t0\t4\t1\n",
    "symbol\twidth\t70000\tconstant\t0\t2\t1\n",
);

const MAIN_ASM: &str = "\
; a program
width   equ 70000

main:   ld a,42

        twice
        twice

done:   halt
";

const MAC_ASM: &str = "\
twice   macro
        ; twice over
        nop
        endm
";

fn sources() -> Sources {
    let mut sources = Sources::new(DebugInfo::parse(SIDECAR).expect("well-formed"));
    sources.set_text_of("src/main.asm", MAIN_ASM);
    sources.set_text_of("src/mac.asm", MAC_ASM);
    sources
}

fn bare() -> Session<FlatMemory> {
    let mut mem = FlatMemory::new();
    mem.load(ORG, PROGRAM);
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    cpu.regs.sp = STACK;
    Session::new(cpu, mem, Config::default())
}

fn session() -> Session<FlatMemory> {
    let mut session = bare();
    session.set_sources(sources());
    session
}

fn run(session: &mut Session<FlatMemory>, line: &str) -> Outcome {
    session
        .run_line(line)
        .unwrap_or_else(|e| panic!("{line:?}: {e}"))
        .unwrap_or_else(|| panic!("{line:?} did nothing"))
}

fn failure(session: &mut Session<FlatMemory>, line: &str) -> ExecError {
    match session.run_line(line) {
        Err(Error::Exec(e)) => e,
        other => panic!("{line:?} returned {other:?}"),
    }
}

#[test]
fn a_breakpoint_on_a_line_in_a_macro_is_one_per_expansion() {
    let mut s = session();
    let Outcome::Armed(Armed::Source { site, breakpoints }) = run(&mut s, "break mac.asm:3") else {
        panic!("not a source breakpoint");
    };
    assert_eq!(site.addresses, [0x8002, 0x8003]);
    assert_eq!(site.line, 3);
    assert!(!site.moved());
    assert_eq!(
        breakpoints
            .iter()
            .map(|b| (b.id, b.addr))
            .collect::<Vec<_>>(),
        [(1, 0x8002), (2, 0x8003)],
        "one breakpoint per address the line produced"
    );

    // And both of them fire, which is the whole reason for arming both.
    let Outcome::Stopped(first) = run(&mut s, "continue") else {
        panic!("not a stop");
    };
    assert_eq!(
        first.reason,
        StopReason::Breakpoint {
            id: 1,
            addr: 0x8002
        }
    );
    let Outcome::Stopped(second) = run(&mut s, "continue") else {
        panic!("not a stop");
    };
    assert_eq!(
        second.reason,
        StopReason::Breakpoint {
            id: 2,
            addr: 0x8003
        }
    );
}

#[test]
fn a_stop_knows_where_it_is_in_source_and_how_it_got_there() {
    let mut s = session();
    run(&mut s, "break mac.asm:3");
    let Outcome::Stopped(first) = run(&mut s, "continue") else {
        panic!("not a stop");
    };
    let at = first.at.expect("debug info covers $8002");
    assert_eq!((at.file.as_str(), at.line), ("src/mac.asm", 3));
    assert_eq!(at.text.as_deref(), Some("        nop"));
    assert!(!at.stale);

    // The body of the macro is written once. Which invocation reached it is
    // what the reader cannot work out from the text.
    assert_eq!(at.frames.len(), 1);
    assert_eq!(at.frames[0].name, "twice");
    assert_eq!(
        (at.frames[0].file.as_str(), at.frames[0].line),
        ("src/main.asm", 6)
    );

    let Outcome::Stopped(second) = run(&mut s, "continue") else {
        panic!("not a stop");
    };
    assert_eq!(second.at.expect("covered").frames[0].line, 7);
}

#[test]
fn a_line_that_produced_nothing_arms_the_next_one_that_did_and_says_so() {
    let mut s = session();
    // Line 5 of main.asm is blank, and lines 6 and 7 produced their bytes
    // inside the macro rather than at themselves. The next line of this file
    // that produced anything is the `halt` on line 9.
    let Outcome::Armed(Armed::Source { site, breakpoints }) = run(&mut s, "break src/main.asm:5")
    else {
        panic!("not a source breakpoint");
    };
    assert_eq!((site.requested, site.line), (5, 9));
    assert!(site.moved());
    assert_eq!(
        breakpoints.iter().map(|b| b.addr).collect::<Vec<_>>(),
        [0x8004]
    );
}

#[test]
fn a_symbol_is_an_address_anywhere_an_address_is() {
    let mut s = session();
    let Outcome::Armed(Armed::Breakpoint(bp)) = run(&mut s, "break done") else {
        panic!("not a breakpoint");
    };
    assert_eq!(bp.addr, 0x8004);

    // Offsets work from a symbol as they do from a register.
    let Outcome::Memory(dump) = run(&mut s, "x/2 main+2") else {
        panic!("not a dump");
    };
    assert_eq!(
        (dump.addr, dump.bytes.as_slice()),
        (0x8002, &[0x00, 0x00][..])
    );

    let Outcome::Disassembly(dis) = run(&mut s, "disas main 1") else {
        panic!("not a disassembly");
    };
    assert_eq!(dis.instructions[0].addr, 0x8000);

    let Outcome::Stopped(stop) = run(&mut s, "until done") else {
        panic!("not a stop");
    };
    assert_eq!(stop.pc, 0x8004);
}

#[test]
fn a_name_that_does_not_resolve_says_which_name_and_why() {
    let mut s = session();
    assert_eq!(
        failure(&mut s, "break absent"),
        ExecError::Unresolved(ResolveError::UnknownSymbol("absent".into()))
    );
    // A constant that does not fit in an address is not one.
    assert_eq!(
        failure(&mut s, "break width"),
        ExecError::Unresolved(ResolveError::NotAnAddress {
            name: "width".into(),
            value: 70000
        })
    );
    assert_eq!(
        failure(&mut s, "break nowhere.asm:1"),
        ExecError::Unresolved(ResolveError::UnknownFile("nowhere.asm".into()))
    );
    assert_eq!(
        failure(&mut s, "break main.asm:99"),
        ExecError::Unresolved(ResolveError::NoCode {
            file: "src/main.asm".into(),
            line: 99
        })
    );
}

#[test]
fn without_debug_info_a_name_says_that_rather_than_failing_to_find_it() {
    let mut s = bare();
    assert_eq!(failure(&mut s, "break main"), ExecError::NoDebugInfo);
    assert_eq!(failure(&mut s, "break main.asm:4"), ExecError::NoDebugInfo);
    assert_eq!(failure(&mut s, "list"), ExecError::NoDebugInfo);

    // Everything that was there before ticket 0011 still is.
    let Outcome::Armed(Armed::Breakpoint(bp)) = run(&mut s, "break $8004") else {
        panic!("not a breakpoint");
    };
    assert_eq!(bp.addr, 0x8004);
    assert!(matches!(run(&mut s, "continue"), Outcome::Stopped(_)));
}

#[test]
fn list_shows_source_around_the_current_position() {
    let mut s = session();
    let Outcome::Source(listing) = run(&mut s, "list") else {
        panic!("not a listing");
    };
    // PC is $8000, which came from main.asm:4.
    assert_eq!(listing.file, "src/main.asm");
    assert_eq!(listing.current, Some(4));
    assert_eq!(listing.lines[0], "; a program");
    assert!(!listing.stale);

    // A place of its own: an address, a symbol, or a line.
    let Outcome::Source(by_line) = run(&mut s, "list mac.asm:2") else {
        panic!("not a listing");
    };
    assert_eq!(
        (by_line.file.as_str(), by_line.current),
        ("src/mac.asm", Some(2))
    );

    let Outcome::Source(by_symbol) = run(&mut s, "list done") else {
        panic!("not a listing");
    };
    assert_eq!(by_symbol.current, Some(9));

    let Outcome::Source(by_address) = run(&mut s, "list $8003") else {
        panic!("not a listing");
    };
    assert_eq!(
        (by_address.file.as_str(), by_address.current),
        ("src/mac.asm", Some(3))
    );

    // Somewhere no source produced.
    assert_eq!(failure(&mut s, "list $9000"), ExecError::NoSourceAt(0x9000));
}
