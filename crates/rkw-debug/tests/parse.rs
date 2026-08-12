//! The command grammar, as a pure function.
//!
//! Every one of these runs without a machine, which is the point of the
//! parser being a library rather than a switch inside a read loop: the shapes
//! that are awkward to get right — a condition with precedence in it, a `/`
//! spec glued to its command, an address relative to a register — are checked
//! here and not by typing at a prompt.

use rkw_debug::cmd::parse::{Base, Format, Info, Unit};
use rkw_debug::cmd::{Addr, Request, parse};
use rkw_debug::condition::{Cmp, Condition, Operand};
use z80::{Reg8, Reg16, flag};

fn parsed(line: &str) -> Request {
    parse(line)
        .unwrap_or_else(|e| panic!("{line:?} did not parse: {}", e.message))
        .unwrap_or_else(|| panic!("{line:?} parsed to nothing"))
}

fn error(line: &str) -> String {
    match parse(line) {
        Err(e) => e.message,
        Ok(other) => panic!("{line:?} parsed to {other:?}"),
    }
}

#[test]
fn blank_lines_and_comments_are_not_commands() {
    assert_eq!(parse(""), Ok(None));
    assert_eq!(parse("   "), Ok(None));
    assert_eq!(parse("; a comment"), Ok(None));
    assert_eq!(parse("# also a comment"), Ok(None));
    assert_eq!(parse("step ; with a comment"), Ok(Some(Request::Step(1))));
}

#[test]
fn numbers_are_written_the_four_ways_they_are_written() {
    for line in [
        "break $8000",
        "break 0x8000",
        "break 32768",
        "break %1000000000000000",
    ] {
        assert_eq!(
            parsed(line),
            Request::Break {
                addr: Addr::abs(0x8000),
                condition: None,
            },
            "{line}"
        );
    }
}

#[test]
fn an_address_can_be_a_register_with_an_offset() {
    assert_eq!(
        parsed("disas pc-8 4"),
        Request::Disas {
            addr: Some(Addr {
                base: Base::Pc,
                offset: -8
            }),
            count: 4,
        }
    );
    assert_eq!(
        parsed("x/4 hl+2"),
        Request::Examine {
            addr: Addr {
                base: Base::Reg(Reg16::Hl),
                offset: 2
            },
            count: 4,
            format: Format::Hex,
            unit: Unit::Byte,
        }
    );
}

#[test]
fn the_examine_spec_reads_as_a_count_a_format_and_a_unit() {
    let Request::Examine {
        count,
        format,
        unit,
        ..
    } = parsed("x/8tw $4000")
    else {
        panic!("not an examine");
    };
    assert_eq!((count, format, unit), (8, Format::Binary, Unit::Word));

    let Request::Examine { count, format, .. } = parsed("x/s $5C00") else {
        panic!("not an examine");
    };
    assert_eq!((count, format), (1, Format::Str), "a string count defaults");
}

#[test]
fn a_bare_examine_has_defaults() {
    assert_eq!(
        parsed("x $4000"),
        Request::Examine {
            addr: Addr::abs(0x4000),
            count: 16,
            format: Format::Hex,
            unit: Unit::Byte,
        }
    );
}

#[test]
fn movements_take_a_count_and_default_to_one() {
    assert_eq!(parsed("step"), Request::Step(1));
    assert_eq!(parsed("s 5"), Request::Step(5));
    assert_eq!(parsed("next"), Request::Next(1));
    assert_eq!(parsed("ni 3"), Request::Next(3));
    assert_eq!(parsed("finish"), Request::Finish);
    assert_eq!(parsed("c"), Request::Continue);
    assert_eq!(parsed("until $8010"), Request::Until(Addr::abs(0x8010)));
    assert_eq!(parsed("run"), Request::Run(None));
    assert_eq!(parsed("run $8000"), Request::Run(Some(Addr::abs(0x8000))));
    assert_eq!(parsed("reset"), Request::Reset);
}

#[test]
fn watches_say_which_side_of_the_access_they_are_on() {
    assert_eq!(
        parsed("watch $4000"),
        Request::Watch {
            addr: Addr::abs(0x4000),
            read: false,
            write: true,
        },
        "gdb's watch is a write watch"
    );
    assert_eq!(
        parsed("rwatch $4000"),
        Request::Watch {
            addr: Addr::abs(0x4000),
            read: true,
            write: false,
        }
    );
    assert_eq!(
        parsed("awatch $4000"),
        Request::Watch {
            addr: Addr::abs(0x4000),
            read: true,
            write: true,
        }
    );
}

#[test]
fn a_port_watch_defaults_to_full_decoding_and_both_directions() {
    assert_eq!(
        parsed("pwatch $FE"),
        Request::PortWatch {
            value: 0xFE,
            mask: 0xFFFF,
            on_in: true,
            on_out: true,
        }
    );
    assert_eq!(
        parsed("pwatch $00FE $00FF out"),
        Request::PortWatch {
            value: 0x00FE,
            mask: 0x00FF,
            on_in: false,
            on_out: true,
        },
        "the mask is how partial decoding is said"
    );
}

