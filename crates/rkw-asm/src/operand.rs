//! Operand classification.
//!
//! The parser deliberately left `(hl)` as a parenthesised identifier, because
//! whether parentheses mean "the memory at" depends on the mnemonic in front of
//! them. This is where that is decided — and it is decided by *asking*: the
//! encoder for each mnemonic asks each operand whether it is an 8-bit register,
//! a condition, an address, and takes the first answer that fits a form of that
//! instruction.
//!
//! Asking rather than classifying up front is what handles the genuinely
//! overloaded spellings. `C` is the carry condition in `JP C,nn`, a register in
//! `LD A,C`, and a port in `IN A,(C)`; `HL` is a register pair in `LD HL,nn`
//! and an address in `LD A,(HL)`. Nothing about the operand itself decides
//! which, so nothing here tries to.
//!
//! Register names win over symbols with the same spelling. A label called `b`
//! cannot be reached from an operand position, which is the behaviour of every
//! Z80 assembler and the reason nobody writes one.

use crate::ast::{BinOp, Expr, ExprKind};

/// Which register pair a `DD`/`FD` prefix substitutes for `HL`.
///
/// [`Index::Hl`] means no prefix, so this doubles as "which prefix byte, if
/// any" and keeps the unprefixed and prefixed forms on one code path — exactly
/// as the disassembler does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Hl,
    Ix,
    Iy,
}

impl Index {
    pub fn prefix(self) -> Option<u8> {
        match self {
            Index::Hl => None,
            Index::Ix => Some(0xDD),
            Index::Iy => Some(0xFD),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Index::Hl => "HL",
            Index::Ix => "IX",
            Index::Iy => "IY",
        }
    }

    /// Combine the index implied by two operands, or `None` if they disagree —
    /// `LD IXH,IYL` is not an instruction.
    pub fn combine(self, other: Index) -> Option<Index> {
        match (self, other) {
            (Index::Hl, x) | (x, Index::Hl) => Some(x),
            (a, b) if a == b => Some(a),
            _ => None,
        }
    }
}

/// A 16-bit register, before it is known which opcode field it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg16 {
    Bc,
    De,
    Hl,
    Sp,
    Af,
    Ix,
    Iy,
}

impl Reg16 {
    /// The `rp[]` field: `BC DE HL SP`, with `HL` standing in for `IX`/`IY`
    /// under a prefix.
    pub fn rp(self) -> Option<(u8, Index)> {
        Some(match self {
            Reg16::Bc => (0, Index::Hl),
            Reg16::De => (1, Index::Hl),
            Reg16::Hl => (2, Index::Hl),
            Reg16::Sp => (3, Index::Hl),
            Reg16::Ix => (2, Index::Ix),
            Reg16::Iy => (2, Index::Iy),
            Reg16::Af => return None,
        })
    }

    /// The `rp2[]` field, which has `AF` where `rp[]` has `SP`.
    pub fn rp2(self) -> Option<(u8, Index)> {
        Some(match self {
            Reg16::Af => (3, Index::Hl),
            Reg16::Sp => return None,
            _ => return self.rp(),
        })
    }

    /// True for the pair a prefix can substitute, so `ADD IX,IX` is accepted
    /// and `ADD IX,HL` is not.
    pub fn index(self) -> Index {
        match self {
            Reg16::Ix => Index::Ix,
            Reg16::Iy => Index::Iy,
            _ => Index::Hl,
        }
    }
}

/// An 8-bit operand: one of the `r[]` slots, possibly reached through an index
/// register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum R {
    /// A register, in its `r[]` slot. `index` is set for `IXH` and friends,
    /// which are slots 4 and 5 under a prefix.
    Reg { slot: u8, index: Index },
    /// `(HL)`, slot 6 with no prefix.
    MemHl,
    /// `(IX+d)`: slot 6 under a prefix, with a displacement byte.
    Indexed { index: Index, displacement: Expr },
}

impl R {
    pub fn index(&self) -> Index {
        match self {
            R::Reg { index, .. } => *index,
            R::MemHl => Index::Hl,
            R::Indexed { index, .. } => *index,
        }
    }

    /// The `r[]` slot number this operand encodes to.
    pub fn slot(&self) -> u8 {
        match self {
            R::Reg { slot, .. } => *slot,
            R::MemHl | R::Indexed { .. } => 6,
        }
    }

    /// True for the two forms that read memory, which cannot both appear in
    /// one `LD` — `LD (HL),(HL)` is `HALT`.
    pub fn is_memory(&self) -> bool {
        matches!(self, R::MemHl | R::Indexed { .. })
    }

    /// The displacement byte this operand contributes, if any.
    pub fn displacement(&self) -> Option<&Expr> {
        match self {
            R::Indexed { displacement, .. } => Some(displacement),
            _ => None,
        }
    }
}

