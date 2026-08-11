#![allow(dead_code)]
//! A minimal assembler driver, shared by the tests that need whole programs.
//!
//! It is the loop described in ADR-0014: walk the statements assigning
//! addresses, repeat until nothing moves, then keep the last pass's output. The
//! instruction encoding it drives is real; the handful of directives it
//! understands (`ORG`, `EQU`, `=`, `DB`, `DW`, `DS`) are the minimum needed to
//! write a program that does something, and ticket 0004 replaces them with the
//! full set.

use rkw_asm::ast::{Expr, ExprKind, Statement};
use rkw_asm::encode::{self, EncodeError};
use rkw_asm::{Diagnostic, FileId, Site, SourceMap, Symbols, eval, parse};

pub struct Assembly {
    /// The assembled bytes, starting at [`Assembly::origin`].
    pub bytes: Vec<u8>,
    pub origin: u16,
    pub symbols: Symbols,
    pub errors: Vec<Diagnostic>,
    pub passes: u32,
}

impl Assembly {
    pub fn symbol(&mut self, name: &str) -> i64 {
        self.symbols
            .iter_values()
            .into_iter()
            .find(|(defined, _)| defined == name)
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("`{name}` is not defined"))
    }
}

/// One walk over the source.
struct Pass<'a> {
    symbols: &'a mut Symbols,
    address: i64,
    section: i64,
    seq: u64,
    /// The whole address space, so an `ORG` anywhere lands in the right place
    /// without the driver having to model sections.
    image: Vec<u8>,
    lowest: i64,
    highest: i64,
    errors: Vec<Diagnostic>,
}

/// What a pass produced, with no borrow of the symbol table left in it.
struct Output {
    image: Vec<u8>,
    lowest: i64,
    highest: i64,
    errors: Vec<Diagnostic>,
}

impl<'a> Pass<'a> {
    fn run(symbols: &'a mut Symbols, statements: &[Statement]) -> Output {
        let mut pass = Pass {
            symbols,
            address: 0,
            section: 0,
            seq: 0,
            image: vec![0; 0x1_0000],
            lowest: i64::MAX,
            highest: i64::MIN,
            errors: Vec::new(),
        };
        for statement in statements {
            pass.statement(statement);
        }
        Output {
            image: pass.image,
            lowest: pass.lowest,
            highest: pass.highest,
            errors: pass.errors,
        }
    }

    fn statement(&mut self, statement: &Statement) {
        self.seq += 1;
        let site = Site::new(self.address, self.section, self.seq);
        let name = statement
            .op
            .as_ref()
            .map(|op| op.name.to_ascii_lowercase())
            .unwrap_or_default();
        let args: &[Expr] = statement.op.as_ref().map(|op| &op.args[..]).unwrap_or(&[]);

        match name.as_str() {
            "equ" | "=" | "defl" => {
                let label = statement.label.as_ref().expect("a name to define");
                if let Err(e) = self.symbols.define_const(
                    &label.name,
                    args[0].clone(),
                    site,
                    label.span,
                    name != "equ",
                ) {
                    self.errors.push(e);
                }
                return;
            }
            _ => {
                if let Some(label) = &statement.label {
                    if let Err(e) = self.symbols.define_label(label, self.address, self.seq) {
                        self.errors.push(e);
                    }
                }
            }
        }

        match name.as_str() {
            "" => {}
            "org" => {
                self.address = self.value(&args[0], site).unwrap_or(self.address);
                self.section = self.address;
            }
            "db" | "defb" | "dm" | "defm" | "byte" => {
                for arg in args {
                    match &arg.kind {
                        ExprKind::Str(s) if s.value.len() != 1 => {
                            let bytes = s.value.clone();
                            self.write(&bytes);
                        }
                        _ => {
                            let value = self.value(arg, site).unwrap_or(0);
                            match rkw_asm::eval::fit_byte(value, arg.span) {
                                Ok(b) => self.write(&[b]),
                                Err(e) => {
                                    self.errors.push(e.diagnostic());
                                    self.write(&[0]);
                                }
                            }
                        }
                    }
                }
            }
            "dw" | "defw" | "word" => {
                for arg in args {
                    let value = self.value(arg, site).unwrap_or(0) as u16;
                    self.write(&value.to_le_bytes());
                }
            }
            "ds" | "defs" | "block" => {
                let count = self.value(&args[0], site).unwrap_or(0);
                let fill = match args.get(1) {
                    Some(e) => self.value(e, site).unwrap_or(0) as u8,
                    None => 0,
                };
                self.write(&vec![fill; count.max(0) as usize]);
            }
            _ => self.instruction(statement, site),
        }
    }

    fn instruction(&mut self, statement: &Statement, site: Site) {
        let op = statement.op.as_ref().expect("an operation");
        // The plan gives the length without evaluating anything, so the address
        // advances correctly even when an operand is still unknown.
        let plan = match encode::plan(op) {
            Ok(plan) => plan,
            Err(e) => {
                self.errors.push(e);
                return;
            }
        };
        let length = plan.len();
        match encode::emit(&plan, site, self.symbols) {
            Ok(bytes) => self.write(&bytes),
            Err(e) => {
                if !e.is_forward_reference() {
                    self.errors.push(e.diagnostic());
                }
                self.address += length as i64;
            }
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let at = self.address & 0xFFFF;
            self.image[at as usize] = byte;
            self.lowest = self.lowest.min(self.address);
            self.highest = self.highest.max(self.address + 1);
            self.address += 1;
        }
    }

    fn value(&mut self, expr: &Expr, site: Site) -> Option<i64> {
        match eval(expr, site, self.symbols) {
            Ok(v) => Some(v),
            Err(e) => {
                if !e.is_forward_reference() {
                    self.errors.push(e.diagnostic());
                }
                None
            }
        }
    }
}

/// Assemble to a fixpoint.
pub fn assemble(map: &SourceMap, file: FileId) -> Assembly {
    const LIMIT: u32 = 8;

    let parsed = parse(map, file);
    assert!(
        parsed.diagnostics.is_empty(),
        "source did not parse:\n{}",
        map.render_all(&parsed.diagnostics)
    );

    let mut symbols = Symbols::new();
    loop {
        let output = Pass::run(&mut symbols, &parsed.program.statements);
        if (symbols.converged() && symbols.pass() >= 2) || symbols.pass() >= LIMIT {
            let (lowest, highest) = (output.lowest, output.highest);
            let bytes = match lowest <= highest {
                true => output.image[lowest as usize..highest as usize].to_vec(),
                false => Vec::new(),
            };
            return Assembly {
                bytes,
                origin: lowest.max(0) as u16,
                passes: symbols.pass(),
                symbols,
                errors: output.errors,
            };
        }
        symbols.begin_pass();
    }
}

/// Assemble one instruction at `at`, which is what the round-trip test needs.
pub fn assemble_one(text: &str, at: u16) -> Result<Vec<u8>, EncodeError> {
    let mut map = SourceMap::new();
    let file = map.add("one.asm", format!("    {text}\n"));
    let parsed = parse(&map, file);
    assert!(
        parsed.diagnostics.is_empty(),
        "{text:?} did not parse:\n{}",
        map.render_all(&parsed.diagnostics)
    );
    let op = parsed.program.statements[0]
        .op
        .as_ref()
        .expect("an operation");
    let mut symbols = Symbols::new();
    encode::encode(op, Site::new(i64::from(at), i64::from(at), 1), &mut symbols)
}
