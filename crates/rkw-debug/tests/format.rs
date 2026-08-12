//! The formatter, against outcomes built by hand.
//!
//! There is no machine in this file. That is the whole claim of the three-way
//! split: if rendering needed an emulator to exercise it, the executor would be
//! returning something less than a value.

use rkw_debug::breakpoints::{Breakpoint, PortWatch, Watchpoint};
use rkw_debug::cmd::exec::{Armed, ArmedList, Disassembly, MemoryDump, RegisterView, Stop};
use rkw_debug::cmd::format::{self, render};
use rkw_debug::cmd::parse::{Format, Unit};
use rkw_debug::cmd::{Outcome, parse};
use rkw_debug::condition::{Cmp, Condition, Operand};
use rkw_debug::{Access, Request, StopReason};
use z80::disasm::{Flow, Instruction};
use z80::{Reg8, Reg16, Regs, flag};

fn instruction(addr: u16, bytes: &[u8], text: &str) -> Instruction {
    Instruction {
        addr,
        len: bytes.len() as u8,
        bytes: bytes.to_vec(),
        text: text.into(),
        flow: Flow::Normal,
        undocumented: false,
    }
}

fn stop(reason: StopReason) -> Stop {
    Stop {
        reason,
        pc: 0x8002,
        t: 1234,
        instructions: 7,
        next: instruction(0x8002, &[0x00], "NOP"),
    }
}

#[test]
fn a_stop_says_why_where_and_when() {
    assert_eq!(
        render(&Outcome::Stopped(stop(StopReason::Breakpoint {
            id: 1,
            addr: 0x8002
        }))),
        "Breakpoint 1 at $8002\n\
         => 8002  00           NOP\n   \
            T=1234 after 7 instructions"
    );
}

#[test]
fn a_watchpoint_stop_shows_the_byte_on_both_sides_of_the_write() {
    let text = render(&Outcome::Stopped(stop(StopReason::Watchpoint {
        id: 2,
        addr: 0x4000,
        access: Access::Write,
        old: 0x00,
        new: 0xFF,
    })));
    assert!(
        text.starts_with("Watchpoint 2 at $4000: write $00 -> $FF"),
        "{text}"
    );
}

#[test]
fn a_step_says_nothing_beyond_where_it_landed() {
    let text = render(&Outcome::Stopped(stop(StopReason::Step)));
    assert!(text.starts_with("=> 8002"), "{text}");
}

#[test]
fn the_run_limit_says_the_machine_is_still_there() {
    let text = render(&Outcome::Stopped(stop(StopReason::OutOfBudget)));
    assert!(text.starts_with("Run limit reached"), "{text}");
}

#[test]
fn the_flag_byte_is_shown_as_its_eight_bits_undocumented_ones_included() {
    assert_eq!(format::flags(0xFF), "SZYHXPNC");
    assert_eq!(format::flags(0x00), "--------");
    assert_eq!(format::flags(flag::Z | flag::C), "-Z-----C");
    assert_eq!(
        format::flags(flag::X | flag::Y),
        "--Y-X---",
        "the undocumented bits are shown, not hidden"
    );
}

#[test]
fn the_register_view_shows_the_shadow_set_wz_and_the_interrupt_state() {
    let mut regs = Regs::default();
    regs.set_af(0x2A44);
    regs.set_bc(0x0003);
    regs.pc = 0x8005;
    regs.sp = 0xFF00;
    regs.wz = 0x8002;
    regs.iff1 = true;
    regs.im = z80::InterruptMode::Im2;
    let text = render(&Outcome::Registers(RegisterView {
        regs,
        t: 99,
        instructions: 5,
    }));

    assert!(text.contains("AF=2A44 [-Z---P--]"), "{text}");
    assert!(text.contains("AF'=FFFF"), "{text}");
    assert!(text.contains("WZ=8002"), "{text}");
    assert!(text.contains("IM2  IFF1=1  IFF2=0"), "{text}");
    assert!(text.ends_with("T=99 after 5 instructions"), "{text}");
}

#[test]
fn a_hex_dump_is_rows_of_sixteen_with_the_text_beside_them() {
    let bytes: Vec<u8> = (0..20).collect();
    let text = render(&Outcome::Memory(MemoryDump {
        addr: 0x4000,
        unit: Unit::Byte,
        format: Format::Hex,
        bytes,
    }));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "$4000  00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F  |................|"
    );
    assert_eq!(
        lines[1], "$4010  10 11 12 13                                      |....|",
        "a short last row keeps its columns"
    );
}

#[test]
fn printable_bytes_show_in_the_gutter() {
    let text = render(&Outcome::Memory(MemoryDump {
        addr: 0x5C00,
        unit: Unit::Byte,
        format: Format::Hex,
        bytes: b"hello\x00".to_vec(),
    }));
    assert!(text.ends_with("|hello.|"), "{text}");
}

#[test]
fn the_width_of_a_dump_follows_its_unit() {
    let text = render(&Outcome::Memory(MemoryDump {
        addr: 0x4000,
        unit: Unit::Word,
        format: Format::Hex,
        bytes: vec![0x00, 0x40, 0x34, 0x12],
    }));
    assert_eq!(text, "$4000  4000 1234", "little-endian words, as read");
}

#[test]
fn decimal_is_signed_and_unsigned_is_not() {
    let dump = |format| {
        render(&Outcome::Memory(MemoryDump {
            addr: 0x4000,
            unit: Unit::Byte,
            format,
            bytes: vec![0xFF],
        }))
    };
    assert_eq!(dump(Format::Dec), "$4000    -1");
    assert_eq!(dump(Format::Unsigned), "$4000  255");
    assert_eq!(dump(Format::Binary), "$4000  11111111");
}

