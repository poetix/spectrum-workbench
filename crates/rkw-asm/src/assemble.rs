//! The assembler proper: walk the statements, place the bytes.
//!
//! This is the loop ADR-0014 describes. Each pass walks every statement in
//! source order, defining labels at the address the location counter has
//! reached and emitting bytes; passes repeat until the symbol table stops
//! changing. Only the last pass's output is kept, because only it saw every
//! symbol.
//!
//! Three things make that loop more than a `for`:
//!
//! * **Conditional assembly.** `IF` on a forward reference cannot be decided on
//!   the first pass, so an unresolved condition is taken as false and the
//!   fixpoint sorts it out — the symbols it moves are exactly what tells the
//!   driver to go round again.
//! * **`INCLUDE`.** Included files are parsed once and cached, and the include
//!   stack is carried through the walk so that a cycle is reported with the
//!   chain that produced it rather than as a stack overflow.
//! * **Everything is re-executed every pass**, including `MODULE` scoping and
//!   redefinable variables, so state that persists between passes is a bug.
//!   Only the symbol table survives, and only so that forward references can
//!   resolve.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::ast::{Expr, ExprKind, Program, Statement};
use crate::diag::Diagnostic;
use crate::encode;
use crate::eval::{Site, eval, fit_byte, fit_word};
use crate::source::{FileId, SourceMap, Span};
use crate::symbols::Symbols;

/// How many passes to make before giving up on a source that will not settle.
const PASS_LIMIT: u32 = 8;

/// A run of assembled bytes and the address they belong at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub origin: u16,
    pub bytes: Vec<u8>,
}

/// What a program assembled to.
///
/// Held as the whole address space rather than as a list of writes, because
/// `ORG` may go backwards to patch something already emitted and the last write
/// to an address is the one that counts.
pub struct Image {
    bytes: Box<[u8; 0x1_0000]>,
    written: Box<[bool; 0x1_0000]>,
}

impl Default for Image {
    fn default() -> Self {
        Self::new()
    }
}

impl Image {
    pub fn new() -> Self {
        Self {
            bytes: Box::new([0; 0x1_0000]),
            written: Box::new([false; 0x1_0000]),
        }
    }

    fn put(&mut self, address: u16, byte: u8) {
        self.bytes[address as usize] = byte;
        self.written[address as usize] = true;
    }

    pub fn is_empty(&self) -> bool {
        !self.written.iter().any(|&w| w)
    }

    /// The lowest address written to.
    pub fn origin(&self) -> Option<u16> {
        self.written.iter().position(|&w| w).map(|at| at as u16)
    }

    /// The contiguous runs of assembled bytes, lowest first. A program with two
    /// `ORG`s far apart gives two segments rather than a binary with a hole the
    /// size of the gap.
    pub fn segments(&self) -> Vec<Segment> {
        let mut segments = Vec::new();
        let mut at = 0usize;
        while at < self.written.len() {
            if !self.written[at] {
                at += 1;
                continue;
            }
            let start = at;
            while at < self.written.len() && self.written[at] {
                at += 1;
            }
            segments.push(Segment {
                origin: start as u16,
                bytes: self.bytes[start..at].to_vec(),
            });
        }
        segments
    }

    /// Everything from the lowest written address to the highest, gaps
    /// included — what a raw binary for a single load address needs.
    pub fn to_binary(&self) -> Vec<u8> {
        let first = self.written.iter().position(|&w| w);
        let last = self.written.iter().rposition(|&w| w);
        match (first, last) {
            (Some(first), Some(last)) => self.bytes[first..=last].to_vec(),
            _ => Vec::new(),
        }
    }

    pub fn byte_at(&self, address: u16) -> u8 {
        self.bytes[address as usize]
    }

    /// Write the assembled bytes to a file as a raw binary: no header, no
    /// origin, exactly what a `LOAD ""CODE` or an emulator's "load at address"
    /// expects. The origin the caller needs is [`Image::origin`].
    pub fn write_binary(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, self.to_binary())
    }
}

/// Where one statement's bytes ended up.
///
/// The raw material for the listing and the debug info sidecar in ticket 0006;
/// the file and line come from the span, so this stays one record per statement
/// rather than a format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRecord {
    pub span: Span,
    pub address: u16,
    pub length: u16,
}

pub struct Assembled {
    pub image: Image,
    pub symbols: Symbols,
    pub diagnostics: Vec<Diagnostic>,
    /// One per statement that emitted bytes, in address order within a pass.
    pub lines: Vec<LineRecord>,
    pub passes: u32,
}

