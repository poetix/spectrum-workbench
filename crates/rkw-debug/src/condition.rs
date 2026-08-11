//! Breakpoint conditions.
//!
//! A condition is data rather than a closure, for three reasons: it can be
//! printed back to the user in `info breakpoints`, it can be built by the
//! command parser (ticket 0010) without that parser having to construct code,
//! and it is `Debug`-comparable so tests can assert on what was set.
//!
//! It is deliberately small. Everything here is a comparison between two
//! operands, optionally combined; there are no arithmetic expressions, because
//! the thing a breakpoint condition needs to say is almost always "this
//! register is that value" and the assembler already owns the question of what
//! a general expression language looks like.
//!
//! Conditions are evaluated only when the address bitmap has already said yes,
//! so their cost is paid at breakpoint-hit rate, not at instruction rate.

use z80::disasm::Peek;
use z80::{Reg8, Reg16, Regs};

/// Something a condition can read. Everything widens to `u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Imm(u16),
    Reg8(Reg8),
    Reg16(Reg16),
    /// A flag, given as its mask from [`z80::flag`]; reads as 0 or 1.
    Flag(u8),
    /// The byte at a fixed address.
    Mem8(u16),
    /// The little-endian word at a fixed address.
    Mem16(u16),
    /// The byte at the address held in a register pair — `(HL)`.
    Mem8At(Reg16),
}

impl Operand {
    pub fn value<P: Peek>(self, regs: &Regs, mem: &P) -> u16 {
        match self {
            Operand::Imm(v) => v,
            Operand::Reg8(r) => u16::from(regs.get8(r)),
            Operand::Reg16(r) => regs.get16(r),
            Operand::Flag(mask) => u16::from(regs.flag(mask)),
            Operand::Mem8(addr) => u16::from(mem.peek(addr)),
            Operand::Mem16(addr) => {
                u16::from_le_bytes([mem.peek(addr), mem.peek(addr.wrapping_add(1))])
            }
            Operand::Mem8At(r) => u16::from(mem.peek(regs.get16(r))),
        }
    }
}

/// Comparisons are unsigned. A signed comparison would need to know the width
/// of both sides, and the Z80's own comparisons are on bytes; `A > $80` means
/// the unsigned thing here, which is what someone reading a hex dump expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn holds(self, lhs: u16, rhs: u16) -> bool {
        match self {
            Cmp::Eq => lhs == rhs,
            Cmp::Ne => lhs != rhs,
            Cmp::Lt => lhs < rhs,
            Cmp::Le => lhs <= rhs,
            Cmp::Gt => lhs > rhs,
            Cmp::Ge => lhs >= rhs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Compare {
        lhs: Operand,
        cmp: Cmp,
        rhs: Operand,
    },
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

impl Condition {
    /// `Condition::cmp(Operand::Reg8(Reg8::A), Cmp::Eq, Operand::Imm(0x2A))`
    pub fn cmp(lhs: Operand, cmp: Cmp, rhs: Operand) -> Condition {
        Condition::Compare { lhs, cmp, rhs }
    }

    /// `reg == value`, the shape nearly every condition actually has.
    pub fn reg8_eq(r: Reg8, v: u8) -> Condition {
        Condition::cmp(Operand::Reg8(r), Cmp::Eq, Operand::Imm(u16::from(v)))
    }

    pub fn reg16_eq(r: Reg16, v: u16) -> Condition {
        Condition::cmp(Operand::Reg16(r), Cmp::Eq, Operand::Imm(v))
    }

    pub fn eval<P: Peek>(&self, regs: &Regs, mem: &P) -> bool {
        match self {
            Condition::Compare { lhs, cmp, rhs } => {
                cmp.holds(lhs.value(regs, mem), rhs.value(regs, mem))
            }
            Condition::All(cs) => cs.iter().all(|c| c.eval(regs, mem)),
            Condition::Any(cs) => cs.iter().any(|c| c.eval(regs, mem)),
            Condition::Not(c) => !c.eval(regs, mem),
        }
    }
}
