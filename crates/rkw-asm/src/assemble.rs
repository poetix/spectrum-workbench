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

/// How deep macro expansion may nest before it is reported as runaway
/// recursion. The limit exists so that a recursive macro produces a diagnostic
/// naming the chain rather than a stack overflow.
const MAX_EXPANSION_DEPTH: usize = 32;

/// A repetition count beyond which the source is more likely wrong than
/// ambitious.
const MAX_REPETITIONS: i64 = 65_536;

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
    /// The macro expansion this statement was assembled inside, as an index
    /// into [`Assembled::expansions`]. `None` for source written directly.
    pub expansion: Option<usize>,
}

/// One use of a macro.
///
/// A macro body is one set of statements however many times it is used, so
/// which expansion a statement belongs to cannot live on the statement. It is
/// recorded here instead, and the listing in ticket 0006 reads it back to show
/// the nesting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub name: String,
    /// Where the macro was defined.
    pub defined_at: Span,
    /// Where this expansion was asked for.
    pub invoked_at: Span,
    /// The expansion this one happened inside.
    pub parent: Option<usize>,
}

pub struct Assembled {
    pub image: Image,
    pub symbols: Symbols,
    pub diagnostics: Vec<Diagnostic>,
    /// One per statement that emitted bytes, in address order within a pass.
    pub lines: Vec<LineRecord>,
    /// Every macro expansion the final pass performed, in the order they
    /// started. [`LineRecord::expansion`] indexes into this.
    pub expansions: Vec<Expansion>,
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
                expansions: state.expansions,
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

/// A macro definition: its parameters and the statements between `MACRO` and
/// `ENDM`, collected but not assembled.
struct Macro {
    name: String,
    parameters: Vec<String>,
    body: Vec<Statement>,
    /// The `MACRO` line, for the "defined here" half of a two-site error.
    span: Span,
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
    /// Macros defined so far on this pass. Collected afresh every pass, like
    /// everything else that is not the symbol table.
    macros: HashMap<String, Rc<Macro>>,
    expansions: Vec<Expansion>,
    /// The expansions currently being assembled, innermost last.
    expanding: Vec<usize>,
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
            macros: HashMap::new(),
            expansions: Vec::new(),
            expanding: Vec::new(),
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
        self.report(Diagnostic::error(span, message));
    }