#[test]
fn delete_with_no_ids_means_everything() {
    assert_eq!(parsed("delete"), Request::Delete(vec![]));
    assert_eq!(parsed("delete 1 2 3"), Request::Delete(vec![1, 2, 3]));
    assert_eq!(parsed("delete 1, 2"), Request::Delete(vec![1, 2]));
    assert_eq!(parsed("enable 2"), Request::Enable(vec![2]));
    assert_eq!(parsed("disable"), Request::Disable(vec![]));
}

#[test]
fn info_picks_between_the_two_things_worth_asking() {
    assert_eq!(parsed("info breakpoints"), Request::Info(Info::Breakpoints));
    assert_eq!(parsed("i b"), Request::Info(Info::Breakpoints));
    assert_eq!(parsed("info registers"), Request::Info(Info::Registers));
    assert_eq!(parsed("info"), Request::Info(Info::Breakpoints));
}

#[test]
fn a_source_path_is_taken_from_the_raw_line() {
    // None of these survive the lexer, which is why `source` is recognised
    // before lexing.
    assert_eq!(
        parsed("source ../scripts/boot.rkw"),
        Request::Source("../scripts/boot.rkw".into())
    );
    assert_eq!(
        parsed("source  a file with spaces.txt"),
        Request::Source("a file with spaces.txt".into())
    );
    assert_eq!(error("source"), "expected a file name");
}

#[test]
fn a_condition_is_a_comparison() {
    assert_eq!(
        parsed("break $8002 if a == $2A"),
        Request::Break {
            addr: Addr::abs(0x8002),
            condition: Some(Condition::reg8_eq(Reg8::A, 0x2A)),
        }
    );
}

#[test]
fn conditions_combine_with_and_or_and_not() {
    let Request::Break { condition, .. } = parsed("break $8000 if a > 1 && hl != $4000") else {
        panic!("not a break");
    };
    assert_eq!(
        condition,
        Some(Condition::All(vec![
            Condition::cmp(Operand::Reg8(Reg8::A), Cmp::Gt, Operand::Imm(1)),
            Condition::cmp(Operand::Reg16(Reg16::Hl), Cmp::Ne, Operand::Imm(0x4000)),
        ]))
    );

    let Request::Break { condition, .. } = parsed("break $8000 if !(f.z == 1 || b == 0)") else {
        panic!("not a break");
    };
    assert_eq!(
        condition,
        Some(Condition::Not(Box::new(Condition::Any(vec![
            Condition::cmp(Operand::Flag(flag::Z), Cmp::Eq, Operand::Imm(1)),
            Condition::reg8_eq(Reg8::B, 0),
        ]))))
    );
}

#[test]
fn memory_in_a_condition_is_square_brackets_so_parentheses_can_group() {
    let Request::Break { condition, .. } = parsed("break $8000 if [hl] == [$5C00]") else {
        panic!("not a break");
    };
    assert_eq!(
        condition,
        Some(Condition::cmp(
            Operand::Mem8At(Reg16::Hl),
            Cmp::Eq,
            Operand::Mem8(0x5C00)
        ))
    );

    let Request::Break { condition, .. } = parsed("break $8000 if w[$5C00] > $8000") else {
        panic!("not a break");
    };
    assert_eq!(
        condition,
        Some(Condition::cmp(
            Operand::Mem16(0x5C00),
            Cmp::Gt,
            Operand::Imm(0x8000)
        ))
    );
}

#[test]
fn a_flag_is_never_spelled_like_a_register() {
    // `c` is the C register; `f.c` is the carry flag. Nothing resolves one to
    // the other, because a condition that quietly meant the wrong one would be
    // invisible.
    let Request::Break { condition, .. } = parsed("break $8000 if c == 1") else {
        panic!("not a break");
    };
    assert_eq!(condition, Some(Condition::reg8_eq(Reg8::C, 1)));

    let Request::Break { condition, .. } = parsed("break $8000 if f.c == 1") else {
        panic!("not a break");
    };
    assert_eq!(
        condition,
        Some(Condition::cmp(
            Operand::Flag(flag::C),
            Cmp::Eq,
            Operand::Imm(1)
        ))
    );
}

#[test]
fn errors_say_what_was_wrong_and_point_at_it() {
    assert_eq!(error("frobnicate"), "unknown command `frobnicate`");
    assert_eq!(error("break"), "expected an address");
    assert_eq!(error("break $10000"), "65536 does not fit in 16 bits");
    assert_eq!(
        error("break $8000 if"),
        "expected a register, flag, number or `[address]`"
    );
    assert_eq!(error("break $8000 if a"), "expected a comparison");
    assert_eq!(
        error("x/8q $4000").split(':').next(),
        Some("unknown format letter `q`")
    );
    assert_eq!(error("step 0"), "a count of zero does nothing");
    assert_eq!(error("delete 0"), "breakpoint ids start at 1");
    assert_eq!(error("poke $4000 $100"), "$0100 is not a byte");
    assert_eq!(error("regs please"), "trailing input");
    assert_eq!(error("break @"), "unexpected character `@`");
}

#[test]
fn the_caret_lands_on_the_offending_token() {
    let error = parse("break $8000 if a == zog").unwrap_err();
    assert_eq!(error.column, 20, "the column of `zog`");
    let error = parse("x/8q $4000").unwrap_err();
    assert_eq!(
        error.column, 3,
        "the column of the bad letter, not the spec"
    );
}
