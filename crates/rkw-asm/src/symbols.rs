//! The symbol table.
//!
//! Two kinds of symbol, because they resolve differently.
//!
//! A **label** has whatever address the location counter had reached when the
//! pass walked past it. Its value cannot be known before then, so labels are
//! (re)defined on every pass and a reference to one that the pass has not yet
//! reached is [`EvalError::NotYetDefined`] rather than an error.
//!
//! A **constant** (`EQU`, or `=` for the redefinable form) is an expression,
//! and is evaluated the first time something asks for it. That is what makes
//! `a equ b` / `b equ a` reportable: the resolution is a walk over a graph, so
//! a cycle is a node visited while it is already being visited, rather than a
//! value that simply never settles.
//!
//! Naming follows sjasmplus. A label beginning `.` belongs to the last
//! non-local label — `.loop` under `main:` is `main.loop`. Inside a `MODULE`,
//! names are qualified with the module path, and a reference resolves by trying
//! the innermost module first and working outwards, so a module can refer to
//! its own names without qualifying them. A label beginning `@` is used exactly
//! as written and escapes both of those.
//!
//! Numeric temporary labels (`1:`, referred to as `1_F` and `1_B`) are ordered
//! rather than named: what `1_B` means depends on where it is asked from. They
//! are kept as a list per number, and a backward reference reads the list being
//! built by this pass while a forward reference reads the one left by the
//! previous pass — which is the only place the two-pass structure is visible in
//! this module's interface.

use std::collections::HashMap;