    /// Record a diagnostic, naming the expansion it happened inside.
    ///
    /// This is what makes an error in a macro tractable: the diagnostic itself
    /// points into the macro body, which is where the mistake is written, and
    /// the notes point at each invocation that led there, which is what the
    /// reader needs to know to work out why the values were wrong.
    fn report(&mut self, mut diagnostic: Diagnostic) {
        for &index in self.expanding.iter().rev() {
            let expansion = &self.expansions[index];
            diagnostic = diagnostic
                .with_related(
                    expansion.invoked_at,
                    format!("in this expansion of `{}`", expansion.name),
                )
                .with_related(
                    expansion.defined_at,
                    format!("`{}` is defined here", expansion.name),
                );
        }
        self.diagnostics.push(diagnostic);
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
            self.report(Diagnostic::error(
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
        self.run(&program.statements, state);
    }

    /// Assemble a run of statements: a file, a macro body, or the body of a
    /// repetition. Indexed rather than iterated because the block directives
    /// consume statements up to their own terminator.
    fn run(&mut self, statements: &[Statement], state: &mut State) {
        let mut at = 0;
        while at < statements.len() && !state.finished {
            let statement = &statements[at];
            let name = directive_name(statement);
            match name.as_str() {
                // Blocks are collected whether or not they are being
                // assembled: a `REPT` inside a skipped `IF` still has to be
                // stepped over as a unit, or its `ENDR` closes nothing.
                "macro" | "rept" | "dup" => {
                    let Some(end) = block_end(statements, at) else {
                        state.error(statement.span, format!("`{name}` is never closed"));
                        return;
                    };
                    let body = &statements[at + 1..end];
                    if state.active() {
                        if name == "macro" {
                            self.define_macro(statement, body, state);
                        } else {
                            self.repeat(statement, body, state);
                        }
                    }
                    at = end + 1;
                }
                "endm" | "endmacro" | "endr" | "edup" => {
                    state.error(
                        statement.span,
                        format!("`{}` closes nothing", name.to_uppercase()),
                    );
                    at += 1;
                }
                _ => {
                    self.statement(statement, state);
                    at += 1;
                }
            }
        }
    }

    // -- macros -------------------------------------------------------------

    /// `name MACRO p,q` or `MACRO name p,q`, which are the same thing.
    fn define_macro(&mut self, statement: &Statement, body: &[Statement], state: &mut State) {
        let op = statement.op.as_ref().expect("a MACRO statement");
        let (name, parameters) = match &statement.label {
            Some(label) => (label.name.to_string(), &op.args[..]),
            None => match op.args.split_first() {
                Some((first, rest)) => match first.as_ident() {
                    Some(name) => (name.to_string(), rest),
                    None => return state.error(first.span, "expected a macro name"),
                },
                None => return state.error(statement.span, "`MACRO` needs a name"),
            },
        };

        if encode::is_instruction(&name) {
            return state.error(
                statement.span,
                format!("`{name}` is an instruction, so a macro cannot be called that"),
            );
        }

        let mut names = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            match parameter.as_ident() {
                Some(name) => names.push(name.to_string()),
                None => return state.error(parameter.span, "expected a parameter name"),
            }
        }

        let key = name.to_ascii_lowercase();
        if let Some(existing) = state.macros.get(&key) {
            let previous = existing.span;
            return state.report(
                Diagnostic::error(statement.span, format!("`{name}` is already a macro"))
                    .with_related(previous, "the earlier definition is"),
            );
        }
        state.macros.insert(
            key,
            Rc::new(Macro {
                name,
                parameters: names,
                body: body.to_vec(),
                span: statement.span,
            }),
        );
    }

    fn expand(&mut self, definition: &Rc<Macro>, call: &Statement, state: &mut State) {
        let op = call.op.as_ref().expect("an invocation");
        if op.args.len() != definition.parameters.len() {
            return state.error(
                call.span,
                format!(
                    "`{}` takes {} argument{}, not {}",
                    definition.name,
                    definition.parameters.len(),
                    if definition.parameters.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    op.args.len()
                ),
            );
        }
        if state.expanding.len() >= MAX_EXPANSION_DEPTH {
            // Reported through the expansion stack, so the message names the
            // chain that got here rather than only where it stopped.
            return state.error(
                call.span,
                format!(
                    "`{}` is expanded more than {MAX_EXPANSION_DEPTH} deep",
                    definition.name
                ),
            );
        }

        let bindings: Vec<(&str, &Expr)> = definition
            .parameters
            .iter()
            .map(String::as_str)
            .zip(op.args.iter())
            .collect();
        let body: Vec<Statement> = definition
            .body
            .iter()
            .map(|statement| substitute_statement(statement, &bindings))
            .collect();

        let index = state.expansions.len();
        state.expansions.push(Expansion {
            name: definition.name.clone(),
            defined_at: definition.span,
            invoked_at: call.span,
            parent: state.expanding.last().copied(),
        });
        state.expanding.push(index);

        // Local labels inside the body hang off a name unique to this
        // expansion, so the same `.loop` written once is a different symbol
        // every time the macro is used.
        let enclosing = self.symbols.local_scope();
        self.symbols
            .set_local_scope(Some(format!("{}#{index}", definition.name)));

        self.run(&body, state);

        self.symbols.set_local_scope(enclosing);
        state.expanding.pop();
    }

    /// `REPT n` / `DUP n`, with an optional name for the iteration counter.
    fn repeat(&mut self, statement: &Statement, body: &[Statement], state: &mut State) {
        let op = statement.op.as_ref().expect("a REPT statement");
        let Some(count) = op.args.first() else {
            return state.error(statement.span, "`REPT` needs a count");
        };
        // Unresolved on the first pass means no iterations for now; defining
        // the symbol moves addresses, which asks for another pass.
        let Some(count) = self.value(count, state) else {
            return;
        };
        if count < 0 {
            return state.error(
                statement.span,
                format!("`REPT {count}` repeats a negative number of times"),
            );
        }
        if count > MAX_REPETITIONS {
            return state.error(
                statement.span,
                format!("`REPT {count}` is more than the {MAX_REPETITIONS} this assembler will do"),
            );
        }

        let counter = match op.args.get(1) {
            Some(arg) => match arg.as_ident() {
                Some(name) => Some(name.to_string()),
                None => return state.error(arg.span, "expected a name for the counter"),
            },
            None => None,
        };

        for iteration in 0..count {
            if let Some(name) = &counter {
                if let Err(e) = self
                    .symbols
                    .define_variable(name, iteration, statement.span)
                {
                    state.report(e);
                }
            }
            self.run(body, state);
            if state.finished {
                return;
            }
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
                state.report(e);
            }
        }

        let start = state.address;
        let before = state.emitted;
        let records_its_own_bytes =
            self.directive_or_instruction(statement, &name, args, span, state);
        // A macro call and an `INCLUDE` emit bytes, but through statements that
        // record themselves. Recording the enclosing statement as well would
        // give the listing overlapping entries and the debug info two answers
        // for one address.
        if records_its_own_bytes && state.emitted > before {
            state.lines.push(LineRecord {
                span,
                address: (start & 0xFFFF) as u16,
                length: (state.emitted - before) as u16,
                expansion: state.expanding.last().copied(),
            });
        }
    }

