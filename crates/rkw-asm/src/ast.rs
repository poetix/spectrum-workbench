//! The syntax tree.
//!
//! This tree records what was written, not what it means. `(HL)` is a
//! parenthesised identifier, and whether that is the memory at HL or a
//! redundantly bracketed expression depends on the mnemonic in front of it —
//! `LD A,(HL)` and `LD A,(label)` have the same shape here and different
//! encodings. Instruction selection resolves that later; if this tree took a
//! view, that later stage would have to unpick it.
//!
//! For the same reason the surface spelling of everything is preserved: `10`
//! and `$0A` are different [`ExprKind::Number`]s with the same value, `and` and
//! `&` are different [`BinOp`]s with the same operation, and redundant
//! parentheses are nodes rather than being folded away. That is what lets the
//! listing (ticket 0006) print source lines it has only parsed, and what lets
//! the disassembler round-trip test compare text with text.
//!
//! There is no distinction here between an instruction, a directive and a macro
//! call. All three are a name followed by comma-separated operands, and telling
//! them apart means knowing the instruction set, the directive set and the
//! macros defined so far — none of which is a question about syntax.

use std::fmt;

use crate::lex::StrLit;
use crate::source::Span;

/// A parsed file: a flat sequence of statements.
///
/// Flat, not nested: `IF`/`ENDIF`, `MACRO`/`ENDM` and `MODULE`/`ENDMODULE` are
/// statements like any other, and pairing them up is conditional assembly's
/// job, not the parser's. It has to be that way round — which arm of an `IF`
/// exists depends on a symbol value, which is not known yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub file: crate::source::FileId,
    pub statements: Vec<Statement>,
}

/// One statement: an optional label, an optional operation, or both.
///
/// A line may hold several, separated by `:`, and each is a statement of its
/// own with its own span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub label: Option<Label>,
    pub op: Option<Op>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub kind: LabelKind,
    /// As written, including any leading `.` or `@`.
    pub name: Box<str>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    /// `main`, `main:`, or `@main` — the `@` form is exempt from module name
    /// prefixing, which is a later stage's concern but a lexical mark.
    Global,
    /// `.loop`, belonging to whichever global label precedes it.
    Local,
    /// `1:` — a numeric temporary label, referred to as `1_F` or `1_B`.
    Temp(u32),
}

impl Label {
    /// True for `@`-prefixed labels, which sjasmplus uses verbatim rather than
    /// qualifying with the current module.
    pub fn is_verbatim(&self) -> bool {
        self.name.starts_with('@')
    }
}

/// An instruction, directive or macro call: a name and its operands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    /// As written, so case and any leading `.` survive for the listing.
    pub name: Box<str>,
    pub name_span: Span,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    /// `text` is the original spelling; `value` is what it denotes.
    Number {
        value: i64,
        text: Box<str>,
    },
    Str(StrLit),
    /// A label, register name, or anything else spelled as a word.
    Ident(Box<str>),
    /// `1_F` / `1_B`.
    TempRef {
        id: u32,
        forward: bool,
    },
    /// `$`.
    Here,
    /// `$$`, the start of the current section.
    SectionStart,
    /// Parentheses, kept because they are the surface mark of a memory operand
    /// and cannot be told from grouping until the mnemonic is known.
    Paren(Box<Expr>),
    Unary {
        op: UnOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// The operand's inner expression if it is parenthesised at the top level —
    /// the surface form that means "the memory at" for most mnemonics.
    pub fn as_parenthesised(&self) -> Option<&Expr> {
        match &self.kind {
            ExprKind::Paren(inner) => Some(inner),
            _ => None,
        }
    }

    /// The identifier this operand consists of, if it is a bare word. Used to
    /// test for register names without committing the tree to a view on which
    /// words are registers.
    pub fn as_ident(&self) -> Option<&str> {
        match &self.kind {
            ExprKind::Ident(name) => Some(name),
            _ => None,
        }
    }

    /// The value of a one-character literal, which sjasmplus treats as a
    /// number: `'a'` is 97. Longer literals are strings and give `None`.
    pub fn as_char_value(&self) -> Option<i64> {
        match &self.kind {
            ExprKind::Str(s) if s.value.len() == 1 => Some(i64::from(s.value[0])),
            _ => None,
        }
    }
}

/// Prefix operators, including the word spellings sjasmplus accepts.
///
/// Word and symbol spellings of the same operation are separate variants so
/// that printing a tree reproduces the source; the stage that evaluates them
/// treats the pairs alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Pos,
    Neg,
    /// `!`
    Not,
    /// `not`
    NotWord,
    /// `~`
    BitNot,
    Low,
    High,
    Abs,
    SizeOf,
    Exist,
}

