//! [`Outcome`] to text: the third of the command layer's three parts, and the
//! only one a GUI throws away.
//!
//! Nothing here reaches for a machine, and nothing upstream reaches for a
//! string. That is what makes this testable on its own — a formatter test
//! builds an [`Outcome`] and asserts on the lines, without an emulator
//! anywhere near it — and it is the same property that lets a DAP adapter use
//! the executor's results directly and this module only for its Debug Console
//! (ADR-0016).

use core::fmt::{self, Write as _};

use z80::{Reg8, Reg16, Regs, flag};

use super::exec::{
    Armed, ArmedList, Disassembly, MemoryDump, Outcome, RegisterView, Stop, StringAt, Trace,
};
use super::parse::{Format, ParseError, Unit};
use crate::StopReason;
use crate::breakpoints::{Access, Breakpoint, PortAccess, PortWatch, Watchpoint};
use crate::condition::{Cmp, Condition, Operand};

/// Render an outcome as the lines a terminal shows for it. No trailing
/// newline: what separates commands is the shell's business.
pub fn render(outcome: &Outcome) -> String {
    let mut out = String::new();
    write_outcome(&mut out, outcome).expect("writing to a String");
    out
}

pub fn write_outcome<W: fmt::Write>(w: &mut W, outcome: &Outcome) -> fmt::Result {
    match outcome {
        Outcome::Stopped(stop) => write_stop(w, stop),
        Outcome::Registers(view) => write_registers(w, view),
        Outcome::Reset(view) => {
            writeln!(w, "Reset.")?;
            write_registers(w, view)
        }
        Outcome::Memory(dump) => write_memory(w, dump),
        Outcome::Strings(strings) => write_strings(w, strings),
        Outcome::Disassembly(dis) => write_disassembly(w, dis),
        Outcome::Armed(armed) => write_armed(w, armed),
        Outcome::Removed(ids) => {
            if ids.is_empty() {
                return write!(w, "Nothing was armed.");
            }
            write!(w, "Deleted {}.", id_list(ids))
        }
        Outcome::Enabled { ids, enabled } => {
            if ids.is_empty() {
                return write!(w, "Nothing was armed.");
            }
            let verb = if *enabled { "Enabled" } else { "Disabled" };
            write!(w, "{verb} {}.", id_list(ids))
        }
        Outcome::List(list) => write_list(w, list),
        Outcome::Trace(trace) => write_trace(w, trace),
        Outcome::Poked { addr, old, new } => {
            write!(w, "${addr:04X}: ${old:02X} -> ${new:02X}")
        }
        Outcome::Script(path) => write!(w, "Reading commands from {path}"),
        Outcome::Help(entries) => write_help(w, entries),
        Outcome::Quit => Ok(()),
    }
}

/// A parse error against the line it came from, with a caret under it.
pub fn parse_error(line: &str, error: &ParseError) -> String {
    let column = error.column.min(line.len());
    format!(
        "error: {}\n  {}\n  {:>width$}",
        error.message,
        line,
        "^",
        width = column + 1
    )
}

// ---- Stops ---------------------------------------------------------------

fn write_stop<W: fmt::Write>(w: &mut W, stop: &Stop) -> fmt::Result {
    match stop.reason {
        StopReason::Step => {}
        StopReason::Breakpoint { id, addr } => {
            writeln!(w, "Breakpoint {id} at ${addr:04X}")?;
        }
        StopReason::Watchpoint {
            id,
            addr,
            access,
            old,
            new,
        } => match access {
            Access::Read => writeln!(w, "Watchpoint {id} at ${addr:04X}: read ${old:02X}")?,
            Access::Write => writeln!(
                w,
                "Watchpoint {id} at ${addr:04X}: write ${old:02X} -> ${new:02X}"
            )?,
        },
        StopReason::PortWatchpoint {
            id,
            port,
            access,
            value,
        } => {
            let (verb, arrow) = match access {
                PortAccess::In => ("IN", "->"),
                PortAccess::Out => ("OUT", "<-"),
            };
            writeln!(
                w,
                "Port watchpoint {id}: {verb} ${port:04X} {arrow} ${value:02X}"
            )?;
        }
        StopReason::Halted => writeln!(w, "Halted with interrupts disabled.")?,
        StopReason::Paused => writeln!(w, "Paused.")?,
        StopReason::OutOfBudget => writeln!(w, "Run limit reached; the machine is still there.")?,
    }
    write!(w, "=> {}", stop.next.listing_line())?;
    write!(
        w,
        "\n   T={} after {} instructions",
        stop.t, stop.instructions
    )
}

