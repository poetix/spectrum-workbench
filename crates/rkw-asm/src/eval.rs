//! Constant expression evaluation.
//!
//! Arithmetic is 32-bit and signed, as sjasmplus's is, and every operation
//! wraps back into 32 bits rather than growing — an assembler that computed
//! `$FFFF * $FFFF` in 64 bits would disagree with the one the source was
//! written for. Values are carried in `i64` so that the wrap is explicit at
//! each step instead of implicit in the type.
//!
//! Evaluation needs three things the expression does not carry: the current
//! address for `$`, the start of the current section for `$$`, and where in the
//! file the expression sits, so that `1_F` and `1_B` can find the right
//! temporary label. Those are the [`Site`], and an `EQU` remembers the site it
//! was *defined* at — `screen equ $` means the address of the `EQU`, not of
//! wherever the symbol is later used.

use crate::ast::{BinOp, Expr, ExprKind, UnOp};
use crate::diag::Diagnostic;
use crate::source::Span;
use crate::symbols::Symbols;

/// Where an expression is being evaluated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Site {
    /// `$`: the address of the statement being assembled.
    pub here: i64,
    /// `$$`: the address the current section started at.
    pub section: i64,
    /// The statement's ordinal in the file, which orders `1_F` and `1_B`
    /// against the temporary labels around them.
    pub seq: u64,
}

impl Site {
    pub fn new(here: i64, section: i64, seq: u64) -> Self {
        Self { here, section, seq }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// No definition anywhere in the program.
    Undefined {
        name: String,
        span: Span,
    },
    /// Defined later than this point and not yet reached on this pass. Not an
    /// error in itself — the caller retries on the next pass — but it has to be
    /// distinguishable from [`EvalError::Undefined`], because on the final pass
    /// it becomes one.
    NotYetDefined {
        name: String,
        span: Span,
    },
    /// `a equ b` where `b equ a`. Reported rather than recursed into.
    Circular {
        name: String,
        span: Span,
    },
    DivideByZero {
        span: Span,
    },
    /// A string of more than one character where a number is required.
    NotAValue {
        span: Span,
    },
    Unsupported {
        what: &'static str,
        span: Span,
    },
    /// A value that will not fit the field it is destined for.
    OutOfRange {
        value: i64,
        min: i64,
        max: i64,
        what: &'static str,
        span: Span,
    },
    /// A relative jump beyond the reach of its one-byte displacement.
    TooFar {
        distance: i64,
        span: Span,
    },
}

impl EvalError {
    pub fn span(&self) -> Span {
        match self {
            EvalError::Undefined { span, .. }
            | EvalError::NotYetDefined { span, .. }
            | EvalError::Circular { span, .. }
            | EvalError::DivideByZero { span }
            | EvalError::NotAValue { span }
            | EvalError::Unsupported { span, .. }
            | EvalError::OutOfRange { span, .. }
            | EvalError::TooFar { span, .. } => *span,
        }
    }

    /// True while this only means "come back on the next pass".
    pub fn is_forward_reference(&self) -> bool {
        matches!(self, EvalError::NotYetDefined { .. })
    }

