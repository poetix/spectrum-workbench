//! Tokeniser.
//!
//! Syntax follows sjasmplus, so existing Spectrum sources assemble unchanged
//! and its documentation applies. That fixes a number of decisions that would
//! otherwise be free: five ways of writing hex, three of writing binary, digit
//! separators, and word spellings for most of the operators.
//!
//! The lexer decides as little as it can get away with. A mnemonic, a register,
//! a directive name and a label are all [`TokenKind::Ident`]; which one it is
//! depends on where it sits and, for operands, on the mnemonic — knowledge the
//! parser has and the lexer does not. What the lexer *must* settle, because it
//! is a question about characters rather than meaning, is where a token ends.
//!
//! Three places where that is genuinely ambiguous, and how it is resolved:
//!
//! * `%` is both the binary prefix in `%1010` and the modulo operator in
//!   `x % 2`. It is a prefix only where a value cannot already have been read —
//!   after an operator or a `(`, not after a number, identifier or `)`.
//! * `1b` is both a binary literal and a backward reference to temporary label
//!   one. The literal wins when every digit is `0` or `1`; write `1_B` for the
//!   label, which is the spelling sjasmplus recommends and which always wins.
//! * `'` opens a string, separates digits in `12'345`, and ends the shadow
//!   register in `EX AF,AF'`. It separates digits only between two digits, and
//!   attaches to an identifier only after `af`, `bc`, `de` or `hl`.
//!
//! Deviations from sjasmplus, both deliberate: `!` and `#` are accepted in
//! sjasmplus identifiers and are not accepted here, because `!` cannot be told
//! from the start of `!=` and `#` cannot be told from a hex prefix.

use crate::diag::Diagnostic;
use crate::source::{FileId, SourceMap, Span};

/// A punctuation or operator token. Word-spelled operators (`mod`, `shl`,
/// `and`, ...) arrive as [`TokenKind::Ident`] and are recognised by the parser,
/// because at that point they are indistinguishable from a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    Comma,
    Colon,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    AmpAmp,
    Pipe,
    PipePipe,
    Caret,
    Tilde,
    Bang,
    Shl,
    Shr,
    Ushr,
    /// `<?`, the smaller of two values.
    Min,
    /// `>?`, the larger of two values.
    Max,
    Lt,
    Le,
    Gt,
    Ge,
    /// `=`, which sjasmplus accepts as a synonym for `==` in expressions.
    Eq,
    EqEq,
    Ne,
}

impl Sym {
    pub fn text(self) -> &'static str {
        match self {
            Sym::Comma => ",",
            Sym::Colon => ":",
            Sym::LParen => "(",
            Sym::RParen => ")",
            Sym::LBracket => "[",
            Sym::RBracket => "]",
            Sym::LBrace => "{",
            Sym::RBrace => "}",
            Sym::Plus => "+",
            Sym::Minus => "-",
            Sym::Star => "*",
            Sym::Slash => "/",
            Sym::Percent => "%",
            Sym::Amp => "&",
            Sym::AmpAmp => "&&",
            Sym::Pipe => "|",
            Sym::PipePipe => "||",
            Sym::Caret => "^",
            Sym::Tilde => "~",
            Sym::Bang => "!",
            Sym::Shl => "<<",
            Sym::Shr => ">>",
            Sym::Ushr => ">>>",
            Sym::Min => "<?",
            Sym::Max => ">?",
            Sym::Lt => "<",
            Sym::Le => "<=",
            Sym::Gt => ">",
            Sym::Ge => ">=",
            Sym::Eq => "=",
            Sym::EqEq => "==",
            Sym::Ne => "!=",
        }
    }
}

/// Which quote character delimited a string. It matters: escape sequences are
/// processed between double quotes only, and between apostrophes a doubled
/// apostrophe is the way to write one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    Double,
    Single,
}