// ---- Registers -----------------------------------------------------------

/// The flag byte as the eight bits it is, undocumented ones included: a letter
/// where the bit is set and a dash where it is not, in the order they sit in
/// `F`.
pub fn flags(f: u8) -> String {
    const BITS: [(u8, char); 8] = [
        (flag::S, 'S'),
        (flag::Z, 'Z'),
        (flag::Y, 'Y'),
        (flag::H, 'H'),
        (flag::X, 'X'),
        (flag::P, 'P'),
        (flag::N, 'N'),
        (flag::C, 'C'),
    ];
    BITS.iter()
        .map(|(mask, name)| if f & mask != 0 { *name } else { '-' })
        .collect()
}

fn write_registers<W: fmt::Write>(w: &mut W, view: &RegisterView) -> fmt::Result {
    let r: &Regs = &view.regs;
    writeln!(
        w,
        "AF={:04X} [{}]  BC={:04X}  DE={:04X}  HL={:04X}",
        r.af(),
        flags(r.f),
        r.bc(),
        r.de(),
        r.hl()
    )?;
    writeln!(
        w,
        "AF'={:04X} [{}] BC'={:04X}  DE'={:04X}  HL'={:04X}",
        u16::from_be_bytes([r.a_, r.f_]),
        flags(r.f_),
        u16::from_be_bytes([r.b_, r.c_]),
        u16::from_be_bytes([r.d_, r.e_]),
        u16::from_be_bytes([r.h_, r.l_])
    )?;
    writeln!(
        w,
        "IX={:04X}  IY={:04X}  SP={:04X}  PC={:04X}  WZ={:04X}",
        r.ix, r.iy, r.sp, r.pc, r.wz
    )?;
    writeln!(
        w,
        "I={:02X}  R={:02X}  IM{}  IFF1={}  IFF2={}  Q={:02X}{}",
        r.i,
        r.r,
        match r.im {
            z80::InterruptMode::Im0 => 0,
            z80::InterruptMode::Im1 => 1,
            z80::InterruptMode::Im2 => 2,
        },
        u8::from(r.iff1),
        u8::from(r.iff2),
        r.q,
        if r.halted { "  halted" } else { "" }
    )?;
    write!(w, "T={} after {} instructions", view.t, view.instructions)
}

// ---- Memory --------------------------------------------------------------

fn write_memory<W: fmt::Write>(w: &mut W, dump: &MemoryDump) -> fmt::Result {
    let width = dump.unit.width();
    let per_row = items_per_row(dump.format, dump.unit);
    let items: Vec<u32> = dump
        .bytes
        .chunks_exact(width)
        .map(|c| match c {
            [b] => u32::from(*b),
            [lo, hi] => u32::from(u16::from_le_bytes([*lo, *hi])),
            _ => unreachable!("chunks of the unit's width"),
        })
        .collect();

    for (row, chunk) in items.chunks(per_row).enumerate() {
        if row > 0 {
            writeln!(w)?;
        }
        let at = dump.addr.wrapping_add((row * per_row * width) as u16);
        write!(w, "${at:04X} ")?;
        for item in chunk {
            write!(w, " {}", item_text(*item, dump.format, dump.unit))?;
        }
        // Hex bytes get the gutter, because reading text out of a dump is what
        // the byte-wide hex view is usually for.
        if dump.format == Format::Hex && dump.unit == Unit::Byte {
            let pad = (per_row - chunk.len()) * 3;
            write!(w, "{:pad$}  |", "")?;
            for item in chunk {
                write!(w, "{}", printable(*item as u8))?;
            }
            write!(w, "|")?;
        }
    }
    Ok(())
}

fn items_per_row(format: Format, unit: Unit) -> usize {
    match (format, unit) {
        (Format::Hex, Unit::Byte) | (Format::Char, _) => 16,
        (Format::Hex, Unit::Word) => 8,
        (Format::Binary, Unit::Byte) => 8,
        (Format::Binary, Unit::Word) => 4,
        _ => 8,
    }
}

fn item_text(item: u32, format: Format, unit: Unit) -> String {
    match (format, unit) {
        (Format::Hex, Unit::Byte) => format!("{item:02X}"),
        (Format::Hex, Unit::Word) => format!("{item:04X}"),
        (Format::Unsigned, Unit::Byte) => format!("{item:>3}"),
        (Format::Unsigned, Unit::Word) => format!("{item:>5}"),
        (Format::Dec, Unit::Byte) => format!("{:>4}", item as u8 as i8),
        (Format::Dec, Unit::Word) => format!("{:>6}", item as u16 as i16),
        (Format::Binary, Unit::Byte) => format!("{item:08b}"),
        (Format::Binary, Unit::Word) => format!("{item:016b}"),
        (Format::Char, _) => format!(" {} ", printable(item as u8)),
        // Strings never reach here: they are their own outcome, because a
        // string is not a grid of fixed-width items.
        (Format::Str, _) => String::new(),
    }
}

