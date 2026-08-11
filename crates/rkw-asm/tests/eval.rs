//! Expression evaluation: operators, 32-bit arithmetic, and the range checks
//! that catch a value too big for the field it is going into.

use rkw_asm::eval::{EvalError, fit_byte, fit_relative, fit_signed_byte, fit_word};
use rkw_asm::{Expr, Site, SourceMap, Symbols, eval, parse};

/// Parse a single expression, by way of a statement that takes one operand.
fn expression(map: &mut SourceMap, src: &str) -> Expr {
    let file = map.add("t.asm", format!("    dw {src}\n"));
    let parsed = parse(map, file);
    assert!(
        parsed.diagnostics.is_empty(),
        "{src:?} did not parse:\n{}",
        map.render_all(&parsed.diagnostics)
    );
    let mut args = parsed
        .program
        .statements
        .into_iter()
        .next()
        .expect("one statement")
        .op
        .expect("an operation")
        .args;
    assert_eq!(args.len(), 1, "{src:?} is not one expression");
    args.remove(0)
}

fn value_at(src: &str, site: Site) -> i64 {
    let mut map = SourceMap::new();
    let expr = expression(&mut map, src);
    let mut symbols = Symbols::new();
    match eval(&expr, site, &mut symbols) {
        Ok(v) => v,
        Err(e) => panic!("{src:?} did not evaluate:\n{}", map.render(&e.diagnostic())),
    }
}

fn value(src: &str) -> i64 {
    value_at(src, Site::default())
}

fn error(src: &str) -> EvalError {
    let mut map = SourceMap::new();
    let expr = expression(&mut map, src);
    let mut symbols = Symbols::new();
    eval(&expr, Site::default(), &mut symbols).expect_err("should not evaluate")
}

#[test]
fn arithmetic() {
    assert_eq!(value("2+3*4"), 14);
    assert_eq!(value("(2+3)*4"), 20);
    assert_eq!(value("10-3-2"), 5);
    assert_eq!(value("7/2"), 3);
    assert_eq!(value("7%3"), 1);
    assert_eq!(value("7 mod 3"), 1);
    assert_eq!(value("-5"), -5);
    assert_eq!(value("--5"), 5);
    assert_eq!(value("+5"), 5);
}

#[test]
fn bitwise_and_shifts() {
    assert_eq!(value("$F0|$0F"), 0xFF);
    assert_eq!(value("$FF&$0F"), 0x0F);
    assert_eq!(value("$FF^$0F"), 0xF0);
    assert_eq!(value("~0"), -1);
    assert_eq!(value("1<<8"), 256);
    assert_eq!(value("1 shl 8"), 256);
    assert_eq!(value("$FF00>>8"), 0xFF);
    // `>>` keeps the sign; `>>>` does not.
    assert_eq!(value("-2>>1"), -1);
    assert_eq!(value("-2>>>1"), 0x7FFF_FFFF);
}

#[test]
fn comparison_and_logic() {
    assert_eq!(value("1<2"), 1);
    assert_eq!(value("2<2"), 0);
    assert_eq!(value("2<=2"), 1);
    assert_eq!(value("2=2"), 1);
    assert_eq!(value("2==2"), 1);
    assert_eq!(value("2!=2"), 0);
    assert_eq!(value("!0"), 1);
    assert_eq!(value("!7"), 0);
    assert_eq!(value("1&&2"), 1);
    assert_eq!(value("0||0"), 0);
    assert_eq!(value("3<?5"), 3);
    assert_eq!(value("3>?5"), 5);
}

#[test]
fn logical_operators_short_circuit() {
    // The usual guard idiom must not evaluate the division it is guarding.
    assert_eq!(value("0&&1/0"), 0);
    assert_eq!(value("1||1/0"), 1);
}

#[test]
fn byte_selectors() {
    assert_eq!(value("low $1234"), 0x34);
    assert_eq!(value("high $1234"), 0x12);
    assert_eq!(value("abs -5"), 5);
}

#[test]
fn character_literals_are_values() {
    assert_eq!(value("'a'"), 97);
    assert_eq!(value("'a'+1"), 98);
    assert_eq!(value("\"a\""), 97);
    // Anything longer is a string, and a string is not a number.
    assert!(matches!(error("\"ab\""), EvalError::NotAValue { .. }));
}

#[test]
fn arithmetic_is_32_bit_and_wraps() {
    // sjasmplus computes in 32 bits, so this is what the source it was
    // written for would have produced.
    assert_eq!(value("$FFFFFFFF"), -1);
    assert_eq!(value("$FFFF*$FFFF"), -131_071); // $FFFE0001 read as signed
    assert_eq!(value("$80000000-1"), 0x7FFF_FFFF);
}

#[test]
fn division_by_zero_is_an_error_not_a_panic() {
    assert!(matches!(error("1/0"), EvalError::DivideByZero { .. }));
    assert!(matches!(error("1%0"), EvalError::DivideByZero { .. }));
}

#[test]
fn here_and_section_start() {
    let site = Site::new(0x8010, 0x8000, 0);
    assert_eq!(value_at("$", site), 0x8010);
    assert_eq!(value_at("$$", site), 0x8000);
    // The idiom for "how far into this block are we".
    assert_eq!(value_at("$-$$", site), 0x10);
}

#[test]
fn ranges_accept_both_readings_of_a_byte() {
    // `LD A,-1` and `LD A,255` are the same instruction, so both fit.
    assert_eq!(fit_byte(255, span()).unwrap(), 0xFF);
    assert_eq!(fit_byte(-1, span()).unwrap(), 0xFF);
    assert!(fit_byte(256, span()).is_err());
    assert!(fit_byte(-129, span()).is_err());

    // An index displacement is signed only: 200 would silently become -56.
    assert_eq!(fit_signed_byte(127, span()).unwrap(), 127);
    assert!(fit_signed_byte(200, span()).is_err());

    assert_eq!(fit_word(65535, span()).unwrap(), 0xFFFF);
    assert_eq!(fit_word(-1, span()).unwrap(), 0xFFFF);
    assert!(fit_word(65536, span()).is_err());
}

#[test]
fn an_out_of_range_value_says_what_was_expected() {
    let d = fit_byte(300, span()).unwrap_err().diagnostic();
    assert_eq!(d.message, "300 does not fit in one byte");
    assert_eq!(d.caret_label.as_deref(), Some("expected -128 to 255"));
}

#[test]
fn relative_jumps_are_measured_from_the_next_instruction() {
    // JR at $8000 is two bytes, so a jump to $8002 is a displacement of zero.
    assert_eq!(fit_relative(0x8002, 0x8002, span()).unwrap(), 0);
    assert_eq!(fit_relative(0x8002, 0x8081, span()).unwrap(), 127);
    assert_eq!(fit_relative(0x8002, 0x7F82, span()).unwrap(), -128);

    let err = fit_relative(0x8002, 0x8082, span()).unwrap_err();
    assert!(matches!(err, EvalError::TooFar { distance: 128, .. }));
    // The error names the distance and the limit, so the fix is obvious.
    let d = err.diagnostic();
    assert_eq!(d.message, "relative jump of 128 bytes is out of range");
    assert_eq!(
        d.caret_label.as_deref(),
        Some("the displacement byte reaches -128 to 127")
    );
}

/// A span for the range checks, which do not care where they point.
fn span() -> rkw_asm::Span {
    let mut map = SourceMap::new();
    let file = map.add("t.asm", "nop\n");
    rkw_asm::Span::new(file, 0, 3)
}
