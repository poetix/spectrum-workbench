//! Text to [`Request`]: the first of the command layer's three parts.
//!
//! The grammar is gdb's where gdb has one, because the people who will type it
//! already know that one. Where it differs it is because the Z80 differs: `x`
//! sizes are one and two bytes rather than four and eight, and a memory
//! reference inside a condition is written `[hl]` rather than `(hl)` so that
//! parentheses can go on meaning grouping.
//!
//! Nothing here touches a machine. Parsing `break $8002 if a == $2a` yields a
//! value, and whether `$8002` is a sensible address is the executor's business
//! — which is what makes the whole grammar testable as a pure function.

use crate::breakpoints::Id;
use crate::condition::{Cmp, Condition, Operand};
use z80::{Reg8, Reg16, flag};

/// What a command names an address by.
///
/// A register base is what makes `x/16 hl` and `disas pc-8` work, and it is
/// resolved at execution time against the registers as they are then — so a
/// script that examines `hl` means the `hl` of the moment it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Abs(u16),
    Reg(Reg16),
    Pc,
}

/// A base plus a signed offset: `$8000`, `hl`, `pc-8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Addr {
    pub base: Base,
    pub offset: i32,
}

impl Addr {
    pub fn abs(addr: u16) -> Addr {
        Addr {
            base: Base::Abs(addr),
            offset: 0,
        }
    }
}

/// How `x` renders what it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Hex,
    /// Signed decimal, of whatever the unit's width is.
    Dec,
    Unsigned,
    Binary,
    Char,
    /// NUL-terminated strings rather than a grid of numbers.
    Str,
}

/// How wide one item of a memory dump is. A Z80 word is two bytes, so `w`
/// means two here and not gdb's four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Byte,
    Word,
}

impl Unit {
    pub fn width(self) -> usize {
        match self {
            Unit::Byte => 1,
            Unit::Word => 2,
        }
    }
}

/// What `info` was asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Info {
    Breakpoints,
    Registers,
}

/// One parsed command.
///
/// The executor turns these into structured results and the formatter turns
/// those into text; nothing downstream re-reads the line the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Break {
        addr: Addr,
        condition: Option<Condition>,
    },
    /// Empty means everything, as gdb's `delete` does.
    Delete(Vec<Id>),
    Enable(Vec<Id>),
    Disable(Vec<Id>),
    Watch {
        addr: Addr,
        read: bool,
        write: bool,
    },
    /// Watch every port matching `port & mask == value`.
    PortWatch {
        value: u16,
        mask: u16,
        on_in: bool,
        on_out: bool,
    },
    Step(u32),
    Next(u32),
    Finish,
    Continue,
    /// Run to an address: gdb's `advance`, the core's run-to-cursor.
    Until(Addr),
    /// Reset and start again from the entry point, optionally moving it.
    Run(Option<Addr>),
    Reset,
    Regs,
    Examine {
        addr: Addr,
        count: u32,
        format: Format,
        unit: Unit,
    },
    Disas {
        /// `None` means "around `PC`", which is the form worth typing often.
        addr: Option<Addr>,
        count: u32,
    },
    /// Step `n` instructions, showing each. Forwards, and unrelated to the
    /// recorded history of ticket 0022.
    Trace(u32),
    Info(Info),
    Poke {
        addr: Addr,
        value: u8,
    },
    /// Read commands from a file. The executor does no I/O; it hands this back
    /// to whatever is driving it.
    Source(String),
    Help,
    Quit,
}

/// Why a line did not parse, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// Zero-based byte offset into the line, for a caret.
    pub column: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// Every command, with the one-line summary `help` prints. The parser owns