fn printable(b: u8) -> char {
    if (0x20..0x7F).contains(&b) {
        b as char
    } else {
        '.'
    }
}

fn write_strings<W: fmt::Write>(w: &mut W, strings: &[StringAt]) -> fmt::Result {
    for (i, s) in strings.iter().enumerate() {
        if i > 0 {
            writeln!(w)?;
        }
        write!(w, "${:04X}  \"", s.addr)?;
        for b in &s.bytes {
            match b {
                0x20..=0x21 | 0x23..=0x5B | 0x5D..=0x7E => write!(w, "{}", *b as char)?,
                b'"' => write!(w, "\\\"")?,
                b'\\' => write!(w, "\\\\")?,
                other => write!(w, "\\x{other:02X}")?,
            }
        }
        write!(w, "\"")?;
        if !s.terminated {
            write!(w, "...")?;
        }
    }
    Ok(())
}

// ---- Disassembly ---------------------------------------------------------

fn write_disassembly<W: fmt::Write>(w: &mut W, dis: &Disassembly) -> fmt::Result {
    for (i, instruction) in dis.instructions.iter().enumerate() {
        if i > 0 {
            writeln!(w)?;
        }
        let marker = if instruction.addr == dis.pc {
            "=> "
        } else {
            "   "
        };
        write!(w, "{marker}{}", instruction.listing_line())?;
        if instruction.undocumented {
            write!(w, "   ; undocumented")?;
        }
    }
    Ok(())
}

// ---- What is armed -------------------------------------------------------

fn write_armed<W: fmt::Write>(w: &mut W, armed: &Armed) -> fmt::Result {
    match armed {
        Armed::Breakpoint(b) => write!(w, "Breakpoint {} at ${:04X}{}", b.id, b.addr, suffix(b)),
        Armed::Watchpoint(p) => write!(
            w,
            "Watchpoint {} at ${:04X} on {}",
            p.id,
            p.addr,
            accesses(p.on_read, p.on_write)
        ),
        Armed::PortWatch(p) => write!(
            w,
            "Port watchpoint {} on {} matching ${:04X}/${:04X}",
            p.id,
            accesses(p.on_in, p.on_out),
            p.value,
            p.mask
        ),
    }
}

fn suffix(b: &Breakpoint) -> String {
    let mut s = String::new();
    if let Some(c) = &b.condition {
        let _ = write!(s, " if {}", condition(c));
    }
    if b.ignore > 0 {
        let _ = write!(s, ", ignoring the next {}", b.ignore);
    }
    if !b.enabled {
        s.push_str(" (disabled)");
    }
    s
}

fn accesses(read: bool, write: bool) -> &'static str {
    match (read, write) {
        (true, true) => "read and write",
        (true, false) => "read",
        _ => "write",
    }
}

fn write_list<W: fmt::Write>(w: &mut W, list: &ArmedList) -> fmt::Result {
    if list.is_empty() {
        return write!(w, "Nothing is armed.");
    }
    writeln!(w, "Num  Type        What         Hits  On   Detail")?;
    let mut rows: Vec<(u32, String)> = Vec::new();
    for b in &list.breakpoints {
        rows.push((b.id, breakpoint_row(b)));
    }
    for p in &list.watchpoints {
        rows.push((p.id, watchpoint_row(p)));
    }
    for p in &list.ports {
        rows.push((p.id, port_row(p)));
    }
    rows.sort_by_key(|(id, _)| *id);
    for (i, (_, row)) in rows.iter().enumerate() {
        if i > 0 {
            writeln!(w)?;
        }
        write!(w, "{row}")?;
    }
    Ok(())
}

fn breakpoint_row(b: &Breakpoint) -> String {
    let detail = match &b.condition {
        Some(c) => format!("if {}", condition(c)),
        None if b.ignore > 0 => format!("ignore {}", b.ignore),
        None => String::new(),
    };
    format!(
        "{:<5}{:<12}${:04X}        {:<6}{:<5}{}",
        b.id,
        "breakpoint",
        b.addr,
        b.hits,
        yes_no(b.enabled),
        detail
    )
}

fn watchpoint_row(p: &Watchpoint) -> String {
    format!(
        "{:<5}{:<12}${:04X}        {:<6}{:<5}{}{}",
        p.id,
        "watchpoint",
        p.addr,
        p.hits,
        yes_no(p.enabled),
        accesses(p.on_read, p.on_write),
        if p.on_change_only { ", on change" } else { "" }
    )
}