/// A letter immediately after the closing quote, which sjasmplus reads as part
/// of the literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrSuffix {
    /// `z`: append a zero byte.
    Zero,
    /// `c`: set the top bit of the last byte, the usual end-of-string marker
    /// in ROM routines.
    TopBit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrLit {
    /// Exactly as written, quotes and suffix included, so a listing can
    /// reproduce the source.
    pub raw: Box<str>,
    /// The bytes it stands for, with escapes decoded and any suffix applied.
    pub value: Vec<u8>,
    pub quote: Quote,
    pub suffix: Option<StrSuffix>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// A label, mnemonic, register, directive or macro name. Which of those it
    /// is is not a question about characters, so the lexer does not answer it.
    Ident(Box<str>),
    /// A numeric literal. `text` is the original spelling, kept so that a
    /// listing — and the disassembler round-trip test — can reproduce what was
    /// written rather than a canonicalised form of it.
    Number {
        value: i64,
        text: Box<str>,
    },
    Str(StrLit),
    /// `1_F` / `1_B`: a reference to the next or previous numeric temporary
    /// label. Which definition it means is the symbol table's business.
    TempRef {
        id: u32,
        forward: bool,
    },
    /// `$` alone: the address of the instruction being assembled.
    Here,
    /// `$$`: the address the current section started at.
    SectionStart,
    Sym(Sym),
    /// End of line. Kept as a token because the assembler is line-oriented:
    /// statements end here, and so does error recovery.
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True if this token starts in column 1. sjasmplus uses that to tell a
    /// label from a mnemonic, so the parser needs it and cannot recover it from
    /// the span without going back to the text.
    pub at_line_start: bool,
}

impl Token {
    pub fn is(&self, sym: Sym) -> bool {
        self.kind == TokenKind::Sym(sym)
    }

    /// How to name this token in an error message.
    pub fn describe(&self) -> String {
        match &self.kind {
            TokenKind::Ident(name) => format!("`{name}`"),
            TokenKind::Number { text, .. } => format!("`{text}`"),
            TokenKind::Str(s) => format!("string {}", s.raw),
            TokenKind::TempRef { id, forward } => {
                format!("`{id}_{}`", if *forward { 'F' } else { 'B' })
            }
            TokenKind::Here => "`$`".into(),
            TokenKind::SectionStart => "`$$`".into(),
            TokenKind::Sym(s) => format!("`{}`", s.text()),
            TokenKind::Newline => "end of line".into(),
            TokenKind::Eof => "end of file".into(),
        }
    }
}

