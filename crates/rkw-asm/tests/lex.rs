//! Lexer tests.
//!
//! Most of these pin down sjasmplus's numeric literal forms, of which there are
//! more than one might expect, and the three places where a character has two
//! meanings: `%` (binary prefix or modulo), `b` (binary suffix or backward
//! temporary label) and `'` (string, digit separator, or shadow register).

use rkw_asm::{Quote, SourceMap, StrSuffix, Sym, TokenKind, lex};

fn kinds(src: &str) -> Vec<TokenKind> {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", src);
    let lexed = lex(&map, file);
    assert!(
        lexed.diagnostics.is_empty(),
        "unexpected diagnostics:\n{}",
        map.render_all(&lexed.diagnostics)
    );
    lexed.tokens.into_iter().map(|t| t.kind).collect()
}

/// The value of a source text that is expected to be one numeric literal.
fn number(src: &str) -> i64 {
    match &kinds(src)[..] {
        [
            TokenKind::Number { value, .. },
            TokenKind::Newline | TokenKind::Eof,
            ..,
        ] => *value,
        other => panic!("{src:?} did not lex as one number: {other:?}"),
    }
}

fn bytes(src: &str) -> Vec<u8> {
    match &kinds(src)[..] {
        [TokenKind::Str(s), ..] => s.value.clone(),
        other => panic!("{src:?} did not lex as one string: {other:?}"),
    }
}