#[test]
fn disassembly_marks_pc_and_only_pc() {
    let text = render(&Outcome::Disassembly(Disassembly {
        pc: 0x8002,
        instructions: vec![
            instruction(0x8000, &[0x3E, 0x2A], "LD A,$2A"),
            instruction(0x8002, &[0x00], "NOP"),
        ],
    }));
    assert_eq!(
        text,
        "   8000  3E 2A        LD A,$2A\n\
         => 8002  00           NOP"
    );
}

#[test]
fn an_armed_breakpoint_reads_back_as_what_was_typed() {
    let breakpoint = Breakpoint {
        id: 1,
        addr: 0x8002,
        enabled: true,
        condition: Some(Condition::reg8_eq(Reg8::A, 0x2A)),
        hits: 0,
        ignore: 0,
    };
    assert_eq!(
        render(&Outcome::Armed(Armed::Breakpoint(breakpoint))),
        "Breakpoint 1 at $8002 if a == $2A"
    );
}

#[test]
fn watches_say_which_accesses_they_are_on() {
    let watchpoint = Watchpoint {
        id: 2,
        addr: 0x4000,
        enabled: true,
        on_read: true,
        on_write: false,
        on_change_only: false,
        hits: 0,
    };
    assert_eq!(
        render(&Outcome::Armed(Armed::Watchpoint(watchpoint))),
        "Watchpoint 2 at $4000 on read"
    );

    let port = PortWatch {
        id: 3,
        mask: 0x00FF,
        value: 0x00FE,
        enabled: true,
        on_in: true,
        on_out: false,
        hits: 0,
    };
    assert_eq!(
        render(&Outcome::Armed(Armed::PortWatch(port))),
        "Port watchpoint 3 on read matching $00FE/$00FF"
    );
}

#[test]
fn the_armed_list_is_one_table_in_id_order() {
    let list = ArmedList {
        breakpoints: vec![Breakpoint {
            id: 1,
            addr: 0x8002,
            enabled: true,
            condition: None,
            hits: 3,
            ignore: 0,
        }],
        watchpoints: vec![Watchpoint {
            id: 2,
            addr: 0x4000,
            enabled: false,
            on_read: false,
            on_write: true,
            on_change_only: false,
            hits: 0,
        }],
        ports: vec![],
    };
    let text = render(&Outcome::List(list));
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("Num  Type"), "{text}");
    assert!(lines[1].starts_with("1    breakpoint  $8002"), "{text}");
    assert!(lines[1].contains("3     y"), "hits and enabled: {text}");
    assert!(lines[2].contains("n    write"), "{text}");

    assert_eq!(
        render(&Outcome::List(ArmedList::default())),
        "Nothing is armed."
    );
}

#[test]
fn deletions_name_what_went() {
    assert_eq!(render(&Outcome::Removed(vec![1, 3])), "Deleted 1, 3.");
    assert_eq!(render(&Outcome::Removed(vec![])), "Nothing was armed.");
    assert_eq!(
        render(&Outcome::Enabled {
            ids: vec![2],
            enabled: false
        }),
        "Disabled 2."
    );
}

#[test]
fn a_poke_shows_the_byte_it_replaced() {
    assert_eq!(
        render(&Outcome::Poked {
            addr: 0x4000,
            old: 0x00,
            new: 0xFF
        }),
        "$4000: $00 -> $FF"
    );
}

#[test]
fn a_parse_error_points_at_the_column_it_came_from() {
    let line = "break $8000 if a == zog";
    let error = parse(line).unwrap_err();
    assert_eq!(
        format::parse_error(line, &error),
        "error: `zog` is not a register, flag or number\n  \
         break $8000 if a == zog\n  \
         \x20                   ^"
    );
}

/// Every condition the parser can build has to print as something the parser
/// accepts, or `info breakpoints` is showing a condition that cannot be typed
/// back in.
#[test]
fn a_rendered_condition_parses_back_to_itself() {
    let conditions = [
        Condition::reg8_eq(Reg8::A, 0x2A),
        Condition::reg16_eq(Reg16::Hl, 0x4000),
        Condition::cmp(Operand::Flag(flag::Z), Cmp::Eq, Operand::Imm(1)),
        Condition::cmp(Operand::Flag(flag::P), Cmp::Ne, Operand::Imm(0)),
        Condition::cmp(Operand::Mem8At(Reg16::Hl), Cmp::Ge, Operand::Mem8(0x5C00)),
        Condition::cmp(Operand::Mem16(0x5C00), Cmp::Lt, Operand::Reg16(Reg16::Sp)),
        Condition::cmp(Operand::Reg8(Reg8::Ixh), Cmp::Le, Operand::Imm(0xFF)),
        Condition::All(vec![
            Condition::reg8_eq(Reg8::B, 1),
            Condition::Any(vec![
                Condition::reg8_eq(Reg8::C, 2),
                Condition::Not(Box::new(Condition::reg8_eq(Reg8::D, 3))),
            ]),
        ]),
    ];

    for condition in conditions {
        let line = format!("break $8000 if {}", format::condition(&condition));
        let parsed = parse(&line)
            .unwrap_or_else(|e| panic!("{line}: {}", e.message))
            .expect("a command");
        assert_eq!(
            parsed,
            Request::Break {
                addr: rkw_debug::cmd::Addr::abs(0x8000),
                condition: Some(condition),
            },
            "{line}"
        );
    }
}

#[test]
fn help_lists_every_command_the_parser_knows() {
    let text = render(&Outcome::Help(rkw_debug::cmd::parse::HELP));
    for name in [
        "break", "watch", "finish", "x/NFU", "disas", "trace", "reset",
    ] {
        assert!(text.contains(name), "{name} is missing from help");
    }
}