impl Assembled {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Assemble `root` and everything it includes.
pub fn assemble(map: &mut SourceMap, root: FileId) -> Assembled {
    let mut assembler = Assembler {
        map,
        symbols: Symbols::new(),
        programs: HashMap::new(),
        by_path: HashMap::new(),
        binaries: HashMap::new(),
        parse_diagnostics: Vec::new(),
    };

    loop {
        let mut state = State::new(root);
        assembler.walk(root, &mut state);
        state.finish();

        let settled = assembler.symbols.converged() && assembler.symbols.pass() >= 2;
        let exhausted = assembler.symbols.pass() >= PASS_LIMIT;
        if settled || exhausted {
            if exhausted && !settled {
                state
                    .diagnostics
                    .push(non_convergence(&assembler.symbols, root));
            }
            // Parse errors first, so a file whose only problems are syntactic
            // reads in source order.
            let mut diagnostics = assembler.parse_diagnostics;
            diagnostics.append(&mut state.diagnostics);
            return Assembled {
                image: state.image,
                lines: state.lines,
                diagnostics,
                passes: assembler.symbols.pass(),
                symbols: assembler.symbols,
            };
        }
        assembler.symbols.begin_pass();
    }
}

fn non_convergence(symbols: &Symbols, root: FileId) -> Diagnostic {
    let still_moving = symbols.unsettled().join(", ");
    Diagnostic::error(
        Span::at(root, 0),
        format!("assembly did not settle after {PASS_LIMIT} passes"),
    )
    .with_note(format!("still moving: {still_moving}"))
    .with_note("a conditional or a reserved block whose size depends on itself will do this")
}

/// One branch of an `IF`, `IFDEF` or `IFN`.
struct Conditional {
    /// Whether this branch is the selected one. Independent of whether the
    /// enclosing conditionals are, so that `ELSE` inside a skipped block still
    /// balances.
    taken: bool,
    /// Whether some branch of this conditional has already been taken, which
    /// is what stops `ELSE` reviving one that was.
    done: bool,
    span: Span,
}

/// The state of one pass over the source.
struct State {
    address: i64,
    section: i64,
    seq: u64,
    image: Image,
    lines: Vec<LineRecord>,
    /// How many bytes this pass has emitted, which is how a statement knows
    /// whether it produced any. `ORG` moves the address without emitting.
    emitted: u64,
    diagnostics: Vec<Diagnostic>,
    conditionals: Vec<Conditional>,
    /// The files currently being walked, innermost last, for relative paths
    /// and for reporting an include cycle with its chain.
    includes: Vec<(FileId, Span)>,
    /// Set by `END`, which stops assembly wherever it appears.
    finished: bool,
}

impl State {
    fn new(root: FileId) -> Self {
        Self {
            address: 0,
            section: 0,
            seq: 0,
            image: Image::new(),
            lines: Vec::new(),
            emitted: 0,
            diagnostics: Vec::new(),
            conditionals: Vec::new(),
            includes: vec![(root, Span::at(root, 0))],
            finished: false,
        }
    }

    /// True when statements are being assembled rather than skipped.
    fn active(&self) -> bool {
        self.conditionals.iter().all(|c| c.taken)
    }

    fn current_file(&self) -> FileId {
        self.includes.last().expect("a file is being walked").0
    }

    fn site(&self) -> Site {
        Site::new(self.address, self.section, self.seq)
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.image.put((self.address & 0xFFFF) as u16, byte);
            self.address += 1;
            self.emitted += 1;
        }
    }

    fn finish(&mut self) {
        for open in std::mem::take(&mut self.conditionals) {
            self.diagnostics.push(Diagnostic::error(
                open.span,
                "this conditional is never closed",
            ));
        }
    }
}

struct Assembler<'a> {
    map: &'a mut SourceMap,
    symbols: Symbols,
    /// Parsed once and reused on every pass; parsing is the one thing that
    /// genuinely cannot change between them.
    programs: HashMap<FileId, Rc<Program>>,
    by_path: HashMap<PathBuf, FileId>,
    binaries: HashMap<PathBuf, Rc<Vec<u8>>>,
    /// Reported once, when a file is first parsed, rather than once per pass.
    parse_diagnostics: Vec<Diagnostic>,
}