/// this because the parser is what decides the names.
pub const HELP: &[(&str, &str)] = &[
    ("break ADDR [if COND]", "set an execution breakpoint"),
    ("delete [ID...]", "remove breakpoints, or all of them"),
    ("enable/disable [ID...]", "arm or disarm without removing"),
    ("watch ADDR", "stop when a write changes ADDR"),
    ("rwatch/awatch ADDR", "stop on reads, or on both"),
    ("pwatch VALUE [MASK] [in|out]", "watch matching I/O ports"),
    ("step [N]", "one instruction, into calls"),
    ("next [N]", "one instruction, over calls and block ops"),
    ("finish", "run until the current routine returns"),
    ("continue", "run until something stops the machine"),
    ("until ADDR", "run to an address"),
    ("run [ADDR]", "reset and start again from the entry point"),
    ("reset", "reset the CPU"),
    ("regs", "registers, flags and interrupt state"),
    ("x/NFU ADDR", "dump memory: count, format xdutcs, unit bw"),
    ("disas [ADDR] [N]", "disassemble, marking PC"),
    ("trace [N]", "step N instructions, showing each"),
    (
        "info breakpoints|registers",
        "what is armed, or where it is",
    ),
    ("poke ADDR VALUE", "write a byte behind the machine's back"),
    ("source FILE", "run a file of commands"),
    ("help", "this"),
    ("quit", "leave"),
];

/// Parse one line. `Ok(None)` for a line that was only whitespace or comment.
pub fn parse(line: &str) -> Result<Option<Request>, ParseError> {
    if let Some(request) = source_command(line) {
        return request.map(Some);
    }
    let tokens = lex(line)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut p = Parser { tokens, i: 0, line };
    let request = p.request()?;
    p.expect_end()?;
    Ok(Some(request))
}

/// `source` is recognised before lexing, because everything after it is a file
/// name and a file name is not made of the same characters as a command. `..`
/// and `~` would not survive the lexer, and a path is not something to
/// reassemble from tokens once it has been taken apart.
fn source_command(line: &str) -> Option<Result<Request, ParseError>> {
    let leading = line.len() - line.trim_start().len();
    let rest = &line[leading..];
    let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let word = &rest[..word_end];
    if !word.eq_ignore_ascii_case("source") && !word.eq_ignore_ascii_case("script") {
        return None;
    }
    let path = rest[word_end..].trim();
    if path.is_empty() {
        return Some(Err(ParseError {
            message: "expected a file name".into(),
            column: line.len(),
        }));
    }
    Some(Ok(Request::Source(path.to_string())))
}

// ---- Lexing --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Num(u32),
    Punct(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    tok: Tok,
    col: usize,
}

const PUNCT: &[&str] = &[
    "==", "!=", "<=", ">=", "&&", "||", "<", ">", "!", "(", ")", "[", "]", "/", "+", "-", ",", "=",
];

fn lex(line: &str) -> Result<Vec<Token>, ParseError> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Both comment characters, because a script file is as likely to be
        // written by someone thinking in assembler as in shell.
        if c == ';' || c == '#' {
            break;
        }
        let col = i;
        if c == '$' || c == '%' || c.is_ascii_digit() {
            let (value, next) = number(line, i)?;
            tokens.push(Token {
                tok: Tok::Num(value),
                col,
            });
            i = next;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                    i += 1;
                } else {
                    break;
                }
            }
            tokens.push(Token {
                tok: Tok::Word(line[start..i].to_ascii_lowercase()),
                col,
            });
            continue;
        }
        match PUNCT.iter().find(|p| line[i..].starts_with(**p)) {
            Some(p) => {
                tokens.push(Token {
                    tok: Tok::Punct(p),
                    col,
                });
                i += p.len();
            }
            None => {
                return Err(ParseError {
                    message: format!("unexpected character `{c}`"),
                    column: col,
                });
            }
        }
    }
    Ok(tokens)
}

/// `$8000`, `0x8000`, `%1010`, `0b1010`, `4096`. Returns the value and where
/// the number ended.
fn number(line: &str, start: usize) -> Result<(u32, usize), ParseError> {
    let rest = &line[start..];
    let (radix, digits_at) = if rest.starts_with('$') {
        (16, start + 1)
    } else if rest.starts_with("0x") || rest.starts_with("0X") {
        (16, start + 2)
    } else if rest.starts_with('%') {
        (2, start + 1)
    } else if rest.starts_with("0b") || rest.starts_with("0B") {
        (2, start + 2)
    } else {
        (10, start)
    };
    let bytes = line.as_bytes();
    let mut i = digits_at;
    while i < bytes.len() && (bytes[i] as char).is_digit(radix) {
        i += 1;
    }
    if i == digits_at {
        return Err(ParseError {
            message: "expected a number".into(),
            column: start,
        });
    }
    let value = u32::from_str_radix(&line[digits_at..i], radix).map_err(|_| ParseError {
        message: format!("`{}` does not fit in 32 bits", &line[start..i]),
        column: start,
    })?;
    Ok((value, i))
}