/// An 8-bit register name and the prefix it needs.
///
/// The half-register spellings are the undocumented ones; sjasmplus accepts
/// several spellings of each and so does this.
pub fn reg8(name: &str) -> Option<(u8, Index)> {
    let name = name.to_ascii_uppercase();
    Some(match name.as_str() {
        "B" => (0, Index::Hl),
        "C" => (1, Index::Hl),
        "D" => (2, Index::Hl),
        "E" => (3, Index::Hl),
        "H" => (4, Index::Hl),
        "L" => (5, Index::Hl),
        "A" => (7, Index::Hl),
        "IXH" | "XH" | "HX" => (4, Index::Ix),
        "IXL" | "XL" | "LX" => (5, Index::Ix),
        "IYH" | "YH" | "HY" => (4, Index::Iy),
        "IYL" | "YL" | "LY" => (5, Index::Iy),
        _ => return None,
    })
}

pub fn reg16(name: &str) -> Option<Reg16> {
    let name = name.to_ascii_uppercase();
    Some(match name.as_str() {
        "BC" => Reg16::Bc,
        "DE" => Reg16::De,
        "HL" => Reg16::Hl,
        "SP" => Reg16::Sp,
        "AF" => Reg16::Af,
        "IX" => Reg16::Ix,
        "IY" => Reg16::Iy,
        _ => return None,
    })
}

/// The `cc[]` field: the eight flag conditions, in opcode order.
pub fn condition(name: &str) -> Option<u8> {
    let name = name.to_ascii_uppercase();
    Some(match name.as_str() {
        "NZ" => 0,
        "Z" => 1,
        "NC" => 2,
        "C" => 3,
        "PO" => 4,
        "PE" => 5,
        "P" => 6,
        "M" => 7,
        _ => return None,
    })
}

/// The bare word an operand consists of, if it is one.
fn word(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(name),
        _ => None,
    }
}

/// The expression inside a top-level pair of parentheses.
///
/// This is the surface form the parser preserved: an operand written `(x)`,
/// whatever `x` turns out to mean.
pub fn parenthesised(expr: &Expr) -> Option<&Expr> {
    expr.as_parenthesised()
}

/// As an 8-bit `r[]` operand.
pub fn as_r(expr: &Expr) -> Option<R> {
    if let Some(name) = word(expr) {
        let (slot, index) = reg8(name)?;
        return Some(R::Reg { slot, index });
    }
    let inner = parenthesised(expr)?;
    if let Some(name) = word(inner) {
        if name.eq_ignore_ascii_case("hl") {
            return Some(R::MemHl);
        }
    }
    let (index, displacement) = as_indexed(inner)?;
    Some(R::Indexed {
        index,
        displacement,
    })
}

/// As a 16-bit register.
pub fn as_reg16(expr: &Expr) -> Option<Reg16> {
    reg16(word(expr)?)
}

/// As a flag condition.
pub fn as_condition(expr: &Expr) -> Option<u8> {
    condition(word(expr)?)
}

/// As `AF'`, which appears in exactly one instruction.
pub fn is_af_shadow(expr: &Expr) -> bool {
    word(expr).is_some_and(|name| name.eq_ignore_ascii_case("af'"))
}

/// As `(BC)`, `(DE)`, `(HL)`, `(SP)` or `(C)` — a register used as an address
/// or a port, with no displacement.
pub fn as_register_indirect(expr: &Expr) -> Option<&str> {
    word(parenthesised(expr)?)
}

/// As `(nn)`: parentheses around something that is not a register form.
pub fn as_address(expr: &Expr) -> Option<&Expr> {
    let inner = parenthesised(expr)?;
    if as_indexed(inner).is_some() {
        return None;
    }
    if let Some(name) = word(inner) {
        // A lone register name in parentheses is an addressing mode, not the
        // address of a symbol that happens to be called `hl`.
        if reg8(name).is_some() || reg16(name).is_some() {
            return None;
        }
    }
    Some(inner)
}

/// The index register and displacement of an `(IX+d)`-style operand, given
/// what was inside the parentheses.
///
/// The displacement is returned as an expression with the index register
/// replaced by zero, which is what makes `(ix+lo-2)` work: `+` and `-`
/// associate to the left, so the index register is the leftmost leaf of the
/// chain and everything else is the displacement.
pub fn as_indexed(inner: &Expr) -> Option<(Index, Expr)> {
    match &inner.kind {
        ExprKind::Ident(name) => {
            let index = match reg16(name)? {
                Reg16::Ix => Index::Ix,
                Reg16::Iy => Index::Iy,
                _ => return None,
            };
            Some((
                index,
                Expr::new(
                    ExprKind::Number {
                        value: 0,
                        text: "0".into(),
                    },
                    inner.span,
                ),
            ))
        }
        ExprKind::Binary {
            op: op @ (BinOp::Add | BinOp::Sub),
            lhs,
            rhs,
        } => {
            let (index, rest) = as_indexed(lhs)?;
            Some((
                index,
                Expr::new(
                    ExprKind::Binary {
                        op: *op,
                        lhs: Box::new(rest),
                        rhs: rhs.clone(),
                    },
                    inner.span,
                ),
            ))
        }
        _ => None,
    }
}