impl Assembler<'_> {
    fn program(&mut self, file: FileId) -> Rc<Program> {
        if let Some(program) = self.programs.get(&file) {
            return Rc::clone(program);
        }
        let parsed = crate::parse::parse(self.map, file);
        let program = Rc::new(parsed.program);
        self.programs.insert(file, Rc::clone(&program));
        self.parse_diagnostics.extend(parsed.diagnostics);
        program
    }

    fn walk(&mut self, file: FileId, state: &mut State) {
        let program = self.program(file);
        for statement in &program.statements {
            if state.finished {
                return;
            }
            self.statement(statement, state);
        }
    }

    fn statement(&mut self, statement: &Statement, state: &mut State) {
        state.seq += 1;
        let name = statement
            .op
            .as_ref()
            .map(|op| op.name.trim_start_matches('.').to_ascii_lowercase())
            .unwrap_or_default();
        let args: &[Expr] = statement.op.as_ref().map(|op| &op.args[..]).unwrap_or(&[]);
        let span = statement.span;

        // Conditionals are read even inside a block that is being skipped, or
        // the ENDIF that closes the skipped block would close the wrong one.
        match name.as_str() {
            "if" | "ifn" | "ifdef" | "ifndef" => {
                return self.open_conditional(&name, args, span, state);
            }
            "else" | "elseif" => return self.else_branch(&name, args, span, state),
            "endif" => {
                if state.conditionals.pop().is_none() {
                    state.error(span, "`ENDIF` with no `IF`");
                }
                return;
            }
            _ => {}
        }
        if !state.active() {
            return;
        }

        // A constant takes the name to its left; everything else labels the
        // address the location counter has reached.
        if matches!(name.as_str(), "equ" | "defl" | "=") {
            return self.define_constant(statement, &name, args, span, state);
        }
        if let Some(label) = &statement.label {
            if let Err(e) = self.symbols.define_label(label, state.address, state.seq) {
                state.diagnostics.push(e);
            }
        }

        let start = state.address;
        let before = state.emitted;
        self.directive_or_instruction(statement, &name, args, span, state);
        if state.emitted > before {
            state.lines.push(LineRecord {
                span,
                address: (start & 0xFFFF) as u16,
                length: (state.emitted - before) as u16,
            });
        }
    }

    fn directive_or_instruction(
        &mut self,
        statement: &Statement,
        name: &str,
        args: &[Expr],
        span: Span,
        state: &mut State,
    ) {
        match name {
            "" => {}
            "org" => {
                if let Some(address) = self.value(&args[0], state) {
                    state.address = address;
                    state.section = address;
                }
            }
            "align" => self.align(args, span, state),
            "ds" | "defs" | "block" => self.reserve(args, span, state),
            "db" | "defb" | "dm" | "defm" | "byte" => self.data(args, false, state),
            "dz" | "defz" => self.data(args, true, state),
            "dw" | "defw" | "word" => self.words(args, state),
            "module" => self.enter_module(args, span, state),
            "endmodule" | "endmod" => self.symbols.leave_module(),
            "include" => self.include(args, span, state),
            "incbin" => self.incbin(args, span, state),
            "end" => state.finished = true,
            _ => self.instruction(statement, state),
        }
    }

    fn instruction(&mut self, statement: &Statement, state: &mut State) {
        let op = statement.op.as_ref().expect("an operation");
        // The plan gives the length without evaluating anything, so the address
        // advances correctly even when an operand cannot be resolved yet.
        let plan = match encode::plan(op) {
            Ok(plan) => plan,
            Err(e) => {
                state.diagnostics.push(e);
                return;
            }
        };
        let length = plan.len() as i64;
        match encode::emit(&plan, state.site(), &mut self.symbols) {
            Ok(bytes) => state.write(&bytes),
            Err(e) => {
                if !e.is_forward_reference() {
                    state.diagnostics.push(e.diagnostic());
                }
                state.address += length;
            }
        }
    }

    // -- layout -------------------------------------------------------------

    fn align(&mut self, args: &[Expr], span: Span, state: &mut State) {
        let Some(boundary) = args.first().and_then(|a| self.value(a, state)) else {
            return state.error(span, "`ALIGN` needs a boundary");
        };
        if boundary <= 0 || boundary & (boundary - 1) != 0 {
            return state.error(span, format!("`ALIGN {boundary}` is not a power of two"));
        }
        let fill = self.fill_byte(args.get(1), state);
        let padding = (boundary - (state.address % boundary)) % boundary;
        state.write(&vec![fill; padding as usize]);
    }

    fn reserve(&mut self, args: &[Expr], span: Span, state: &mut State) {
        let Some(count) = args.first().and_then(|a| self.value(a, state)) else {
            // On the first pass the size may not be known yet; the fixpoint
            // will come back once it is.
            return;
        };
        if count < 0 {
            return state.error(span, format!("`DS {count}` reserves a negative amount"));
        }
        let fill = self.fill_byte(args.get(1), state);
        state.write(&vec![fill; count as usize]);
    }

    fn fill_byte(&mut self, arg: Option<&Expr>, state: &mut State) -> u8 {
        arg.and_then(|e| self.value(e, state))
            .map(|v| v as u8)
            .unwrap_or(0)
    }

    // -- data ---------------------------------------------------------------

    fn data(&mut self, args: &[Expr], zero_terminated: bool, state: &mut State) {
        for arg in args {
            // A string is its bytes; anything else is one byte, including the
            // one-character literal that `'a'+1` is built from.
            if let ExprKind::Str(literal) = &arg.kind {
                let bytes = literal.value.clone();
                state.write(&bytes);
                continue;
            }
            let value = self.value(arg, state).unwrap_or(0);
            match fit_byte(value, arg.span) {
                Ok(byte) => state.write(&[byte]),
                Err(e) => {
                    state.diagnostics.push(e.diagnostic());
                    state.write(&[0]);
                }
            }
        }
        if zero_terminated {
            state.write(&[0]);
        }
    }

    fn words(&mut self, args: &[Expr], state: &mut State) {
        for arg in args {
            let value = self.value(arg, state).unwrap_or(0);
            match fit_word(value, arg.span) {
                Ok(word) => state.write(&word.to_le_bytes()),
                Err(e) => {
                    state.diagnostics.push(e.diagnostic());
                    state.write(&[0, 0]);
                }
            }
        }
    }

    // -- symbols ------------------------------------------------------------

    fn define_constant(
        &mut self,
        statement: &Statement,
        name: &str,
        args: &[Expr],
        span: Span,
        state: &mut State,
    ) {
        let Some(label) = &statement.label else {
            return state.error(span, format!("`{name}` needs a name to its left"));
        };
        let Some(value) = args.first() else {
            return state.error(span, format!("`{name}` needs a value"));
        };
        // `EQU` is held as an expression and evaluated when something asks
        // for it, so it may refer forwards. `DEFL` is evaluated now, so that
        // `count DEFL count+1` sees the value the name has at this point.
        let result = if name == "equ" {
            self.symbols
                .define_const(&label.name, value.clone(), state.site(), label.span, false)
        } else {
            match self.value(value, state) {
                Some(value) => self.symbols.define_variable(&label.name, value, label.span),
                None => return,
            }
        };
        if let Err(e) = result {
            state.diagnostics.push(e);
        }
    }

    fn enter_module(&mut self, args: &[Expr], span: Span, state: &mut State) {
        match args.first().and_then(Expr::as_ident) {
            Some(name) => self.symbols.enter_module(name),
            None => state.error(span, "`MODULE` needs a name"),
        }
    }

    // -- conditionals -------------------------------------------------------

    fn open_conditional(&mut self, name: &str, args: &[Expr], span: Span, state: &mut State) {
        let taken = if state.active() {
            self.condition(name, args, span, state)
        } else {
            // Not evaluated at all inside a skipped block: the symbols it names
            // may not exist on this branch, and complaining about them would be
            // complaining about code that is not being assembled.
            false
        };
        state.conditionals.push(Conditional {
            taken,
            done: taken,
            span,
        });
    }

    fn condition(&mut self, name: &str, args: &[Expr], span: Span, state: &mut State) -> bool {
        let Some(arg) = args.first() else {
            state.error(span, format!("`{name}` needs a condition"));
            return false;
        };
        match name {
            "ifdef" | "ifndef" => {
                let Some(symbol) = arg.as_ident() else {
                    state.error(arg.span, format!("`{name}` needs a symbol name"));
                    return false;
                };
                let defined = self.symbols.is_defined(symbol);
                defined == (name == "ifdef")
            }
            // An unresolved condition is false for now. On the first pass that
            // is a forward reference and the fixpoint will come back to it; on
            // a later one `value` has already reported it as undefined.
            "ifn" => self.value(arg, state).unwrap_or(0) == 0,
            _ => self.value(arg, state).unwrap_or(0) != 0,
        }
    }

    fn else_branch(&mut self, name: &str, args: &[Expr], span: Span, state: &mut State) {
        let Some(open) = state.conditionals.pop() else {
            return state.error(span, format!("`{}` with no `IF`", name.to_uppercase()));
        };
        let outer_active = state.active();
        let taken = match (open.done, name) {
            (true, _) => false,
            (false, "elseif") if outer_active => self.condition("if", args, span, state),
            (false, "elseif") => false,
            (false, _) => true,
        };
        state.conditionals.push(Conditional {
            taken,
            done: open.done || taken,
            span: open.span,
        });
    }

    // -- files --------------------------------------------------------------

    fn include(&mut self, args: &[Expr], span: Span, state: &mut State) {
        let Some(path) = self.path_argument(args, span, state) else {
            return;
        };

        if let Some(at) = state
            .includes
            .iter()
            .position(|(file, _)| self.map.file(*file).path() == Some(path.as_path()))
        {
            let mut diagnostic = Diagnostic::error(
                span,
                format!("`{}` is already being included", path.display()),
            );
            for (file, from) in &state.includes[at..] {
                let name = self.map.file(*file).name().to_string();
                diagnostic = diagnostic.with_related(*from, format!("`{name}` was included"));
            }
            state.diagnostics.push(diagnostic);
            return;
        }

        let file = match self.by_path.get(&path) {
            Some(file) => *file,
            None => match self.map.load(&path) {
                Ok(file) => {
                    self.by_path.insert(path.clone(), file);
                    file
                }
                Err(e) => {
                    return state.error(span, format!("cannot read `{}`: {e}", path.display()));
                }
            },
        };

        state.includes.push((file, span));
        self.walk(file, state);
        state.includes.pop();
    }

    fn incbin(&mut self, args: &[Expr], span: Span, state: &mut State) {
        let Some(path) = self.path_argument(args, span, state) else {
            return;
        };
        let bytes = match self.binaries.get(&path) {
            Some(bytes) => Rc::clone(bytes),
            None => match std::fs::read(&path) {
                Ok(bytes) => {
                    let bytes = Rc::new(bytes);
                    self.binaries.insert(path.clone(), Rc::clone(&bytes));
                    bytes
                }
                Err(e) => {
                    return state.error(span, format!("cannot read `{}`: {e}", path.display()));
                }
            },
        };

        let offset = match args.get(1) {
            Some(e) => self.value(e, state).unwrap_or(0).max(0) as usize,
            None => 0,
        };
        if offset > bytes.len() {
            return state.error(
                span,
                format!(
                    "offset {offset} is past the end of `{}`, which is {} bytes",
                    path.display(),
                    bytes.len()
                ),
            );
        }
        let available = bytes.len() - offset;
        let length = match args.get(2) {
            Some(e) => match self.value(e, state) {
                Some(length) if length as usize > available => {
                    return state.error(
                        span,
                        format!(
                            "`{}` has {available} bytes after offset {offset}, not {length}",
                            path.display()
                        ),
                    );
                }
                Some(length) => length.max(0) as usize,
                None => available,
            },
            None => available,
        };
        state.write(&bytes[offset..offset + length]);
    }

    /// The file a directive names, resolved against the directory of the file
    /// that names it rather than the process working directory.
    fn path_argument(&mut self, args: &[Expr], span: Span, state: &mut State) -> Option<PathBuf> {
        let Some(arg) = args.first() else {
            state.error(span, "expected a file name in quotes");
            return None;
        };
        let ExprKind::Str(literal) = &arg.kind else {
            state.error(arg.span, "expected a file name in quotes");
            return None;
        };
        let Ok(text) = std::str::from_utf8(&literal.value) else {
            state.error(arg.span, "file name is not valid UTF-8");
            return None;
        };

        let relative = Path::new(text);
        if relative.is_absolute() {
            return Some(relative.to_path_buf());
        }
        let base = self.map.file(state.current_file()).directory();
        Some(match base {
            Some(directory) => directory.join(relative),
            None => relative.to_path_buf(),
        })
    }

    // -- values -------------------------------------------------------------

    /// Evaluate, reporting anything except a forward reference — which is not
    /// a complaint but a request for another pass.
    fn value(&mut self, expr: &Expr, state: &mut State) -> Option<i64> {
        match eval(expr, state.site(), &mut self.symbols) {
            Ok(value) => Some(value),
            Err(e) => {
                if !e.is_forward_reference() {
                    state.diagnostics.push(e.diagnostic());
                }
                None
            }
        }
    }
}