// ---- Parsing -------------------------------------------------------------

struct Parser<'a> {
    tokens: Vec<Token>,
    i: usize,
    line: &'a str,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.i).map(|t| &t.tok)
    }

    fn column(&self) -> usize {
        self.tokens.get(self.i).map_or(self.line.len(), |t| t.col)
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            message: message.into(),
            column: self.column(),
        })
    }

    fn at_end(&self) -> bool {
        self.i >= self.tokens.len()
    }

    /// True if the next token butts up against the one before it, with no
    /// space between. `x/16xb` is one thing written without spaces, and that
    /// is the only thing that tells it from `x/16 xb`.
    fn adjacent(&self) -> bool {
        match self.tokens.get(self.i) {
            None => false,
            Some(token) => {
                token.col > 0 && !self.line.as_bytes()[token.col - 1].is_ascii_whitespace()
            }
        }
    }

    fn advance(&mut self) {
        self.i += 1;
    }

    /// Consume the next token if it is this word.
    fn eat_word(&mut self, word: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Word(w)) if w == word) {
            self.advance();
            return true;
        }
        false
    }

    fn eat_punct(&mut self, punct: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(p)) if *p == punct) {
            self.advance();
            return true;
        }
        false
    }

    fn word(&mut self) -> Option<String> {
        match self.peek() {
            Some(Tok::Word(w)) => {
                let w = w.clone();
                self.advance();
                Some(w)
            }
            _ => None,
        }
    }

    fn number(&mut self) -> Option<u32> {
        match self.peek() {
            Some(Tok::Num(v)) => {
                let v = *v;
                self.advance();
                Some(v)
            }
            _ => None,
        }
    }

    fn expect_end(&self) -> Result<(), ParseError> {
        if self.at_end() {
            return Ok(());
        }
        Err(ParseError {
            message: "trailing input".into(),
            column: self.column(),
        })
    }

    fn request(&mut self) -> Result<Request, ParseError> {
        let Some(name) = self.word() else {
            return self.error("expected a command");
        };
        match name.as_str() {
            "break" | "b" | "br" => self.parse_break(),
            "delete" | "del" | "d" => Ok(Request::Delete(self.ids()?)),
            "enable" => Ok(Request::Enable(self.ids()?)),
            "disable" => Ok(Request::Disable(self.ids()?)),
            "watch" | "w" => Ok(Request::Watch {
                addr: self.addr()?,
                read: false,
                write: true,
            }),
            "rwatch" => Ok(Request::Watch {
                addr: self.addr()?,
                read: true,
                write: false,
            }),
            "awatch" => Ok(Request::Watch {
                addr: self.addr()?,
                read: true,
                write: true,
            }),
            "pwatch" => self.parse_port_watch(),
            "step" | "s" | "stepi" | "si" => Ok(Request::Step(self.count(1)?)),
            "next" | "n" | "nexti" | "ni" => Ok(Request::Next(self.count(1)?)),
            "finish" | "fin" => Ok(Request::Finish),
            "continue" | "cont" | "c" => Ok(Request::Continue),
            "until" | "advance" | "u" => Ok(Request::Until(self.addr()?)),
            "run" | "r" => Ok(Request::Run(self.optional_addr()?)),
            "reset" => Ok(Request::Reset),
            "regs" | "registers" => Ok(Request::Regs),
            "x" => self.parse_examine(),
            "disas" | "disassemble" | "dis" => self.parse_disas(),
            "trace" | "t" => Ok(Request::Trace(self.count(16)?)),
            "info" | "i" => self.parse_info(),
            "poke" => self.parse_poke(),
            "help" | "h" => Ok(Request::Help),
            "quit" | "q" | "exit" => Ok(Request::Quit),
            other => Err(ParseError {
                message: format!("unknown command `{other}`"),
                column: self.tokens[self.i - 1].col,
            }),
        }
    }

    fn parse_break(&mut self) -> Result<Request, ParseError> {
        let addr = self.addr()?;
        let condition = if self.eat_word("if") {
            Some(self.condition()?)
        } else {
            None
        };
        Ok(Request::Break { addr, condition })
    }

    fn parse_port_watch(&mut self) -> Result<Request, ParseError> {
        let value = self.u16_value()?;
        let mask = match self.peek() {
            Some(Tok::Num(_)) => self.u16_value()?,
            // No mask given means the port is fully decoded, which is the
            // literal reading of "watch this port".
            _ => 0xFFFF,
        };
        let (on_in, on_out) = match self.word().as_deref() {
            None => (true, true),
            Some("in") => (true, false),
            Some("out") => (false, true),
            Some("inout") => (true, true),
            Some(other) => {
                return Err(ParseError {
                    message: format!("expected `in`, `out` or `inout`, found `{other}`"),
                    column: self.tokens[self.i - 1].col,
                });
            }
        };
        Ok(Request::PortWatch {
            value,
            mask,
            on_in,
            on_out,
        })
    }

    fn parse_examine(&mut self) -> Result<Request, ParseError> {
        let mut count = None;
        let mut format = Format::Hex;
        let mut unit = Unit::Byte;
        if self.eat_punct("/") {
            count = self.number();
            // Only a word glued to the spec is part of it. `x/4 hl+2` examines
            // through `HL`, and reading `hl` as format letters would be a
            // command that silently did something else.
            if self.adjacent()
                && let Some(letters) = self.word()
            {
                let letters_at = self.tokens[self.i - 1].col;
                for (n, c) in letters.chars().enumerate() {
                    match c {
                        'x' => format = Format::Hex,
                        'd' => format = Format::Dec,
                        'u' => format = Format::Unsigned,
                        't' => format = Format::Binary,
                        'c' => format = Format::Char,
                        's' => format = Format::Str,
                        'b' => unit = Unit::Byte,
                        'w' => unit = Unit::Word,
                        other => {
                            return Err(ParseError {
                                message: format!(
                                    "unknown format letter `{other}`: formats are xdutcs, units bw"
                                ),
                                column: letters_at + n,
                            });
                        }
                    }
                }
            }
        }
        let addr = self.addr()?;
        Ok(Request::Examine {
            addr,
            // A string is a variable-length thing and one of them is a
            // sensible ask; a row of hex is not, and sixteen is a row.
            count: count.unwrap_or(if format == Format::Str { 1 } else { 16 }),
            format,
            unit,
        })
    }

    fn parse_disas(&mut self) -> Result<Request, ParseError> {
        let addr = self.optional_addr()?;
        let count = self.count(8)?;
        Ok(Request::Disas { addr, count })
    }

    fn parse_info(&mut self) -> Result<Request, ParseError> {
        match self.word().as_deref() {
            Some("breakpoints") | Some("break") | Some("b") | Some("watchpoints") | Some("w")
            | None => Ok(Request::Info(Info::Breakpoints)),
            Some("registers") | Some("regs") | Some("r") => Ok(Request::Info(Info::Registers)),
            Some(other) => Err(ParseError {
                message: format!("expected `breakpoints` or `registers`, found `{other}`"),
                column: self.tokens[self.i - 1].col,
            }),
        }
    }

    fn parse_poke(&mut self) -> Result<Request, ParseError> {
        let addr = self.addr()?;
        let column = self.column();
        let value = self.u16_value()?;
        if value > 0xFF {
            return Err(ParseError {
                message: format!("${value:04X} is not a byte"),
                column,
            });
        }
        Ok(Request::Poke {
            addr,
            value: value as u8,
        })
    }

    /// A list of breakpoint ids, possibly empty.
    fn ids(&mut self) -> Result<Vec<Id>, ParseError> {
        let mut ids = Vec::new();
        while let Some(Tok::Num(_)) = self.peek() {
            let column = self.column();
            let value = self.number().expect("just peeked a number");
            if value == 0 {
                return Err(ParseError {
                    message: "breakpoint ids start at 1".into(),
                    column,
                });
            }
            ids.push(value);
            self.eat_punct(",");
        }
        Ok(ids)
    }

    /// A repeat count, defaulting when absent.
    fn count(&mut self, default: u32) -> Result<u32, ParseError> {
        match self.peek() {
            Some(Tok::Num(_)) => {
                let column = self.column();
                let value = self.number().expect("just peeked a number");
                if value == 0 {
                    return Err(ParseError {
                        message: "a count of zero does nothing".into(),
                        column,
                    });
                }
                Ok(value)
            }
            _ => Ok(default),
        }
    }

    fn optional_addr(&mut self) -> Result<Option<Addr>, ParseError> {
        match self.peek() {
            Some(Tok::Num(_)) => Ok(Some(self.addr()?)),
            Some(Tok::Word(w)) if base_named(w).is_some() => Ok(Some(self.addr()?)),
            _ => Ok(None),
        }
    }

    /// `$8000`, `hl`, `pc-8`, `ix+4`.
    fn addr(&mut self) -> Result<Addr, ParseError> {
        let column = self.column();
        let base = match self.peek() {
            Some(Tok::Num(_)) => {
                let value = self.u16_value()?;
                Base::Abs(value)
            }
            Some(Tok::Word(w)) => match base_named(w) {
                Some(base) => {
                    self.advance();
                    base
                }
                None => {
                    return Err(ParseError {
                        message: format!("`{w}` is not an address or a register"),
                        column,
                    });
                }
            },
            _ => return self.error("expected an address"),
        };
        let mut offset = 0i32;
        loop {
            let sign = if self.eat_punct("+") {
                1
            } else if self.eat_punct("-") {
                -1
            } else {
                break;
            };
            let column = self.column();
            let Some(value) = self.number() else {
                return Err(ParseError {
                    message: "expected a number after the sign".into(),
                    column,
                });
            };
            offset += sign * value as i32;
        }
        Ok(Addr { base, offset })
    }

    fn u16_value(&mut self) -> Result<u16, ParseError> {
        let column = self.column();
        let Some(value) = self.number() else {
            return Err(ParseError {
                message: "expected a number".into(),
                column,
            });
        };
        if value > 0xFFFF {
            return Err(ParseError {
                message: format!("{value} does not fit in 16 bits"),
                column,
            });
        }
        Ok(value as u16)
    }

    // ---- Conditions ------------------------------------------------------

    fn condition(&mut self) -> Result<Condition, ParseError> {
        let mut terms = vec![self.condition_and()?];
        while self.eat_punct("||") {
            terms.push(self.condition_and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().expect("one term")
        } else {
            Condition::Any(terms)
        })
    }

    fn condition_and(&mut self) -> Result<Condition, ParseError> {
        let mut terms = vec![self.condition_unary()?];
        while self.eat_punct("&&") {
            terms.push(self.condition_unary()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().expect("one term")
        } else {
            Condition::All(terms)
        })
    }

    fn condition_unary(&mut self) -> Result<Condition, ParseError> {
        if self.eat_punct("!") {
            return Ok(Condition::Not(Box::new(self.condition_unary()?)));
        }
        // `(` is grouping and only grouping: memory is `[hl]`, so there is
        // nothing here to disambiguate.
        if self.eat_punct("(") {
            let inner = self.condition()?;
            if !self.eat_punct(")") {
                return self.error("expected `)`");
            }
            return Ok(inner);
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Condition, ParseError> {
        let lhs = self.operand()?;
        let cmp = match self.peek() {
            Some(Tok::Punct("==")) | Some(Tok::Punct("=")) => Cmp::Eq,
            Some(Tok::Punct("!=")) => Cmp::Ne,
            Some(Tok::Punct("<")) => Cmp::Lt,
            Some(Tok::Punct("<=")) => Cmp::Le,
            Some(Tok::Punct(">")) => Cmp::Gt,
            Some(Tok::Punct(">=")) => Cmp::Ge,
            _ => return self.error("expected a comparison"),
        };
        self.advance();
        let rhs = self.operand()?;
        Ok(Condition::Compare { lhs, cmp, rhs })
    }

    fn operand(&mut self) -> Result<Operand, ParseError> {
        let column = self.column();
        match self.peek().cloned() {
            Some(Tok::Num(_)) => Ok(Operand::Imm(self.u16_value()?)),
            Some(Tok::Punct("[")) => {
                self.advance();
                self.memory_operand(false, column)
            }
            Some(Tok::Word(w)) => {
                self.advance();
                // `w[...]` is the sixteen-bit read; the bare form is a byte.
                if w == "w" && self.eat_punct("[") {
                    return self.memory_operand(true, column);
                }
                if let Some(mask) = flag_named(&w) {
                    return Ok(Operand::Flag(mask));
                }
                if let Some(r) = reg8_named(&w) {
                    return Ok(Operand::Reg8(r));
                }
                if let Some(r) = reg16_named(&w) {
                    return Ok(Operand::Reg16(r));
                }
                Err(ParseError {
                    message: format!("`{w}` is not a register, flag or number"),
                    column,
                })
            }
            _ => Err(ParseError {
                message: "expected a register, flag, number or `[address]`".into(),
                column,
            }),
        }
    }

    /// The inside of `[...]`, having already consumed the bracket.
    fn memory_operand(&mut self, wide: bool, column: usize) -> Result<Operand, ParseError> {
        let inner_column = self.column();
        let operand = match self.peek().cloned() {
            Some(Tok::Num(_)) => {
                let addr = self.u16_value()?;
                if wide {
                    Operand::Mem16(addr)
                } else {
                    Operand::Mem8(addr)
                }
            }
            Some(Tok::Word(w)) => {
                self.advance();
                let Some(r) = reg16_named(&w) else {
                    return Err(ParseError {
                        message: format!("`{w}` is not a register pair"),
                        column: inner_column,
                    });
                };
                if wide {
                    return Err(ParseError {
                        message: "a sixteen-bit read through a register pair is not supported"
                            .into(),
                        column,
                    });
                }
                Operand::Mem8At(r)
            }
            _ => {
                return Err(ParseError {
                    message: "expected an address or a register pair".into(),
                    column: inner_column,
                });
            }
        };
        if !self.eat_punct("]") {
            return self.error("expected `]`");
        }
        Ok(operand)
    }
}

fn base_named(word: &str) -> Option<Base> {
    if word == "pc" {
        return Some(Base::Pc);
    }
    reg16_named(word).map(Base::Reg)
}

fn reg16_named(word: &str) -> Option<Reg16> {
    Some(match word {
        "af" => Reg16::Af,
        "bc" => Reg16::Bc,
        "de" => Reg16::De,
        "hl" => Reg16::Hl,
        "sp" => Reg16::Sp,
        "ix" => Reg16::Ix,
        "iy" => Reg16::Iy,
        _ => return None,
    })
}

fn reg8_named(word: &str) -> Option<Reg8> {
    Some(match word {
        "a" => Reg8::A,
        "b" => Reg8::B,
        "c" => Reg8::C,
        "d" => Reg8::D,
        "e" => Reg8::E,
        "h" => Reg8::H,
        "l" => Reg8::L,
        "ixh" => Reg8::Ixh,
        "ixl" => Reg8::Ixl,
        "iyh" => Reg8::Iyh,
        "iyl" => Reg8::Iyl,
        _ => return None,
    })
}

/// Flags are written `f.z` rather than `z`, because `c` is a register as well
/// as a flag and a condition that quietly meant the other one would be a bug
/// nobody could see.
fn flag_named(word: &str) -> Option<u8> {
    Some(match word {
        "f.s" => flag::S,
        "f.z" => flag::Z,
        "f.y" => flag::Y,
        "f.h" => flag::H,
        "f.x" => flag::X,
        "f.p" | "f.pv" | "f.v" => flag::P,
        "f.n" => flag::N,
        "f.c" => flag::C,
        _ => return None,
    })
}