fn diagnostics(src: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", src);
    let lexed = lex(&map, file);
    lexed
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn every_documented_numeric_form() {
    // Decimal.
    assert_eq!(number("12"), 12);
    assert_eq!(number("12d"), 12);
    // Hexadecimal, all four spellings.
    assert_eq!(number("0xc"), 12);
    assert_eq!(number("$c"), 12);
    assert_eq!(number("#c"), 12);
    assert_eq!(number("0ch"), 12);
    assert_eq!(number("$FFFF"), 0xFFFF);
    assert_eq!(number("0FFh"), 0xFF);
    // Binary, all three.
    assert_eq!(number("0b1100"), 12);
    assert_eq!(number("%1100"), 12);
    assert_eq!(number("1100b"), 12);
    // Octal, all three.
    assert_eq!(number("0q14"), 12);
    assert_eq!(number("14q"), 12);
    assert_eq!(number("14o"), 12);
}

/// A prefix beats a suffix that is also a hex digit. Half the bytes in a hex
/// table end in `d`, and reading `0xbd` as decimal `0x` + `bd` made every one
/// of them an error (ticket 0030).
#[test]
fn a_hex_prefix_survives_a_trailing_d() {
    assert_eq!(number("0xd"), 0xD);
    assert_eq!(number("0xbd"), 0xBD);
    assert_eq!(number("0xBD"), 0xBD);
    assert_eq!(number("0x12ad"), 0x12AD);
    // Without the prefix a trailing `d` still means decimal, and `0d` is zero
    // in decimal rather than a hexadecimal literal with no digits.
    assert_eq!(number("12d"), 12);
    assert_eq!(number("0d"), 0);
}

#[test]
fn digit_separators_are_ignored() {
    assert_eq!(number("12'345"), 12345);
    assert_eq!(number("1_3_7q"), 0o137);
    assert_eq!(number("$FF_FF"), 0xFFFF);
}

#[test]
fn the_literal_keeps_its_original_spelling() {
    // The listing and the round-trip test both need what was written, not a
    // canonical form of the value.
    let TokenKind::Number { text, value } = kinds("$0A").remove(0) else {
        panic!("not a number");
    };
    assert_eq!(&*text, "$0A");
    assert_eq!(value, 10);
}

#[test]
fn negative_literals_are_an_operator_and_a_literal() {
    // Sign is not part of the literal: `-` has to stay an operator, or
    // `2-1` would lex as two numbers.
    assert!(matches!(
        &kinds("-5")[..],
        [
            TokenKind::Sym(Sym::Minus),
            TokenKind::Number { value: 5, .. },
            ..
        ]
    ));
    assert!(matches!(
        &kinds("2-1")[..],
        [
            TokenKind::Number { value: 2, .. },
            TokenKind::Sym(Sym::Minus),
            TokenKind::Number { value: 1, .. },
            ..
        ]
    ));
}

#[test]
fn percent_is_a_binary_prefix_only_where_a_value_can_start() {
    assert!(matches!(
        &kinds("ld a,%1010")[..],
        [
            TokenKind::Ident(_),
            TokenKind::Ident(_),
            TokenKind::Sym(Sym::Comma),
            TokenKind::Number { value: 10, .. },
            ..
        ]
    ));
    // After a value it is modulo, and after `)` it is modulo too.
    assert!(matches!(
        &kinds("5%10")[..],
        [
            TokenKind::Number { value: 5, .. },
            TokenKind::Sym(Sym::Percent),
            TokenKind::Number { value: 10, .. },
            ..
        ]
    ));
    assert!(matches!(
        &kinds("(x)%2")[..],
        [
            TokenKind::Sym(Sym::LParen),
            TokenKind::Ident(_),
            TokenKind::Sym(Sym::RParen),
            TokenKind::Sym(Sym::Percent),
            ..
        ]
    ));
}

#[test]
fn temporary_label_references() {
    assert_eq!(
        kinds("1_F").remove(0),
        TokenKind::TempRef {
            id: 1,
            forward: true
        }
    );
    assert_eq!(
        kinds("2_B").remove(0),
        TokenKind::TempRef {
            id: 2,
            forward: false
        }
    );
    // The short forms too, where they are not ambiguous.
    assert_eq!(
        kinds("3f").remove(0),
        TokenKind::TempRef {
            id: 3,
            forward: true
        }
    );
    assert_eq!(
        kinds("12b").remove(0),
        TokenKind::TempRef {
            id: 12,
            forward: false
        }
    );
    // `1b` collides with binary 1, and the literal wins; `1_B` is how you
    // write the label and it is never ambiguous.
    assert_eq!(number("1b"), 1);
    assert_eq!(number("1010b"), 0b1010);
    assert_eq!(
        kinds("1_B").remove(0),
        TokenKind::TempRef {
            id: 1,
            forward: false
        }
    );
}

#[test]
fn dollar_is_the_current_address_unless_digits_follow() {
    assert!(matches!(
        &kinds("jr $+2")[..],
        [
            TokenKind::Ident(_),
            TokenKind::Here,
            TokenKind::Sym(Sym::Plus),
            TokenKind::Number { value: 2, .. },
            ..
        ]
    ));
    assert_eq!(number("$2"), 2);
    // `$$` is the start of the current section.
    assert_eq!(kinds("$$").remove(0), TokenKind::SectionStart);
    assert!(matches!(
        &kinds("$-$$")[..],
        [
            TokenKind::Here,
            TokenKind::Sym(Sym::Minus),
            TokenKind::SectionStart,
            ..
        ]
    ));
}

#[test]
fn strings_and_character_literals() {
    assert_eq!(bytes("'a'"), b"a".to_vec());
    assert_eq!(bytes("\"hello\""), b"hello".to_vec());
    // Escapes are processed between double quotes only.
    assert_eq!(bytes("\"\\n\""), vec![10]);
    assert_eq!(bytes("\"\\d\\e\\v\""), vec![127, 27, 11]);
    assert_eq!(bytes("'\\n'"), b"\\n".to_vec());
    // Between apostrophes, a doubled apostrophe is one apostrophe.
    assert_eq!(bytes("'it''s'"), b"it's".to_vec());
}

#[test]
fn string_suffixes() {
    assert_eq!(bytes("\"hi\"z"), vec![b'h', b'i', 0]);
    assert_eq!(bytes("\"hi\"c"), vec![b'h', b'i' | 0x80]);
    let TokenKind::Str(s) = kinds("\"hi\"z").remove(0) else {
        panic!("not a string");
    };
    assert_eq!(s.suffix, Some(StrSuffix::Zero));
    assert_eq!(s.quote, Quote::Double);
    assert_eq!(&*s.raw, "\"hi\"z");
    // A letter that is part of a following word is not a suffix.
    assert!(matches!(
        &kinds("\"hi\" code")[..],
        [TokenKind::Str(_), TokenKind::Ident(_), ..]
    ));
}

#[test]
fn shadow_register_apostrophe_attaches_to_the_register() {
    let ks = kinds("ex af,af'");
    assert_eq!(ks[3], TokenKind::Ident("af'".into()));
    // But an apostrophe after any other word still opens a string, so
    // `db 'x'` is unaffected.
    assert!(matches!(
        &kinds("db 'x'")[..],
        [TokenKind::Ident(_), TokenKind::Str(_), ..]
    ));
}

#[test]
fn comments_in_all_three_forms() {
    assert!(matches!(
        &kinds("nop ; trailing\n")[..],
        [TokenKind::Ident(_), TokenKind::Newline, TokenKind::Eof]
    ));
    assert!(matches!(
        &kinds("nop // trailing\n")[..],
        [TokenKind::Ident(_), TokenKind::Newline, TokenKind::Eof]
    ));
    // Block comments nest, and a newline inside one still ends its line so
    // that a commented-out block does not swallow the line structure.
    assert!(matches!(
        &kinds("/* a /* b */ c */nop")[..],
        [TokenKind::Ident(_), TokenKind::Eof]
    ));
    assert!(matches!(
        &kinds("/* one\ntwo */nop")[..],
        [TokenKind::Newline, TokenKind::Ident(_), TokenKind::Eof]
    ));
}

#[test]
fn every_token_records_its_file_and_position() {
    let mut map = SourceMap::new();
    let first = map.add("first.asm", "nop\n");
    let second = map.add("second.asm", "  ld a,1\n");

    let a = lex(&map, first).tokens;
    let b = lex(&map, second).tokens;
    assert_eq!(a[0].span.file, first);
    assert_eq!(b[0].span.file, second);
    assert_ne!(a[0].span.file, b[0].span.file);

    assert_eq!(map.location(a[0].span).to_string(), "first.asm:1:1");
    assert_eq!(map.location(b[0].span).to_string(), "second.asm:1:3");
    assert_eq!(map.snippet(b[0].span), "ld");
    // Column 1 is what tells a label from a mnemonic, so it is recorded.
    assert!(a[0].at_line_start);
    assert!(!b[0].at_line_start);
}

#[test]
fn line_and_column_survive_multiple_lines() {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", "nop\n\n    ld a,$FF\n");
    let tokens = lex(&map, file).tokens;
    let dollar = tokens
        .iter()
        .find(|t| matches!(&t.kind, TokenKind::Number { text, .. } if &**text == "$FF"))
        .expect("literal present");
    assert_eq!(map.location(dollar.span).to_string(), "t.asm:3:10");
}

#[test]
fn malformed_literals_are_reported() {
    assert_eq!(diagnostics("$FG"), ["`G` is not a hexadecimal digit"]);
    assert_eq!(diagnostics("#"), ["hexadecimal literal has no digits"]);
    assert_eq!(
        diagnostics("$100000000"),
        ["numeric literal does not fit in 32 bits"]
    );
    assert_eq!(
        diagnostics("db \"unterminated"),
        ["unterminated string literal"]
    );
    assert_eq!(diagnostics("db \"\\q\""), ["unknown escape sequence `\\q`"]);
    assert_eq!(diagnostics("/* open"), ["unterminated block comment"]);
    assert_eq!(diagnostics("nop \\"), ["unexpected character `\\`"]);
}