/// The result of lexing one file.
pub struct Lexed {
    /// Always ends with [`TokenKind::Eof`], so the parser can look ahead one
    /// token without a bounds check.
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Tokenise a registered file.
pub fn lex(map: &SourceMap, file: FileId) -> Lexed {
    Lexer::new(map.file(file).text(), file).run()
}

/// Registers whose shadow copy is written with a trailing apostrophe.
const SHADOW_PAIRS: [&str; 4] = ["af", "bc", "de", "hl"];

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    file: FileId,
    line_begin: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    /// Whether the previous token could end a value, which is what decides
    /// between the two meanings of `%`.
    prev_ends_value: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, file: FileId) -> Self {
        Self {
            src,
            pos: 0,
            file,
            line_begin: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            prev_ends_value: false,
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start as u32, self.pos as u32)
    }

    fn push(&mut self, start: usize, kind: TokenKind) {
        self.prev_ends_value = matches!(
            kind,
            TokenKind::Ident(_)
                | TokenKind::Number { .. }
                | TokenKind::Str(_)
                | TokenKind::TempRef { .. }
                | TokenKind::Here
                | TokenKind::SectionStart
                | TokenKind::Sym(Sym::RParen)
                | TokenKind::Sym(Sym::RBracket)
                | TokenKind::Sym(Sym::RBrace)
        );
        self.tokens.push(Token {
            kind,
            span: self.span(start),
            at_line_start: start == self.line_begin,
        });
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    fn newline(&mut self, start: usize) {
        self.push(start, TokenKind::Newline);
        self.line_begin = self.pos;
        self.prev_ends_value = false;
    }

    fn run(mut self) -> Lexed {
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.peek() else {
                self.push(start, TokenKind::Eof);
                break;
            };
            match c {
                '\n' => {
                    self.bump();
                    self.newline(start);
                }
                '\r' => {
                    self.bump();
                    self.eat('\n');
                    self.newline(start);
                }
                '"' | '\'' => self.string(),
                '$' => self.dollar(),
                '#' => self.sigil_number(16, "hexadecimal"),
                '%' => self.percent(),
                c if c.is_ascii_digit() => self.number(),
                c if is_ident_start(c) => self.ident(),
                _ => self.symbol(),
            }
        }
        Lexed {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    /// Whitespace and comments. Newlines inside a block comment are still
    /// emitted, so commenting out a run of lines leaves the line structure —
    /// and therefore error recovery — intact.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t') => {
                    self.bump();
                }
                Some(';') => self.skip_to_end_of_line(),
                Some('/') if self.peek_at(1) == Some('/') => self.skip_to_end_of_line(),
                Some('/') if self.peek_at(1) == Some('*') => self.block_comment(),
                _ => return,
            }
        }
    }

    fn skip_to_end_of_line(&mut self) {
        while !matches!(self.peek(), None | Some('\n') | Some('\r')) {
            self.bump();
        }
    }

    fn block_comment(&mut self) {
        let start = self.pos;
        self.bump();
        self.bump();
        // sjasmplus nests these, so a commented-out region containing a comment
        // ends where the reader expects rather than at the first `*/`.
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek() {
                None => {
                    let span = Span::new(self.file, start as u32, self.pos as u32);
                    self.error(span, "unterminated block comment");
                    return;
                }
                Some('*') if self.peek_at(1) == Some('/') => {
                    self.bump();
                    self.bump();
                    depth -= 1;
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.bump();
                    self.bump();
                    depth += 1;
                }
                Some('\n') => {
                    let nl = self.pos;
                    self.bump();
                    self.newline(nl);
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn ident(&mut self) {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            self.bump();
        }
        // `EX AF,AF'`. Only the four pairs that have a shadow copy take the
        // apostrophe, so `db 'x'` after a bare `db` is still a string.
        if self.peek() == Some('\'')
            && SHADOW_PAIRS
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&self.src[start..self.pos]))
        {
            self.bump();
        }
        let text = self.src[start..self.pos].into();
        self.push(start, TokenKind::Ident(text));
    }

    /// `$` is hexadecimal when digits follow it, the start of the current
    /// section when doubled, and the current address otherwise.
    fn dollar(&mut self) {
        if matches!(self.peek_at(1), Some(c) if c.is_ascii_hexdigit()) {
            self.sigil_number(16, "hexadecimal");
            return;
        }
        let start = self.pos;
        self.bump();
        let kind = if self.eat('$') {
            TokenKind::SectionStart
        } else {
            TokenKind::Here
        };
        self.push(start, kind);
    }

    /// `%` prefixes a binary literal only where no value has just been read;
    /// everywhere else it is modulo.
    fn percent(&mut self) {
        let binary_here =
            !self.prev_ends_value && matches!(self.peek_at(1), Some('0' | '1' | '_' | '\''));
        if binary_here {
            self.sigil_number(2, "binary");
        } else {
            let start = self.pos;
            self.bump();
            self.push(start, TokenKind::Sym(Sym::Percent));
        }
    }

    /// A literal whose radix is fixed by a leading `$`, `#` or `%`.
    fn sigil_number(&mut self, radix: u32, what: &str) {
        let start = self.pos;
        self.bump();
        let digits_at = self.pos;
        self.scan_digit_run();
        let digits = strip_separators(&self.src[digits_at..self.pos]);
        let text: Box<str> = self.src[start..self.pos].into();
        let span = self.span(start);
        let value = self.digits_to_value(&digits, radix, what, span);
        self.push(start, TokenKind::Number { value, text });
    }

    /// A literal that starts with a digit, so its radix comes from a `0x`-style
    /// prefix or a trailing letter.
    fn number(&mut self) {
        let start = self.pos;
        self.scan_digit_run();
        let text: Box<str> = self.src[start..self.pos].into();
        let span = self.span(start);

        if let Some((id, forward)) = temp_label_ref(&text) {
            self.push(start, TokenKind::TempRef { id, forward });
            return;
        }

        let cleaned = strip_separators(&text);
        let (digits, radix, what) = split_radix(&cleaned);
        let value = self.digits_to_value(digits, radix, what, span);
        self.push(start, TokenKind::Number { value, text });
    }

    /// Letters, digits, and the `_` and `'` digit separators. Letters are taken
    /// greedily so that `0FFh` and `1_3_7q` arrive whole and the suffix can be
    /// examined; a run that turns out not to be a number is reported, not
    /// silently split.
    fn scan_digit_run(&mut self) {
        while let Some(c) = self.peek() {
            // An apostrophe separates digits only between two of them, so the
            // string in `db 5,'a'` still opens where it should.
            let separator = c == '\'' && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit());
            if c.is_ascii_alphanumeric() || c == '_' || separator {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn digits_to_value(&mut self, digits: &str, radix: u32, what: &str, span: Span) -> i64 {
        if digits.is_empty() {
            self.error(span, format!("{what} literal has no digits"));
            return 0;
        }
        if let Some(bad) = digits.chars().find(|c| !c.is_digit(radix)) {
            self.error(span, format!("`{bad}` is not a {what} digit"));
            return 0;
        }
        match i64::from_str_radix(digits, radix) {
            // sjasmplus expressions are 32-bit, so anything wider is a typo
            // rather than a value the assembler could ever use.
            Ok(v) if v <= u32::MAX as i64 => v,
            _ => {
                self.error(span, "numeric literal does not fit in 32 bits");
                0
            }
        }
    }

    fn string(&mut self) {
        let start = self.pos;
        let quote = if self.peek() == Some('"') {
            Quote::Double
        } else {
            Quote::Single
        };
        let delimiter = self.bump().expect("called with a quote available");
        let mut value = Vec::new();
        let terminated = loop {
            match self.peek() {
                None | Some('\n') | Some('\r') => break false,
                Some(c) if c == delimiter => {
                    self.bump();
                    // Between apostrophes, `''` is how one apostrophe is
                    // written, so a doubled delimiter continues the string.
                    if quote == Quote::Single && self.peek() == Some('\'') {
                        self.bump();
                        value.push(b'\'');
                        continue;
                    }
                    break true;
                }
                Some('\\') if quote == Quote::Double => {
                    let esc_start = self.pos;
                    self.bump();
                    match self.bump() {
                        Some(e) => match escape_value(e) {
                            Some(b) => value.push(b),
                            None => {
                                let span = self.span(esc_start);
                                self.error(span, format!("unknown escape sequence `\\{e}`"));
                                push_char(&mut value, e);
                            }
                        },
                        None => break false,
                    }
                }
                Some(c) => {
                    self.bump();
                    push_char(&mut value, c);
                }
            }
        };
        if !terminated {
            let span = self.span(start);
            self.error(span, "unterminated string literal");
        }

        // `"..."z` and `"..."c` are part of the literal, but only when the
        // letter stands alone — `"a"code` is a string followed by a label.
        let suffix = match self.peek() {
            Some(c @ ('z' | 'Z' | 'c' | 'C'))
                if terminated && !matches!(self.peek_at(1), Some(n) if is_ident_continue(n)) =>
            {
                self.bump();
                if c.eq_ignore_ascii_case(&'z') {
                    value.push(0);
                    Some(StrSuffix::Zero)
                } else {
                    if let Some(last) = value.last_mut() {
                        *last |= 0x80;
                    }
                    Some(StrSuffix::TopBit)
                }
            }
            _ => None,
        };

        let raw = self.src[start..self.pos].into();
        self.push(
            start,
            TokenKind::Str(StrLit {
                raw,
                value,
                quote,
                suffix,
            }),
        );
    }

    fn symbol(&mut self) {
        let start = self.pos;
        let c = self.bump().expect("called with a character available");
        let sym = match c {
            ',' => Sym::Comma,
            ':' => Sym::Colon,
            '(' => Sym::LParen,
            ')' => Sym::RParen,
            '[' => Sym::LBracket,
            ']' => Sym::RBracket,
            '{' => Sym::LBrace,
            '}' => Sym::RBrace,
            '+' => Sym::Plus,
            '-' => Sym::Minus,
            '*' => Sym::Star,
            '/' => Sym::Slash,
            '~' => Sym::Tilde,
            '^' => Sym::Caret,
            '&' => {
                if self.eat('&') {
                    Sym::AmpAmp
                } else {
                    Sym::Amp
                }
            }
            '|' => {
                if self.eat('|') {
                    Sym::PipePipe
                } else {
                    Sym::Pipe
                }
            }
            '!' => {
                if self.eat('=') {
                    Sym::Ne
                } else {
                    Sym::Bang
                }
            }
            '=' => {
                if self.eat('=') {
                    Sym::EqEq
                } else {
                    Sym::Eq
                }
            }
            '<' => {
                if self.eat('<') {
                    Sym::Shl
                } else if self.eat('=') {
                    Sym::Le
                } else if self.eat('?') {
                    Sym::Min
                } else {
                    Sym::Lt
                }
            }
            '>' => {
                if self.eat('>') {
                    if self.eat('>') { Sym::Ushr } else { Sym::Shr }
                } else if self.eat('=') {
                    Sym::Ge
                } else if self.eat('?') {
                    Sym::Max
                } else {
                    Sym::Gt
                }
            }
            _ => {
                let span = self.span(start);
                self.error(span, format!("unexpected character `{c}`"));
                return;
            }
        };
        self.push(start, TokenKind::Sym(sym));
    }
}

/// Characters are written into strings as UTF-8, which for the ASCII that
/// Spectrum sources are made of is the byte itself.
fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// The sjasmplus escape table. Deliberately not Rust's or C's: `\d` is delete
/// and `\e` is escape, and there are no `\xNN` or `\u{...}` forms.
fn escape_value(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        '\\' => 92,
        '?' => 63,
        '\'' => 39,
        '"' => 34,
        '0' => 0,
        'a' => 7,
        'b' => 8,
        'd' => 127,
        'e' => 27,
        'f' => 12,
        'n' => 10,
        'r' => 13,
        't' => 9,
        'v' => 11,
        _ => return None,
    })
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '_' | '.' | '?' | '@')
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '?' | '@')
}