fn port_row(p: &PortWatch) -> String {
    format!(
        "{:<5}{:<12}{:<13}{:<6}{:<5}{}",
        p.id,
        "port",
        format!("${:04X}/${:04X}", p.value, p.mask),
        p.hits,
        yes_no(p.enabled),
        accesses(p.on_in, p.on_out)
    )
}

fn yes_no(on: bool) -> &'static str {
    if on { "y" } else { "n" }
}

fn id_list(ids: &[u32]) -> String {
    ids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---- Trace ---------------------------------------------------------------

fn write_trace<W: fmt::Write>(w: &mut W, trace: &Trace) -> fmt::Result {
    for instruction in &trace.steps {
        writeln!(w, "   {}", instruction.listing_line())?;
    }
    if trace.interrupted {
        writeln!(w, "   ... stopped early")?;
    }
    write_stop(w, &trace.end)
}

fn write_help<W: fmt::Write>(w: &mut W, entries: &[(&str, &str)]) -> fmt::Result {
    let width = entries
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    for (i, (name, summary)) in entries.iter().enumerate() {
        if i > 0 {
            writeln!(w)?;
        }
        write!(w, "{name:width$}  {summary}")?;
    }
    Ok(())
}

// ---- Conditions ----------------------------------------------------------

/// A condition as the text that produced it, so `info breakpoints` shows
/// something that can be typed back in. `tests/format.rs` asserts the round
/// trip rather than trusting that.
pub fn condition(c: &Condition) -> String {
    match c {
        Condition::Compare { lhs, cmp, rhs } => {
            format!("{} {} {}", operand(*lhs), comparison(*cmp), operand(*rhs))
        }
        Condition::All(cs) => join(cs, " && "),
        Condition::Any(cs) => join(cs, " || "),
        Condition::Not(c) => format!("!{}", grouped(c)),
    }
}

fn join(cs: &[Condition], sep: &str) -> String {
    cs.iter().map(grouped).collect::<Vec<_>>().join(sep)
}

/// Anything that is not a single comparison is parenthesised, which is more
/// brackets than precedence requires and no ambiguity at all.
fn grouped(c: &Condition) -> String {
    match c {
        Condition::Compare { .. } => condition(c),
        _ => format!("({})", condition(c)),
    }
}

fn comparison(cmp: Cmp) -> &'static str {
    match cmp {
        Cmp::Eq => "==",
        Cmp::Ne => "!=",
        Cmp::Lt => "<",
        Cmp::Le => "<=",
        Cmp::Gt => ">",
        Cmp::Ge => ">=",
    }
}

fn operand(o: Operand) -> String {
    match o {
        Operand::Imm(v) if v <= 0xFF => format!("${v:02X}"),
        Operand::Imm(v) => format!("${v:04X}"),
        Operand::Reg8(r) => reg8_name(r).to_string(),
        Operand::Reg16(r) => reg16_name(r).to_string(),
        Operand::Flag(mask) => flag_name(mask).to_string(),
        Operand::Mem8(addr) => format!("[${addr:04X}]"),
        Operand::Mem16(addr) => format!("w[${addr:04X}]"),
        Operand::Mem8At(r) => format!("[{}]", reg16_name(r)),
    }
}

fn reg8_name(r: Reg8) -> &'static str {
    match r {
        Reg8::A => "a",
        Reg8::B => "b",
        Reg8::C => "c",
        Reg8::D => "d",
        Reg8::E => "e",
        Reg8::H => "h",
        Reg8::L => "l",
        Reg8::Ixh => "ixh",
        Reg8::Ixl => "ixl",
        Reg8::Iyh => "iyh",
        Reg8::Iyl => "iyl",
    }
}

fn reg16_name(r: Reg16) -> &'static str {
    match r {
        Reg16::Af => "af",
        Reg16::Bc => "bc",
        Reg16::De => "de",
        Reg16::Hl => "hl",
        Reg16::Sp => "sp",
        Reg16::Ix => "ix",
        Reg16::Iy => "iy",
    }
}

fn flag_name(mask: u8) -> &'static str {
    match mask {
        flag::S => "f.s",
        flag::Z => "f.z",
        flag::Y => "f.y",
        flag::H => "f.h",
        flag::X => "f.x",
        // P and V are the same bit, and the parser accepts either spelling.
        flag::P => "f.pv",
        flag::N => "f.n",
        flag::C => "f.c",
        _ => "f.?",
    }
}
