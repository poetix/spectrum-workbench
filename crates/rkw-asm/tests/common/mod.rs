#![allow(dead_code)]
//! Shared test helpers.
//!
//! The driver these used to carry has been replaced by the real one in
//! `rkw_asm::assemble`, which is what ticket 0004 delivered.

use rkw_asm::encode::{self, EncodeError};
use rkw_asm::{Assembled, Site, SourceMap, Symbols, parse};

pub use rkw_asm::assemble;

/// Assemble source text held in memory, and require it to succeed.
pub fn assemble_ok(source: &str) -> (SourceMap, Assembled) {
    let mut map = SourceMap::new();
    let file = map.add("program.asm", source);
    let assembled = assemble(&mut map, file);
    assert!(
        assembled.diagnostics.is_empty(),
        "did not assemble:\n{}",
        map.render_all(&assembled.diagnostics)
    );
    (map, assembled)
}

/// The messages an assembly produced, for the tests that expect it to fail.
pub fn errors(source: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.add("program.asm", source);
    assemble(&mut map, file)
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

pub fn symbol(assembled: &mut Assembled, name: &str) -> i64 {
    assembled
        .symbols
        .iter_values()
        .into_iter()
        .find(|(defined, _)| defined == name)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("`{name}` is not defined"))
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
