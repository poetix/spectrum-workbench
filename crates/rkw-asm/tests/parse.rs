//! Parser tests: statement shapes, label forms, expression precedence, and
//! what happens to the rest of the file when one line is wrong.

use rkw_asm::{BinOp, Expr, ExprKind, LabelKind, Op, Program, SourceMap, Statement, UnOp, parse};

fn program(src: &str) -> Program {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", src);
    let parsed = parse(&map, file);
    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics:\n{}",
        map.render_all(&parsed.diagnostics)
    );
    parsed.program
}

fn statement(src: &str) -> Statement {
    let mut statements = program(src).statements;
    assert_eq!(statements.len(), 1, "expected one statement in {src:?}");
    statements.remove(0)
}

fn op(src: &str) -> Op {
    statement(src).op.expect("statement has an operation")
}

/// The single operand of a one-operand statement, for testing expressions.
fn operand(src: &str) -> Expr {
    let mut args = op(&format!("    dw {src}")).args;
    assert_eq!(args.len(), 1);
    args.remove(0)
}

fn errors(src: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", src);
    parse(&map, file)
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn instruction_with_operands() {
    let op = op("    ld a,(hl)");
    assert_eq!(&*op.name, "ld");
    assert_eq!(op.args.len(), 2);
    assert_eq!(op.args[0].as_ident(), Some("a"));
    // The parser does not decide that this is a memory operand — it records
    // that it was written in parentheses and leaves the rest to 0003.
    assert_eq!(
        op.args[1].as_parenthesised().and_then(Expr::as_ident),
        Some("hl")
    );
}

#[test]
fn instruction_without_operands() {
    let op = op("    nop");
    assert_eq!(&*op.name, "nop");
    assert!(op.args.is_empty());
}

#[test]
fn directives_and_macro_calls_are_the_same_shape() {
    // Nothing here distinguishes them, which is the point: a macro named
    // `plot` and a directive named `org` are both a name and operands until
    // something knows which names exist.
    assert_eq!(&*op("    org $8000").name, "org");
    assert_eq!(&*op("    include \"lib.asm\"").name, "include");
    assert_eq!(op("    plot 12,34").args.len(), 2);

    let stmt = statement("border  equ 40");
    assert_eq!(stmt.label.as_ref().map(|l| &*l.name), Some("border"));
    assert_eq!(stmt.op.as_ref().map(|o| &*o.name), Some("equ"));
}

#[test]
fn a_symbol_definition_makes_the_word_before_it_a_label() {
    // `SIZE` is also a directive name, so the column-1 rule alone would read
    // this as the directive applied to `equ`.
    let stmt = statement("size    equ 40");
    assert_eq!(stmt.label.as_ref().map(|l| &*l.name), Some("size"));
    assert_eq!(stmt.op.as_ref().map(|o| &*o.name), Some("equ"));

    // `=` is the one operation not spelled as a word.
    let stmt = statement("count = 3");
    assert_eq!(stmt.label.as_ref().map(|l| &*l.name), Some("count"));
    let op = stmt.op.expect("an operation");
    assert_eq!(&*op.name, "=");
    assert_eq!(op.args.len(), 1);
}

#[test]
fn label_forms() {
    let global = statement("main:").label.expect("label");
    assert_eq!(global.kind, LabelKind::Global);
    assert_eq!(&*global.name, "main");

    // In column 1 the colon is optional, which is why a word there has to be
    // checked against the mnemonic and directive names.
    let bare = statement("main").label.expect("label");
    assert_eq!(bare.kind, LabelKind::Global);

    let local = statement(".loop:").label.expect("label");
    assert_eq!(local.kind, LabelKind::Local);
    assert_eq!(&*local.name, ".loop");
    assert_eq!(statement(".loop").label.unwrap().kind, LabelKind::Local);

    let temp = statement("1:").label.expect("label");
    assert_eq!(temp.kind, LabelKind::Temp(1));

    let verbatim = statement("@absolute:").label.expect("label");
    assert!(verbatim.is_verbatim());
}

#[test]
fn a_mnemonic_in_column_one_is_not_a_label() {
    let stmt = statement("ld a,1");
    assert!(stmt.label.is_none());
    assert_eq!(stmt.op.map(|o| o.name.to_string()), Some("ld".into()));

    // Nor is a dot-prefixed directive, which is how sjasmplus lets `.db` and
    // `.loop` both start a line.
    let stmt = statement(".db 1");
    assert!(stmt.label.is_none());

    // But with a colon it is a label whatever it is called.
    let stmt = statement("ld: nop");
    assert_eq!(stmt.label.map(|l| l.name.to_string()), Some("ld".into()));
}

/// The dot form belongs to the directives: sjasmplus has `.db` but no `.ld`,
/// and real sources name local labels after the instruction under test. Ticket
/// 0030 — reading `.scf` as `SCF` lost the label and misparsed the line.
#[test]
fn a_dot_prefixed_mnemonic_in_column_one_is_a_local_label() {
    let stmt = statement(".scf    db 1");
    assert_eq!(stmt.label.map(|l| l.name.to_string()), Some(".scf".into()));
    assert_eq!(stmt.op.map(|o| o.name.to_string()), Some("db".into()));

    // A macro call after such a label is the case that made this visible: the
    // parser cannot know `flags` is a macro, so the label has to be right.
    let stmt = statement(".ccf    flags 1,2");
    assert_eq!(stmt.label.map(|l| l.name.to_string()), Some(".ccf".into()));
    assert_eq!(stmt.op.map(|o| o.args.len()), Some(2));
}

#[test]
fn label_and_instruction_on_one_line() {
    let stmt = statement("start:  ld a,1");
    assert_eq!(stmt.label.map(|l| l.name.to_string()), Some("start".into()));
    assert_eq!(stmt.op.map(|o| o.args.len()), Some(2));
}

#[test]
fn colon_separates_statements_on_one_line() {
    let statements = program("    ld a,1 : ld b,2 : ret\n").statements;
    assert_eq!(statements.len(), 3);
    assert_eq!(statements[2].to_string(), "ret");
}

#[test]
fn blank_and_comment_only_lines_produce_nothing() {
    assert!(program("\n\n; just a comment\n   \n").statements.is_empty());
}

#[test]
fn anonymous_labels_and_their_references() {
    let statements = program("1:\n    djnz 1_B\n    jr 2_F\n2:\n").statements;
    assert_eq!(
        statements[0].label.as_ref().unwrap().kind,
        LabelKind::Temp(1)
    );
    assert_eq!(
        statements[1].op.as_ref().unwrap().args[0].kind,
        ExprKind::TempRef {
            id: 1,
            forward: false
        }
    );
    assert_eq!(
        statements[2].op.as_ref().unwrap().args[0].kind,
        ExprKind::TempRef {
            id: 2,
            forward: true
        }
    );
    assert_eq!(
        statements[3].label.as_ref().unwrap().kind,
        LabelKind::Temp(2)
    );
}

/// The operator at the root of the tree, which is the one that binds loosest.
fn root_op(src: &str) -> BinOp {
    match operand(src).kind {
        ExprKind::Binary { op, .. } => op,
        other => panic!("{src:?} is not a binary expression: {other:?}"),
    }
}

#[test]
fn precedence_follows_the_documented_table() {
    assert_eq!(root_op("1+2*3"), BinOp::Add);
    assert_eq!(root_op("1*2+3"), BinOp::Add);
    assert_eq!(root_op("1|2&3"), BinOp::BitOr);
    assert_eq!(root_op("1&&2||3"), BinOp::OrOr);
    assert_eq!(root_op("1<<2+3"), BinOp::Shl);
    // Comparison binds tighter than the bitwise operators, so `&` is the root.
    assert_eq!(root_op("1=2&3"), BinOp::BitAnd);
    assert_eq!(root_op("1<?2<3"), BinOp::Lt);
    // Word spellings sit at the same level as their symbols, and keep their
    // own spelling in the tree.
    assert_eq!(root_op("1 or 2 and 3"), BinOp::OrWord);
    assert_eq!(root_op("1 shl 2 + 3"), BinOp::ShlWord);
}

#[test]
fn operators_associate_to_the_left() {
    let ExprKind::Binary { lhs, .. } = operand("1-2-3").kind else {
        panic!("not binary");
    };
    assert!(matches!(lhs.kind, ExprKind::Binary { op: BinOp::Sub, .. }));
}

#[test]
fn unary_operators() {
    assert!(matches!(
        operand("-1").kind,
        ExprKind::Unary { op: UnOp::Neg, .. }
    ));
    assert!(matches!(
        operand("~$FF").kind,
        ExprKind::Unary {
            op: UnOp::BitNot,
            ..
        }
    ));
    assert!(matches!(
        operand("high label").kind,
        ExprKind::Unary { op: UnOp::High, .. }
    ));
    // Unary binds tighter than any infix operator.
    assert_eq!(root_op("-1*2"), BinOp::Mul);
}

#[test]
fn parentheses_are_kept_rather_than_folded_away() {
    let e = operand("(1+2)*3");
    assert_eq!(e.to_string(), "(1+2)*3");
    assert!(operand("(label)").as_parenthesised().is_some());
}

#[test]
fn character_literals_are_available_as_numbers() {
    assert_eq!(operand("'a'").as_char_value(), Some(97));
    assert_eq!(operand("\"ab\"").as_char_value(), None);
    assert_eq!(root_op("'a'+1"), BinOp::Add);
}

#[test]
fn spans_cover_what_they_name() {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", "start:  ld a,(ix+5)\n");
    let parsed = parse(&map, file);
    let stmt = &parsed.program.statements[0];

    let label = stmt.label.as_ref().unwrap();
    assert_eq!(map.snippet(label.span), "start:");
    assert_eq!(map.location(label.span).to_string(), "t.asm:1:1");

    let op = stmt.op.as_ref().unwrap();
    assert_eq!(map.snippet(op.name_span), "ld");
    assert_eq!(map.snippet(op.args[1].span), "(ix+5)");
    assert_eq!(map.location(op.args[1].span).to_string(), "t.asm:1:14");
    assert_eq!(map.snippet(stmt.span), "start:  ld a,(ix+5)");
}

#[test]
fn an_error_reports_a_position_and_does_not_cascade() {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", "    ld a,(hl\n    ret\n    nop\n");
    let parsed = parse(&map, file);

    assert_eq!(parsed.diagnostics.len(), 1);
    let d = &parsed.diagnostics[0];
    assert_eq!(map.location(d.span).to_string(), "t.asm:1:13");
    assert_eq!(
        map.render(d),
        concat!(
            "error: expected `)`\n",
            " --> t.asm:1:13\n",
            "  |\n",
            "1 |     ld a,(hl\n",
            "  |             ^ found end of line\n",
            "  = note: unclosed `(` at t.asm:1:10\n",
        )
    );

    // Recovery stops at the end of the bad line, so the next two still parse.
    assert_eq!(parsed.program.statements.len(), 2);
    assert_eq!(parsed.program.statements[0].to_string(), "ret");
}

#[test]
fn several_independent_errors_are_all_reported() {
    assert_eq!(
        errors("    ld a,(hl\n    ld b,\n    ld c,)\n"),
        ["expected `)`", "expected an operand", "expected an operand",]
    );
}

#[test]
fn junk_after_the_operands_is_reported_where_it_starts() {
    assert_eq!(
        errors("    ld a,1 2\n"),
        ["expected `,` or end of statement"]
    );
}

#[test]
fn a_statement_prints_back_as_it_was_written() {
    // Not a canonical form: the spelling of every literal and operator is
    // preserved, which is what the listing and the round-trip test rely on.
    for line in [
        "ld a,$FF",
        "ld a,%1010",
        "ld hl,(1+2)*3",
        "djnz 1_B",
        "db \"hi\"z,'a'",
        "ld a,high label",
        "defb 1 or 2",
        "ex af,af'",
    ] {
        assert_eq!(op(&format!("    {line}")).to_string(), line);
    }
    assert_eq!(statement("main:   ld a,1").to_string(), "main: ld a,1");
}
