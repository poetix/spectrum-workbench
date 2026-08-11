//! Symbol table tests, driven through a stand-in for the assembly pass.
//!
//! The interesting behaviour — forward references, convergence, what `1_B`
//! means from where it is written — only appears when something walks the
//! source assigning addresses. [`Pass`] is the smallest thing that does that:
//! it pretends every instruction is one byte, because what is under test is the
//! symbol table and not the encoder that ticket 0003 will write.

use rkw_asm::ast::Statement;
use rkw_asm::{Diagnostic, Site, SourceMap, Symbols, eval, parse};

/// One walk over the statements, assigning addresses and defining labels.
struct Pass<'a> {
    symbols: &'a mut Symbols,
    address: i64,
    section: i64,
    seq: u64,
    errors: Vec<Diagnostic>,
    /// Every operand value this pass worked out, in source order.
    values: Vec<i64>,
}

impl<'a> Pass<'a> {
    fn run(symbols: &'a mut Symbols, statements: &[Statement]) -> Self {
        let mut pass = Pass {
            symbols,
            address: 0,
            section: 0,
            seq: 0,
            errors: Vec::new(),
            values: Vec::new(),
        };
        for statement in statements {
            pass.statement(statement);
        }
        pass
    }

    fn statement(&mut self, statement: &Statement) {
        self.seq += 1;
        let site = Site::new(self.address, self.section, self.seq);
        let name = statement
            .op
            .as_ref()
            .map(|op| op.name.to_ascii_lowercase())
            .unwrap_or_default();
        let args = statement.op.as_ref().map(|op| &op.args[..]).unwrap_or(&[]);

        // A constant takes the name on its left; everything else labels an
        // address.
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
            "module" => self
                .symbols
                .enter_module(args[0].as_ident().expect("a name")),
            "endmodule" => self.symbols.leave_module(),
            "org" => {
                self.address = self.value(&args[0], site).unwrap_or(self.address);
                self.section = self.address;
            }
            "ds" => self.address += self.value(&args[0], site).unwrap_or(0),
            "db" => {
                self.eval_all(args, site);
                self.address += args.len() as i64;
            }
            "dw" => {
                self.eval_all(args, site);
                self.address += 2 * args.len() as i64;
            }
            // Every instruction is one byte here; only the addresses matter.
            _ => {
                self.eval_all(args, site);
                self.address += 1;
            }
        }
    }

    fn eval_all(&mut self, args: &[rkw_asm::Expr], site: Site) {
        for arg in args.iter().filter(|a| !is_register(a)) {
            self.value(arg, site);
        }
    }

    fn value(&mut self, expr: &rkw_asm::Expr, site: Site) -> Option<i64> {
        match eval(expr, site, self.symbols) {
            Ok(v) => {
                self.values.push(v);
                Some(v)
            }
            Err(e) => {
                // A forward reference is not a complaint, it is a request for
                // another pass. Only what survives to the last pass is an error.
                if !e.is_forward_reference() {
                    self.errors.push(e.diagnostic());
                }
                None
            }
        }
    }
}

/// A bare register or condition name, which is not a symbol reference.
///
/// The real assembler decides this from the mnemonic — `LD A,B` has two
/// register operands, `LD A,(B)` does not exist, and `CALL C,nn` and `CALL nn`
/// differ only in whether the first operand is a condition. Ticket 0003 does
/// that properly; here a name that is spelled like a register is simply not
/// looked up.
fn is_register(expr: &rkw_asm::Expr) -> bool {
    const NAMES: [&str; 24] = [
        "a", "b", "c", "d", "e", "h", "l", "i", "r", "af", "bc", "de", "hl", "sp", "ix", "iy",
        "nz", "z", "nc", "po", "pe", "p", "m", "af'",
    ];
    expr.as_ident()
        .is_some_and(|name| NAMES.iter().any(|r| r.eq_ignore_ascii_case(name)))
}

struct Outcome {
    map: SourceMap,
    symbols: Symbols,
    errors: Vec<Diagnostic>,
    values: Vec<i64>,
    passes: u32,
}

impl Outcome {
    fn names(&mut self) -> Vec<(String, i64)> {
        self.symbols.iter_values()
    }

    fn messages(&self) -> Vec<String> {
        self.errors.iter().map(|d| d.message.clone()).collect()
    }

    fn expect_clean(&self) -> &Self {
        assert!(
            self.errors.is_empty(),
            "unexpected errors:\n{}",
            self.map.render_all(&self.errors)
        );
        self
    }
}

/// Assemble to a fixpoint: keep making passes until nothing moves, or give up.
fn assemble(src: &str) -> Outcome {
    const LIMIT: u32 = 8;

    let mut map = SourceMap::new();
    let file = map.add("t.asm", src);
    let parsed = parse(&map, file);
    assert!(
        parsed.diagnostics.is_empty(),
        "source did not parse:\n{}",
        map.render_all(&parsed.diagnostics)
    );

    let mut symbols = Symbols::new();
    loop {
        let pass = Pass::run(&mut symbols, &parsed.program.statements);
        let (errors, values) = (pass.errors, pass.values);
        // Two passes minimum: the first is what makes forward references
        // knowable, so nothing can be believed until a second agrees with it.
        if (symbols.converged() && symbols.pass() >= 2) || symbols.pass() >= LIMIT {
            return Outcome {
                map,
                passes: symbols.pass(),
                symbols,
                errors,
                values,
            };
        }
        symbols.begin_pass();
    }
}