fn strip_separators(raw: &str) -> String {
    raw.chars().filter(|&c| c != '_' && c != '\'').collect()
}

/// `1_F` / `2_B`, and the older `1F` / `1B` spellings.
///
/// `1_F` and `1_B` are recognised before anything else and so always mean a
/// temporary label. Of the short forms, `1F` is unambiguous — no numeric suffix
/// is `f` — but `1B` collides with binary `1`, and there the literal wins.
fn temp_label_ref(raw: &str) -> Option<(u32, bool)> {
    let (digits, last) = raw.split_at(raw.len().checked_sub(1)?);
    let forward = match last {
        "f" | "F" => true,
        "b" | "B" => false,
        _ => return None,
    };
    let (digits, explicit) = match digits.strip_suffix('_') {
        Some(d) => (d, true),
        None => (digits, false),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !forward && !explicit && is_binary(digits) {
        return None; // a binary literal, per the rule above
    }
    Some((digits.parse().ok()?, forward))
}

/// Strip whatever fixes the radix of a digit-led literal, returning the digits,
/// the radix, and what to call it in an error message.
fn split_radix(cleaned: &str) -> (&str, u32, &'static str) {
    // The suffix is examined first: in `0b1h` the trailing `h` decides, and
    // `0FFh` would otherwise be read as an unprefixed decimal.
    if let Some(body) = cleaned.strip_suffix(['h', 'H']) {
        return (body, 16, "hexadecimal");
    }
    if let Some(body) = cleaned.strip_suffix(['b', 'B']) {
        if is_binary(body) {
            return (body, 2, "binary");
        }
    }
    if let Some(body) = cleaned.strip_suffix(['o', 'O', 'q', 'Q']) {
        return (body, 8, "octal");
    }
    if let Some(body) = cleaned.strip_suffix(['d', 'D']) {
        return (body, 10, "decimal");
    }
    if let Some(rest) = cleaned.strip_prefix('0') {
        if let Some(body) = rest.strip_prefix(['x', 'X']) {
            return (body, 16, "hexadecimal");
        }
        if let Some(body) = rest.strip_prefix(['q', 'Q']) {
            return (body, 8, "octal");
        }
        if let Some(body) = rest.strip_prefix(['b', 'B']) {
            if is_binary(body) {
                return (body, 2, "binary");
            }
        }
    }
    (cleaned, 10, "decimal")
}

fn is_binary(digits: &str) -> bool {
    digits.bytes().all(|b| b == b'0' || b == b'1')
}