    pub fn diagnostic(&self) -> Diagnostic {
        let span = self.span();
        match self {
            EvalError::Undefined { name, .. } => {
                Diagnostic::error(span, format!("undefined symbol `{name}`"))
            }
            EvalError::NotYetDefined { name, .. } => {
                Diagnostic::error(span, format!("`{name}` is not defined yet on this pass"))
                    .with_note("this is normally resolved by a later pass")
            }
            EvalError::Circular { name, .. } => {
                Diagnostic::error(span, format!("`{name}` is defined in terms of itself"))
            }
            EvalError::DivideByZero { .. } => Diagnostic::error(span, "division by zero"),
            EvalError::NotAValue { .. } => Diagnostic::error(
                span,
                "a string of more than one character is not a value here",
            ),
            EvalError::Unsupported { what, .. } => {
                Diagnostic::error(span, format!("`{what}` is not supported yet"))
            }
            EvalError::OutOfRange {
                value,
                min,
                max,
                what,
                ..
            } => Diagnostic::error(span, format!("{value} does not fit in {what}"))
                .with_caret_label(format!("expected {min} to {max}")),
            EvalError::TooFar { distance, .. } => Diagnostic::error(
                span,
                format!("relative jump of {distance} bytes is out of range"),
            )
            .with_caret_label("the displacement byte reaches -128 to 127"),
        }
    }
}

/// Truncate to 32 bits, the width sjasmplus computes in.
fn wrap(v: i64) -> i64 {
    i64::from(v as i32)
}

/// Evaluate an expression to a number.
///
/// Takes the symbol table by mutable reference because an `EQU` is evaluated
/// the first time it is asked for and then remembered: constants form a graph,
/// and walking it repeatedly would be quadratic on the deep chains that macro
/// -heavy sources produce.
pub fn eval(expr: &Expr, site: Site, symbols: &mut Symbols) -> Result<i64, EvalError> {
    let span = expr.span;
    match &expr.kind {
        ExprKind::Number { value, .. } => Ok(wrap(*value)),
        ExprKind::Str(s) => match s.value.len() {
            1 => Ok(i64::from(s.value[0])),
            _ => Err(EvalError::NotAValue { span }),
        },
        ExprKind::Ident(name) => symbols.lookup(name, span),
        ExprKind::TempRef { id, forward } => symbols.lookup_temp(*id, *forward, site.seq, span),
        ExprKind::Here => Ok(wrap(site.here)),
        ExprKind::SectionStart => Ok(wrap(site.section)),
        ExprKind::Paren(inner) => eval(inner, site, symbols),
        ExprKind::Unary { op, operand } => unary(*op, operand, span, site, symbols),
        ExprKind::Binary { op, lhs, rhs } => binary(*op, lhs, rhs, span, site, symbols),
    }
}

fn unary(
    op: UnOp,
    operand: &Expr,
    span: Span,
    site: Site,
    symbols: &mut Symbols,
) -> Result<i64, EvalError> {
    // `exist` asks whether a name is defined, so it must not evaluate it.
    if op == UnOp::Exist {
        let ExprKind::Ident(name) = &operand.kind else {
            return Err(EvalError::NotAValue { span: operand.span });
        };
        return Ok(i64::from(symbols.is_defined(name)));
    }
    if op == UnOp::SizeOf {
        // Needs STRUCT, which arrives with the directives in 0004.
        return Err(EvalError::Unsupported {
            what: "sizeof",
            span,
        });
    }

    let v = eval(operand, site, symbols)?;
    Ok(wrap(match op {
        UnOp::Pos => v,
        UnOp::Neg => v.wrapping_neg(),
        // `not` is grouped with `!` rather than with `~` in the sjasmplus
        // precedence table, so it is read as the logical one.
        UnOp::Not | UnOp::NotWord => i64::from(v == 0),
        UnOp::BitNot => !v,
        UnOp::Low => v & 0xFF,
        UnOp::High => (v >> 8) & 0xFF,
        UnOp::Abs => v.wrapping_abs(),
        UnOp::SizeOf | UnOp::Exist => unreachable!("handled above"),
    }))
}

fn binary(
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    span: Span,
    site: Site,
    symbols: &mut Symbols,
) -> Result<i64, EvalError> {
    let a = eval(lhs, site, symbols)?;

    // Short-circuit before the right-hand side is touched, so the usual guard
    // idiom — `count && total/count` — does not divide by zero.
    match op {
        BinOp::AndAnd if a == 0 => return Ok(0),
        BinOp::OrOr if a != 0 => return Ok(1),
        _ => {}
    }
    let b = eval(rhs, site, symbols)?;

    let v = match op {
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::Div | BinOp::Mod | BinOp::ModWord if b == 0 => {
            return Err(EvalError::DivideByZero { span });
        }
        BinOp::Div => a.wrapping_div(b),
        BinOp::Mod | BinOp::ModWord => a.wrapping_rem(b),
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        // Shifting is done in 32 bits, so the count wraps at 32 rather than at
        // the 64 an `i64` shift would use.
        BinOp::Shl | BinOp::ShlWord => (a as i32).wrapping_shl(b as u32).into(),
        BinOp::Shr | BinOp::ShrWord => (a as i32).wrapping_shr(b as u32).into(),
        BinOp::Ushr => i64::from((a as u32).wrapping_shr(b as u32) as i32),
        BinOp::Min => a.min(b),
        BinOp::Max => a.max(b),
        BinOp::Lt => i64::from(a < b),
        BinOp::Le => i64::from(a <= b),
        BinOp::Gt => i64::from(a > b),
        BinOp::Ge => i64::from(a >= b),
        BinOp::Eq | BinOp::EqEq => i64::from(a == b),
        BinOp::Ne => i64::from(a != b),
        BinOp::BitAnd | BinOp::AndWord => a & b,
        BinOp::BitXor | BinOp::XorWord => a ^ b,
        BinOp::BitOr | BinOp::OrWord => a | b,
        BinOp::AndAnd => i64::from(b != 0),
        BinOp::OrOr => i64::from(b != 0),
    };
    Ok(wrap(v))
}

/// One byte, accepting both the signed and unsigned readings: `LD A,-1` and
/// `LD A,255` are the same instruction, and refusing either would reject
/// idiomatic source.
pub fn fit_byte(value: i64, span: Span) -> Result<u8, EvalError> {
    match value {
        -128..=255 => Ok(value as u8),
        _ => Err(EvalError::OutOfRange {
            value,
            min: -128,
            max: 255,
            what: "one byte",
            span,
        }),
    }
}

/// A signed byte: an index displacement, where 200 is not 200 but -56 and
/// almost certainly a mistake.
pub fn fit_signed_byte(value: i64, span: Span) -> Result<i8, EvalError> {
    match value {
        -128..=127 => Ok(value as i8),
        _ => Err(EvalError::OutOfRange {
            value,
            min: -128,
            max: 127,
            what: "a signed byte",
            span,
        }),
    }
}

pub fn fit_word(value: i64, span: Span) -> Result<u16, EvalError> {
    match value {
        -32768..=65535 => Ok(value as u16),
        _ => Err(EvalError::OutOfRange {
            value,
            min: -32768,
            max: 65535,
            what: "two bytes",
            span,
        }),
    }
}

/// The displacement for `JR` or `DJNZ`, measured from the address of the
/// instruction *after* the jump — which is what the CPU adds it to.
pub fn fit_relative(from_next: i64, target: i64, span: Span) -> Result<i8, EvalError> {
    let distance = target - from_next;
    match distance {
        -128..=127 => Ok(distance as i8),
        _ => Err(EvalError::TooFar { distance, span }),
    }
}