use crate::ast::{Expr, Label, LabelKind};
use crate::diag::Diagnostic;
use crate::eval::{EvalError, Site, eval};
use crate::source::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// An address, assigned by the pass that walks the source.
    Label,
    /// `EQU`: an expression, and an error to define twice.
    Const,
    /// `=` and `DEFL`: a constant that may be redefined as the pass proceeds.
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Binding {
    Label(i64),
    Const {
        expr: Expr,
        site: Site,
        state: ConstState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConstState {
    /// Not evaluated on this pass yet.
    Pending,
    /// Being evaluated: seeing this again is a cycle.
    InProgress,
    Done(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Symbol {
    binding: Binding,
    kind: SymbolKind,
    /// Where it was defined, for "already defined at ..." on a duplicate.
    span: Span,
    /// The pass that last defined it. Redefinition is normal across passes and
    /// an error within one.
    defined_on: u32,
    /// The value it had on the previous pass, to detect a program whose
    /// addresses are still moving.
    previous: Option<i64>,
    /// The value it has taken on this pass, if it has taken one.
    ///
    /// Kept separately from the binding because a constant's binding is reset
    /// to unevaluated when the pass reaches its `EQU` again, which would
    /// otherwise erase the value the same pass had already computed from it and
    /// make every pass look like a change.
    latest: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Symbols {
    names: HashMap<String, Symbol>,
    /// Temporary labels being collected on this pass, per number, in source
    /// order.
    temps: HashMap<u32, Vec<(u64, i64)>>,
    /// The same from the previous pass, which is where forward references have
    /// to look.
    temps_previous: HashMap<u32, Vec<(u64, i64)>>,
    modules: Vec<String>,
    last_global: Option<String>,
    pass: u32,
    changed: bool,
}

impl Symbols {
    pub fn new() -> Self {
        Self {
            pass: 1,
            ..Self::default()
        }
    }

    pub fn pass(&self) -> u32 {
        self.pass
    }

    /// True if nothing moved during the pass just finished, so another pass
    /// would produce identical output.
    pub fn converged(&self) -> bool {
        !self.changed
    }

    /// Start the next pass over the source. Definitions are kept — that is what
    /// makes forward references resolvable second time round — but constants go
    /// back to being unevaluated, since the labels they are computed from may
    /// have moved.
    pub fn begin_pass(&mut self) {
        self.pass += 1;
        self.changed = false;
        self.modules.clear();
        self.last_global = None;
        self.temps_previous = std::mem::take(&mut self.temps);
        for symbol in self.names.values_mut() {
            symbol.previous = symbol.latest.take();
            if let Binding::Const { state, .. } = &mut symbol.binding {
                *state = ConstState::Pending;
            }
        }
    }

    pub fn enter_module(&mut self, name: &str) {
        self.modules.push(name.to_string());
    }

    pub fn leave_module(&mut self) {
        self.modules.pop();
    }

    /// The module path in force, empty at the top level.
    pub fn module_path(&self) -> String {
        self.modules.join(".")
    }

    /// The name a definition written here would be filed under.
    pub fn qualify(&self, name: &str) -> String {
        if let Some(bare) = name.strip_prefix('@') {
            return bare.to_string();
        }
        if name.starts_with('.') {
            return match &self.last_global {
                Some(parent) => format!("{parent}{name}"),
                // A local label with no global label above it: keep it as
                // written, so the error names what the source called it.
                None => name.to_string(),
            };
        }
        match self.modules.is_empty() {
            true => name.to_string(),
            false => format!("{}.{}", self.module_path(), name),
        }
    }

    /// Define a label at `address`. `seq` is the statement's ordinal, which
    /// only matters for the numeric temporary labels.
    pub fn define_label(
        &mut self,
        label: &Label,
        address: i64,
        seq: u64,
    ) -> Result<(), Diagnostic> {
        if let LabelKind::Temp(id) = label.kind {
            self.define_temp(id, seq, address);
            return Ok(());
        }
        let key = self.qualify(&label.name);
        if label.kind != LabelKind::Local {
            self.last_global = Some(key.clone());
        }
        self.define(key, Binding::Label(address), SymbolKind::Label, label.span)
    }

    /// Define a constant. `redefinable` distinguishes `=` and `DEFL`, which may
    /// be assigned again later in the same pass, from `EQU`, which may not.
    pub fn define_const(
        &mut self,
        name: &str,
        expr: Expr,
        site: Site,
        span: Span,
        redefinable: bool,
    ) -> Result<(), Diagnostic> {
        let key = self.qualify(name);
        // A constant is a global name like any other, so locals written after
        // it belong to it: `.size` under `screen equ $4000` is `screen.size`.
        if !name.starts_with('.') {
            self.last_global = Some(key.clone());
        }
        let kind = if redefinable {
            SymbolKind::Variable
        } else {
            SymbolKind::Const
        };
        let binding = Binding::Const {
            expr,
            site,
            state: ConstState::Pending,
        };
        self.define(key, binding, kind, span)
    }

    fn define(
        &mut self,
        key: String,
        binding: Binding,
        kind: SymbolKind,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match self.names.get_mut(&key) {
            Some(existing) => {
                // Every pass walks the whole source, so seeing a definition
                // again is only a duplicate if it happens twice on one pass.
                if existing.defined_on == self.pass && kind != SymbolKind::Variable {
                    return Err(
                        Diagnostic::error(span, format!("`{key}` is already defined"))
                            .with_related(existing.span, "the earlier definition is"),
                    );
                }
                // Only labels are compared here: a constant has no value until
                // something asks for it, so whether it moved is decided in
                // `lookup` instead.
                if let Binding::Label(address) = binding {
                    if existing.previous != Some(address) {
                        self.changed = true;
                    }
                    existing.latest = Some(address);
                }
                existing.binding = binding;
                existing.kind = kind;
                existing.defined_on = self.pass;
                existing.span = span;
            }
            None => {
                // A name that did not exist last pass is a change by itself:
                // something that referred to it saw a forward reference.
                self.changed = true;
                let latest = match binding {
                    Binding::Label(address) => Some(address),
                    Binding::Const { .. } => None,
                };
                self.names.insert(
                    key,
                    Symbol {
                        binding,
                        kind,
                        span,
                        defined_on: self.pass,
                        previous: None,
                        latest,
                    },
                );
            }
        }
        Ok(())
    }

    fn define_temp(&mut self, id: u32, seq: u64, value: i64) {
        let entries = self.temps.entry(id).or_default();
        let index = entries.len();
        let previous = self
            .temps_previous
            .get(&id)
            .and_then(|prev| prev.get(index))
            .map(|&(_, v)| v);
        if previous != Some(value) {
            self.changed = true;
        }
        entries.push((seq, value));
    }

    /// True if `name` has a definition, whether or not it can be evaluated.
    /// This is what the `exist` operator asks.
    pub fn is_defined(&self, name: &str) -> bool {
        self.resolve_key(name).is_some()
    }

    pub fn kind_of(&self, name: &str) -> Option<SymbolKind> {
        let key = self.resolve_key(name)?;
        Some(self.names[&key].kind)
    }

    /// The value of `name`, evaluating its defining expression if this is the
    /// first time it has been asked for on this pass.
    pub fn lookup(&mut self, name: &str, span: Span) -> Result<i64, EvalError> {
        let Some(key) = self.resolve_key(name) else {
            return Err(self.unknown(name, span));
        };
        let (expr, const_site) = match &mut self.names.get_mut(&key).expect("just resolved").binding
        {
            Binding::Label(value) => return Ok(*value),
            Binding::Const {
                state: ConstState::Done(value),
                ..
            } => return Ok(*value),
            Binding::Const {
                state: ConstState::InProgress,
                ..
            } => return Err(EvalError::Circular { name: key, span }),
            Binding::Const { expr, site, state } => {
                *state = ConstState::InProgress;
                // Cloned rather than borrowed: evaluating it needs the table
                // this expression lives in.
                (expr.clone(), *site)
            }
        };

        let result = eval(&expr, const_site, self);
        let symbol = self.names.get_mut(&key).expect("still present");
        match result {
            Ok(value) => {
                if let Binding::Const { state, .. } = &mut symbol.binding {
                    *state = ConstState::Done(value);
                }
                symbol.latest = Some(value);
                if symbol.previous != Some(value) {
                    self.changed = true;
                }
                Ok(value)
            }
            Err(e) => {
                // Not left InProgress: the next attempt should see the real
                // problem again rather than a spurious cycle.
                if let Binding::Const { state, .. } = &mut symbol.binding {
                    *state = ConstState::Pending;
                }
                Err(e)
            }
        }
    }

    /// The nearest temporary label with this number, in the given direction.
    pub fn lookup_temp(
        &self,
        id: u32,
        forward: bool,
        seq: u64,
        span: Span,
    ) -> Result<i64, EvalError> {
        let name = format!("{id}_{}", if forward { 'F' } else { 'B' });
        let found = if forward {
            // Forward: this pass has not reached the definition, so use where
            // the previous pass found it.
            self.temps_previous
                .get(&id)
                .and_then(|entries| entries.iter().find(|&&(at, _)| at > seq))
        } else {
            // Backward: this pass has already passed it, and its value is
            // fresher than the previous pass's.
            self.temps
                .get(&id)
                .and_then(|entries| entries.iter().rev().find(|&&(at, _)| at <= seq))
        };
        match found {
            Some(&(_, value)) => Ok(value),
            None if forward && self.pass == 1 => Err(EvalError::NotYetDefined { name, span }),
            None => Err(EvalError::Undefined { name, span }),
        }
    }

    /// Which stored name a reference means, trying the innermost module first.
    fn resolve_key(&self, name: &str) -> Option<String> {
        let key = self.qualify(name);
        if self.names.contains_key(&key) {
            return Some(key);
        }
        // `@name` and `.local` name exactly one thing, so there is nothing
        // further to try for them.
        if name.starts_with('@') || name.starts_with('.') {
            return None;
        }
        for depth in (0..self.modules.len()).rev() {
            let candidate = match depth {
                0 => name.to_string(),
                _ => format!("{}.{}", self.modules[..depth].join("."), name),
            };
            if self.names.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Whether an unknown name is a forward reference or a mistake. On the
    /// first pass nothing below the current line has been seen yet; by the
    /// second, everything has.
    fn unknown(&self, name: &str, span: Span) -> EvalError {
        let name = name.to_string();
        if self.pass == 1 {
            EvalError::NotYetDefined { name, span }
        } else {
            EvalError::Undefined { name, span }
        }
    }

    /// Every defined name and its value, for the listing and the debugger's
    /// symbol map. Constants that were never referenced are evaluated here.
    pub fn iter_values(&mut self) -> Vec<(String, i64)> {
        let keys: Vec<String> = self.names.keys().cloned().collect();
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let span = self.names[&key].span;
            if let Ok(value) = self.lookup(&key, span) {
                out.push((key, value));
            }
        }
        out.sort();
        out
    }
}