impl UnOp {
    pub fn text(self) -> &'static str {
        match self {
            UnOp::Pos => "+",
            UnOp::Neg => "-",
            UnOp::Not => "!",
            UnOp::NotWord => "not",
            UnOp::BitNot => "~",
            UnOp::Low => "low",
            UnOp::High => "high",
            UnOp::Abs => "abs",
            UnOp::SizeOf => "sizeof",
            UnOp::Exist => "exist",
        }
    }

    /// True if written as a word, and so needing a space before its operand.
    pub fn is_word(self) -> bool {
        matches!(
            self,
            UnOp::NotWord | UnOp::Low | UnOp::High | UnOp::Abs | UnOp::SizeOf | UnOp::Exist
        )
    }
}

/// Infix operators, in the precedence sjasmplus documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Mul,
    Div,
    Mod,
    ModWord,
    Add,
    Sub,
    Shl,
    ShlWord,
    Shr,
    ShrWord,
    /// `>>>`, shifting in zeros where `>>` propagates the sign bit.
    Ushr,
    /// `<?`
    Min,
    /// `>?`
    Max,
    Lt,
    Le,
    Gt,
    Ge,
    /// `=`, which sjasmplus accepts as a comparison.
    Eq,
    EqEq,
    Ne,
    BitAnd,
    AndWord,
    BitXor,
    XorWord,
    BitOr,
    OrWord,
    AndAnd,
    OrOr,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::ModWord => "mod",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Shl => "<<",
            BinOp::ShlWord => "shl",
            BinOp::Shr => ">>",
            BinOp::ShrWord => "shr",
            BinOp::Ushr => ">>>",
            BinOp::Min => "<?",
            BinOp::Max => ">?",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "=",
            BinOp::EqEq => "==",
            BinOp::Ne => "!=",
            BinOp::BitAnd => "&",
            BinOp::AndWord => "and",
            BinOp::BitXor => "^",
            BinOp::XorWord => "xor",
            BinOp::BitOr => "|",
            BinOp::OrWord => "or",
            BinOp::AndAnd => "&&",
            BinOp::OrOr => "||",
        }
    }

    pub fn is_word(self) -> bool {
        matches!(
            self,
            BinOp::ModWord
                | BinOp::ShlWord
                | BinOp::ShrWord
                | BinOp::AndWord
                | BinOp::XorWord
                | BinOp::OrWord
        )
    }

    /// Binding power, higher binding tighter. All of these associate to the
    /// left, so the parser needs no separate associativity table.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::ModWord => 10,
            BinOp::Add | BinOp::Sub => 9,
            BinOp::Shl | BinOp::ShlWord | BinOp::Shr | BinOp::ShrWord | BinOp::Ushr => 8,
            BinOp::Min | BinOp::Max => 7,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 6,
            BinOp::Eq | BinOp::EqEq | BinOp::Ne => 5,
            BinOp::BitAnd | BinOp::AndWord => 4,
            BinOp::BitXor | BinOp::XorWord => 3,
            BinOp::BitOr | BinOp::OrWord => 2,
            BinOp::AndAnd => 1,
            BinOp::OrOr => 0,
        }
    }
}

// Printing reproduces the source, which is what makes "disassemble, parse,
// print, compare" a test rather than an approximation. Symbol operators are
// printed tight and word operators spaced, matching how each is written.

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Number { text, .. } => f.write_str(text),
            ExprKind::Str(s) => f.write_str(&s.raw),
            ExprKind::Ident(name) => f.write_str(name),
            ExprKind::TempRef { id, forward } => {
                write!(f, "{id}_{}", if *forward { 'F' } else { 'B' })
            }
            ExprKind::Here => f.write_str("$"),
            ExprKind::SectionStart => f.write_str("$$"),
            ExprKind::Paren(inner) => write!(f, "({inner})"),
            ExprKind::Unary { op, operand } => {
                f.write_str(op.text())?;
                if op.is_word() {
                    f.write_str(" ")?;
                }
                write!(f, "{operand}")
            }
            ExprKind::Binary { op, lhs, rhs } => {
                if op.is_word() {
                    write!(f, "{lhs} {} {rhs}", op.text())
                } else {
                    write!(f, "{lhs}{}{rhs}", op.text())
                }
            }
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        for (i, arg) in self.args.iter().enumerate() {
            f.write_str(if i == 0 { " " } else { "," })?;
            write!(f, "{arg}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.name)
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.label, &self.op) {
            (Some(l), Some(o)) => write!(f, "{l} {o}"),
            (Some(l), None) => write!(f, "{l}"),
            (None, Some(o)) => write!(f, "{o}"),
            (None, None) => Ok(()),
        }
    }
}