#[test]
fn forward_references_resolve_on_the_second_pass() {
    let mut out = assemble("    jp later\nlater:\n    ret\n");
    out.expect_clean();
    assert_eq!(out.names(), [("later".to_string(), 1)]);
    assert_eq!(out.passes, 2, "nothing moved, so two passes is enough");
}

#[test]
fn an_undefined_symbol_is_not_a_forward_reference() {
    // Both look identical on the first pass. By the second, everything that is
    // ever going to be defined has been.
    let out = assemble("    jp missing\n");
    assert_eq!(out.messages(), ["undefined symbol `missing`"]);
}

#[test]
fn constants_may_be_defined_after_use() {
    let out = assemble("    ld a,size\nsize equ 40\n");
    out.expect_clean();
    assert_eq!(out.values, [40]);
}

#[test]
fn circular_definitions_are_reported_rather_than_looping() {
    let out = assemble("alpha equ beta\nbeta equ alpha\n    dw alpha\n");
    assert_eq!(out.messages(), ["`alpha` is defined in terms of itself"]);
}

#[test]
fn a_symbol_defined_twice_on_one_pass_is_an_error() {
    let out = assemble("main:\n    nop\nmain:\n");
    assert_eq!(out.messages(), ["`main` is already defined"]);
    // The error points at both definitions.
    assert_eq!(out.errors[0].related.len(), 1);

    let out = assemble("x equ 1\nx equ 2\n");
    assert_eq!(out.messages(), ["`x` is already defined"]);
}

#[test]
fn a_variable_may_be_redefined() {
    // `=` and DEFL exist precisely so that a macro can count.
    let out = assemble("n = 1\nn = 2\n    dw n\n");
    out.expect_clean();
    assert_eq!(out.values, [2]);
}

#[test]
fn local_labels_belong_to_the_last_global_label() {
    let mut out =
        assemble("main:\n    ld b,4\n.loop:\n    djnz .loop\n    ret\nother:\n.loop:\n    ret\n");
    out.expect_clean();
    assert_eq!(
        out.names(),
        [
            ("main".to_string(), 0),
            ("main.loop".to_string(), 1),
            ("other".to_string(), 3),
            ("other.loop".to_string(), 3),
        ]
    );
    // `.loop` under `main` is a different symbol from `.loop` under `other`,
    // and the jump found the near one.
    assert_eq!(out.values, [4, 1]);
}

#[test]
fn module_names_qualify_and_resolve_outwards() {
    let mut out = assemble(
        "    module video\nclear:\n    call clear\n    endmodule\nstart:\n    call video.clear\n",
    );
    out.expect_clean();
    assert_eq!(
        out.names(),
        [("start".to_string(), 1), ("video.clear".to_string(), 0),]
    );
    // Unqualified inside the module, qualified outside it, same symbol.
    assert_eq!(out.values, [0, 0]);
}

#[test]
fn an_at_prefixed_label_escapes_the_module() {
    let mut out = assemble("    module video\n@rom_cls:\n    ret\n    endmodule\n");
    out.expect_clean();
    assert_eq!(out.names(), [("rom_cls".to_string(), 0)]);
}

#[test]
fn temporary_labels_resolve_by_position() {
    let out = assemble("1:\n    djnz 1_B\n    jr 1_F\n1:\n    ret\n");
    out.expect_clean();
    // Backwards to the one above, forwards to the one below — the same name
    // meaning two different addresses.
    assert_eq!(out.values, [0, 2]);
}

#[test]
fn a_program_whose_addresses_move_takes_another_pass() {
    // `ds size` cannot be sized on the first pass, so everything after it
    // moves once `size` is known. This is the case the fixpoint exists for.
    let mut out = assemble("    ds size\nend:\nsize equ 4\n");
    out.expect_clean();
    assert_eq!(
        out.names(),
        [("end".to_string(), 4), ("size".to_string(), 4)]
    );
    assert_eq!(out.passes, 3, "one pass to place it, one to agree");
}

#[test]
fn org_sets_both_the_address_and_the_section_start() {
    let mut out = assemble("    org $8000\nstart:\n    dw $-$$\n");
    out.expect_clean();
    assert_eq!(out.names(), [("start".to_string(), 0x8000)]);
    assert_eq!(out.values, [0x8000, 0]);
}

#[test]
fn exist_asks_whether_a_name_is_defined_without_evaluating_it() {
    let out = assemble("defined equ 1\n    dw exist defined\n    dw exist absent\n");
    out.expect_clean();
    assert_eq!(out.values, [1, 0]);
}
