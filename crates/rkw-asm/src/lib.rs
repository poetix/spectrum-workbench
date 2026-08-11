//! The front end of `rkw-asm`: source text to syntax tree.
//!
//! Syntax follows sjasmplus, so existing Spectrum sources assemble unchanged
//! and its documentation applies to this assembler too.
//!
//! The tree this produces says what was written and not what it means. Nothing
//! here knows the instruction set, the directive set or the symbol table:
//! `LD A,(HL)` parses to the name `LD` and two operands, one of which is a
//! parenthesised identifier. Deciding that the parentheses mean memory is
//! instruction selection's job, and it needs the mnemonic to decide it, which
//! the parser deliberately does not consult.
//!
//! ```
//! use rkw_asm::{SourceMap, parse};
//!
//! let mut map = SourceMap::new();
//! let file = map.add("demo.asm", "start:  ld a,%1010   ; four\n        ret\n");
//! let parsed = parse(&map, file);
//!
//! assert!(parsed.diagnostics.is_empty());
//! assert_eq!(parsed.program.statements.len(), 2);
//! assert_eq!(parsed.program.statements[0].to_string(), "start: ld a,%1010");
//! ```
//!
//! Errors do not stop it. Each is reported against its own file, line and
//! column, the rest of that line is discarded, and parsing carries on — so one
//! run reports every independent mistake rather than the first:
//!
//! ```
//! use rkw_asm::{SourceMap, parse};
//!
//! let mut map = SourceMap::new();
//! let file = map.add("bad.asm", "    ld a,(hl\n    ret\n");
//! let parsed = parse(&map, file);
//!
//! assert_eq!(parsed.diagnostics.len(), 1);
//! assert_eq!(parsed.program.statements.len(), 1); // the RET still parsed
//! print!("{}", map.render(&parsed.diagnostics[0]));
//! // error: expected `)`
//! //  --> bad.asm:1:12
//! //   |
//! // 1 |     ld a,(hl
//! //   |             ^ found end of line
//! //   = note: unclosed `(` at bad.asm:1:10
//! ```

pub mod ast;
pub mod diag;
pub mod keywords;
pub mod lex;
pub mod parse;
pub mod source;

pub use ast::{BinOp, Expr, ExprKind, Label, LabelKind, Op, Program, Statement, UnOp};
pub use diag::{Diagnostic, Severity};
pub use lex::{Lexed, Quote, StrLit, StrSuffix, Sym, Token, TokenKind, lex};
pub use parse::{Parsed, parse};
pub use source::{FileId, Location, SourceFile, SourceMap, Span};