    /// Returns whether this statement is the one that should be recorded as
    /// having produced the bytes it emitted.
    fn directive_or_instruction(
        &mut self,
        statement: &Statement,
        name: &str,
        args: &[Expr],
        span: Span,
        state: &mut State,
    ) -> bool {
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
            "include" => {
                self.include(args, span, state);
                return false;
            }
            "incbin" => self.incbin(args, span, state),
            "end" => state.finished = true,
            // A macro call and an instruction are the same shape, so this is
            // where the parser's refusal to guess is finally answered.
            _ => match state.macros.get(name).cloned() {
                Some(definition) => {
                    self.expand(&definition, statement, state);
                    return false;
                }
                None => self.instruction(statement, state),
            },
        }
        true
    }

    fn instruction(&mut self, statement: &Statement, state: &mut State) {
        let op = statement.op.as_ref().expect("an operation");
        // The plan gives the length without evaluating anything, so the address
        // advances correctly even when an operand cannot be resolved yet.
        let plan = match encode::plan(op) {
            Ok(plan) => plan,
            Err(e) => {
                state.report(e);
                return;
            }
        };
        let length = plan.len() as i64;
        match encode::emit(&plan, state.site(), &mut self.symbols) {
            Ok(bytes) => state.write(&bytes),
            Err(e) => {
                if !e.is_forward_reference() {
                    state.report(e.diagnostic());
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
                    state.report(e.diagnostic());
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
                    state.report(e.diagnostic());
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
            state.report(e);
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
            state.report(diagnostic);
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
                    state.report(e.diagnostic());
                }
                None
            }
        }
    }
}

/// The directive name a statement begins with, lower-cased and without a
/// leading dot, or empty for a statement that is only a label.
fn directive_name(statement: &Statement) -> String {
    statement
        .op
        .as_ref()
        .map(|op| op.name.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_default()
}

/// The index of the statement closing the block opened at `from`, honouring
/// nesting.
fn block_end(statements: &[Statement], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (at, statement) in statements.iter().enumerate().skip(from + 1) {
        match directive_name(statement).as_str() {
            "macro" | "rept" | "dup" => depth += 1,
            "endm" | "endmacro" | "endr" | "edup" => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// Replace parameter names with the arguments given for them.
///
/// Arguments are substituted as expressions, keeping their own spans, so an
/// error about a value points at the call that supplied it while the statement
/// still points into the macro body.
fn substitute_statement(statement: &Statement, bindings: &[(&str, &Expr)]) -> Statement {
    let op = statement.op.as_ref().map(|op| crate::ast::Op {
        name: op.name.clone(),
        name_span: op.name_span,
        args: op
            .args
            .iter()
            .map(|arg| substitute(arg, bindings))
            .collect(),
        span: op.span,
    });
    Statement {
        label: statement.label.clone(),
        op,
        span: statement.span,
    }
}

fn substitute(expr: &Expr, bindings: &[(&str, &Expr)]) -> Expr {
    match &expr.kind {
        ExprKind::Ident(name) => match bindings.iter().find(|(parameter, _)| *parameter == &**name)
        {
            Some((_, argument)) => (*argument).clone(),
            None => expr.clone(),
        },
        ExprKind::Paren(inner) => Expr::new(
            ExprKind::Paren(Box::new(substitute(inner, bindings))),
            expr.span,
        ),
        ExprKind::Unary { op, operand } => Expr::new(
            ExprKind::Unary {
                op: *op,
                operand: Box::new(substitute(operand, bindings)),
            },
            expr.span,
        ),
        ExprKind::Binary { op, lhs, rhs } => Expr::new(
            ExprKind::Binary {
                op: *op,
                lhs: Box::new(substitute(lhs, bindings)),
                rhs: Box::new(substitute(rhs, bindings)),
            },
            expr.span,
        ),
        _ => expr.clone(),
    }
}
