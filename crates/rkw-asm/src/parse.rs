//! Parser.
//!
//! Line-oriented, like the assembler it imitates: a statement is a label, an
//! operation, or both, and it ends at a newline or a `:`. Expressions are
//! parsed by precedence climbing over the table in [`BinOp::precedence`].
//!
//! Errors do not stop the parse. A bad line is reported, the rest of that line
//! is discarded, and parsing resumes at the next one — so a missing bracket on
//! line 3 does not turn lines 4 to 400 into nonsense, and one run reports every
//! independent mistake in the file rather than the first.

use crate::ast::{BinOp, Expr, ExprKind, Label, LabelKind, Op, Program, Statement, UnOp};
use crate::diag::Diagnostic;
use crate::keywords;
use crate::lex::{Sym, Token, TokenKind, lex};
use crate::source::{FileId, SourceMap, Span};

pub struct Parsed {
    pub program: Program,
    /// Lexer diagnostics first, then the parser's, so they read in source
    /// order for a file whose only problems are lexical.
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Parse a registered file.
pub fn parse(map: &SourceMap, file: FileId) -> Parsed {
    let lexed = lex(map, file);
    let mut parser = Parser {
        tokens: lexed.tokens,
        pos: 0,
        diagnostics: lexed.diagnostics,
    };
    let statements = parser.program();
    Parsed {
        program: Program { file, statements },
        diagnostics: parser.diagnostics,
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl Parser {
    fn cur(&self) -> &Token {
        // The token stream always ends with Eof, and `advance` will not step
        // past it, so this never needs a bounds check.
        &self.tokens[self.pos]
    }

    fn kind(&self) -> &TokenKind {
        &self.cur().kind
    }

    fn span(&self) -> Span {
        self.cur().span
    }

    /// The end of the last token consumed, for closing off a node's span.
    fn prev_end(&self) -> Span {
        let i = self.pos.saturating_sub(1);
        self.tokens[i].span
    }

    fn advance(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn nth_kind(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[i].kind
    }

    fn eat(&mut self, sym: Sym) -> bool {
        if self.cur().is(sym) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }

    /// A statement ends at end of line, end of file, or a `:` separating it
    /// from the next statement on the same line.
    fn at_statement_end(&self) -> bool {
        matches!(self.kind(), TokenKind::Newline | TokenKind::Eof) || self.cur().is(Sym::Colon)
    }

    /// Throw away the rest of the line. The newline itself stays, so the main
    /// loop sees the statement boundary it expects.
    fn recover(&mut self) {
        while !matches!(self.kind(), TokenKind::Newline | TokenKind::Eof) {
            self.advance();
        }
    }

    fn program(&mut self) -> Vec<Statement> {
        let mut statements = Vec::new();
        loop {
            while matches!(self.kind(), TokenKind::Newline) || self.cur().is(Sym::Colon) {
                self.advance();
            }
            if matches!(self.kind(), TokenKind::Eof) {
                return statements;
            }
            let before = self.pos;
            match self.statement() {
                Some(s) => statements.push(s),
                None => self.recover(),
            }
            // A statement that consumed nothing would loop forever. Nothing
            // known does, but the parse must terminate whatever a future
            // statement form gets wrong.
            if self.pos == before {
                self.advance();
            }
        }
    }

    fn statement(&mut self) -> Option<Statement> {
        let start = self.span();
        let label = self.label();
        let op = if self.at_statement_end() {
            None
        } else {
            Some(self.op()?)
        };
        if label.is_none() && op.is_none() {
            // Only reachable if `program` let something through that is neither
            // a label nor an operation; report it rather than looping.
            let span = self.span();
            let found = self.cur().describe();
            self.error(
                Diagnostic::error(span, "expected a label, instruction or directive")
                    .with_caret_label(format!("found {found}")),
            );
            return None;
        }
        Some(Statement {
            label,
            op,
            span: start.to(self.prev_end()),
        })
    }

    /// A label definition, if the statement starts with one.
    ///
    /// Three forms, following sjasmplus: any word followed by `:`, a word in
    /// column 1 that is not a mnemonic or directive name, and a bare number
    /// followed by `:` for a temporary label.
    fn label(&mut self) -> Option<Label> {
        let span = self.span();
        let colon_next = self.nth_kind(1) == &TokenKind::Sym(Sym::Colon);
        match self.kind() {
            TokenKind::Ident(name) => {
                let name = name.clone();
                let column_one = self.cur().at_line_start
                    && (!keywords::is_op_name(&name) || self.defines_a_symbol());
                if !colon_next && !column_one {
                    return None;
                }
                let kind = if name.starts_with('.') {
                    LabelKind::Local
                } else {
                    LabelKind::Global
                };
                self.advance();
                let span = if colon_next {
                    self.advance();
                    span.to(self.prev_end())
                } else {
                    span
                };
                Some(Label { kind, name, span })
            }
            // `1:`, but not `1000h:` — a temporary label is written in plain
            // decimal, and anything else with a colon after it is a mistake
            // that later stages should see intact.
            TokenKind::Number { value, text }
                if colon_next && text.bytes().all(|b| b.is_ascii_digit()) =>
            {
                let kind = LabelKind::Temp(*value as u32);
                let name = text.clone();
                self.advance();
                self.advance();
                Some(Label {
                    kind,
                    name,
                    span: span.to(self.prev_end()),
                })
            }
            _ => None,
        }
    }

    /// True if what follows the current word can only be a symbol definition,
    /// which makes the word a label whatever it is called.
    ///
    /// `size equ 40` is the case that needs this: `SIZE` is also a directive
    /// name, so the column-1 rule alone would read the line as the `SIZE`
    /// directive applied to `equ`. Nothing can sensibly precede `EQU` except
    /// the name being defined.
    fn defines_a_symbol(&self) -> bool {
        match self.nth_kind(1) {
            TokenKind::Ident(next) => {
                matches!(
                    next.to_ascii_lowercase().as_str(),
                    "equ" | "defl" | "macro" | "field"
                )
            }
            TokenKind::Sym(Sym::Eq) => true,
            _ => false,
        }
    }

    fn op(&mut self) -> Option<Op> {
        let name_span = self.span();
        // `label = expr`, the one operation not written as a word.
        if self.cur().is(Sym::Eq) {
            self.advance();
            let args = self.operand_list()?;
            return Some(Op {
                name: "=".into(),
                name_span,
                args,
                span: name_span.to(self.prev_end()),
            });
        }
        let TokenKind::Ident(name) = self.kind() else {
            let found = self.cur().describe();
            self.error(
                Diagnostic::error(name_span, "expected an instruction or directive")
                    .with_caret_label(format!("found {found}")),
            );
            return None;
        };
        let name = name.clone();
        self.advance();
        let args = self.operand_list()?;
        Some(Op {
            name,
            name_span,
            args,
            span: name_span.to(self.prev_end()),
        })
    }

    /// Comma-separated operands, up to the end of the statement.
    fn operand_list(&mut self) -> Option<Vec<Expr>> {
        let mut args = Vec::new();
        if !self.at_statement_end() {
            loop {
                args.push(self.expr()?);
                if !self.eat(Sym::Comma) {
                    break;
                }
            }
        }
        if !self.at_statement_end() {
            let span = self.span();
            let found = self.cur().describe();
            self.error(
                Diagnostic::error(span, "expected `,` or end of statement")
                    .with_caret_label(format!("found {found}")),
            );
            return None;
        }
        Some(args)
    }

    fn expr(&mut self) -> Option<Expr> {
        self.binary(0)
    }

    /// Precedence climbing. Every operator is left-associative, so the
    /// right-hand side is parsed at one level tighter than the operator itself.
    fn binary(&mut self, min_precedence: u8) -> Option<Expr> {
        let mut lhs = self.unary()?;
        while let Some(op) = self.peek_binop() {
            if op.precedence() < min_precedence {
                break;
            }
            self.advance();
            let rhs = self.binary(op.precedence() + 1)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr::new(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Some(lhs)
    }

    fn unary(&mut self) -> Option<Expr> {
        let Some(op) = self.peek_unop() else {
            return self.atom();
        };
        let span = self.span();
        self.advance();
        let operand = self.unary()?;
        let span = span.to(operand.span);
        Some(Expr::new(
            ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            span,
        ))
    }

    fn atom(&mut self) -> Option<Expr> {
        let span = self.span();
        let kind = match self.kind() {
            TokenKind::Number { value, text } => ExprKind::Number {
                value: *value,
                text: text.clone(),
            },
            TokenKind::Str(s) => ExprKind::Str(s.clone()),
            TokenKind::Ident(name) => ExprKind::Ident(name.clone()),
            TokenKind::TempRef { id, forward } => ExprKind::TempRef {
                id: *id,
                forward: *forward,
            },
            TokenKind::Here => ExprKind::Here,
            TokenKind::SectionStart => ExprKind::SectionStart,
            TokenKind::Sym(Sym::LParen) => return self.paren(),
            _ => {
                let found = self.cur().describe();
                self.error(
                    Diagnostic::error(span, "expected an operand")
                        .with_caret_label(format!("found {found}")),
                );
                return None;
            }
        };
        self.advance();
        Some(Expr::new(kind, span))
    }

    fn paren(&mut self) -> Option<Expr> {
        let open = self.span();
        self.advance();
        let inner = self.expr()?;
        if !self.cur().is(Sym::RParen) {
            let span = self.span();
            let found = self.cur().describe();
            self.error(
                Diagnostic::error(span, "expected `)`")
                    .with_caret_label(format!("found {found}"))
                    .with_related(open, "unclosed `(`"),
            );
            return None;
        }
        self.advance();
        Some(Expr::new(
            ExprKind::Paren(Box::new(inner)),
            open.to(self.prev_end()),
        ))
    }

    /// The infix operator at the cursor, if there is one. Word-spelled
    /// operators arrive as identifiers, which is why this is a lookup rather
    /// than a match on symbols alone.
    fn peek_binop(&self) -> Option<BinOp> {
        match self.kind() {
            TokenKind::Sym(sym) => Some(match sym {
                Sym::Star => BinOp::Mul,
                Sym::Slash => BinOp::Div,
                Sym::Percent => BinOp::Mod,
                Sym::Plus => BinOp::Add,
                Sym::Minus => BinOp::Sub,
                Sym::Shl => BinOp::Shl,
                Sym::Shr => BinOp::Shr,
                Sym::Ushr => BinOp::Ushr,
                Sym::Min => BinOp::Min,
                Sym::Max => BinOp::Max,
                Sym::Lt => BinOp::Lt,
                Sym::Le => BinOp::Le,
                Sym::Gt => BinOp::Gt,
                Sym::Ge => BinOp::Ge,
                Sym::Eq => BinOp::Eq,
                Sym::EqEq => BinOp::EqEq,
                Sym::Ne => BinOp::Ne,
                Sym::Amp => BinOp::BitAnd,
                Sym::AmpAmp => BinOp::AndAnd,
                Sym::Pipe => BinOp::BitOr,
                Sym::PipePipe => BinOp::OrOr,
                Sym::Caret => BinOp::BitXor,
                _ => return None,
            }),
            TokenKind::Ident(name) => word_binop(name),
            _ => None,
        }
    }

    fn peek_unop(&self) -> Option<UnOp> {
        match self.kind() {
            TokenKind::Sym(Sym::Plus) => Some(UnOp::Pos),
            TokenKind::Sym(Sym::Minus) => Some(UnOp::Neg),
            TokenKind::Sym(Sym::Bang) => Some(UnOp::Not),
            TokenKind::Sym(Sym::Tilde) => Some(UnOp::BitNot),
            TokenKind::Ident(name) => word_unop(name),
            _ => None,
        }
    }
}

fn word_binop(name: &str) -> Option<BinOp> {
    Some(match name.to_ascii_lowercase().as_str() {
        "mod" => BinOp::ModWord,
        "shl" => BinOp::ShlWord,
        "shr" => BinOp::ShrWord,
        "and" => BinOp::AndWord,
        "or" => BinOp::OrWord,
        "xor" => BinOp::XorWord,
        _ => return None,
    })
}

fn word_unop(name: &str) -> Option<UnOp> {
    Some(match name.to_ascii_lowercase().as_str() {
        "not" => UnOp::NotWord,
        "low" => UnOp::Low,
        "high" => UnOp::High,
        "abs" => UnOp::Abs,
        "sizeof" => UnOp::SizeOf,
        "exist" => UnOp::Exist,
        _ => return None,
    })
}
