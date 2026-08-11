//! The listing: what was assembled, against what was written.
//!
//! ```text
//! ; ---- main.asm ----
//!                   ; count down from four
//! 8000  06 04       wait:   ld b,4
//! 8002  10 FE       .loop:  djnz .loop
//! 8004  C9                  ret
//! ```
//!
//! Statements produced by a macro are printed under the line that invoked them,
//! marked with one `>` per level of nesting, and showing the macro body's own
//! source line — which is where the reader has to look to understand the bytes,
//! and is not otherwise anywhere near them.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::assemble::{Assembled, Image, LineRecord};
use crate::debug::{DebugInfo, Symbol};
use crate::source::{FileId, SourceMap};

/// How many encoded bytes fit on one listing line before the rest continue on
/// the next.
const BYTES_PER_LINE: usize = 4;

/// The width of the bytes column: two digits and a space per byte.
const BYTES_WIDTH: usize = BYTES_PER_LINE * 3;

/// Everything before the source text, so that lines which emitted nothing
/// still line up with those that did.
const PREFIX_WIDTH: usize = 6 + BYTES_WIDTH;

/// Produce a listing for an assembled program.
///
/// Files appear in the order they were registered — the root first, then each
/// `INCLUDE`d file in full — rather than spliced in at the point of inclusion.
/// That keeps every line of a file next to the lines around it, which is what a
/// person reading a listing is usually following.
pub fn listing(map: &SourceMap, assembled: &Assembled) -> String {
    // Grouped up front: a listing walks every source line, and scanning every
    // record for each of them would be quadratic in the size of the program.
    let mut written_here: HashMap<(FileId, u32), Vec<&LineRecord>> = HashMap::new();
    let mut expanded_here: HashMap<(FileId, u32), Vec<(&LineRecord, usize)>> = HashMap::new();

    for record in &assembled.lines {
        match record.expansion {
            None => written_here
                .entry((record.span.file, map.location(record.span).line))
                .or_default()
                .push(record),
            Some(expansion) => {
                let (outermost, depth) = outermost_expansion(assembled, expansion);
                let invoked = assembled.expansions[outermost].invoked_at;
                expanded_here
                    .entry((invoked.file, map.location(invoked).line))
                    .or_default()
                    .push((record, depth));
            }
        }
    }

    let mut out = String::new();
    for index in 0..map.file_count() {
        let id = map.file_id(index);
        let file = map.file(id);
        if index > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "; ---- {} ----", file.name());

        for number in 1..=file.line_count() {
            let text = file.line_text(number);
            match written_here.get(&(id, number)) {
                None => {
                    // A line that emitted nothing — a comment, a label on its
                    // own, an `EQU` — still belongs in the listing.
                    let _ = writeln!(out, "{:PREFIX_WIDTH$}{text}", "");
                }
                Some(records) => {
                    for (at, record) in records.iter().enumerate() {
                        // Several statements on one line, which `:` allows.
                        let text = if at == 0 { text } else { "" };
                        write_record(&mut out, &assembled.image, record, "", text);
                    }
                }
            }

            for (record, depth) in expanded_here.get(&(id, number)).into_iter().flatten() {
                let source = map
                    .file(record.span.file)
                    .line_text(map.location(record.span).line);
                let marker = format!("{} ", ">".repeat(*depth));
                write_record(
                    &mut out,
                    &assembled.image,
                    record,
                    &marker,
                    source.trim_end(),
                );
            }
        }
    }
    out
}

/// The outermost expansion of a chain, and how deeply nested it is.
fn outermost_expansion(assembled: &Assembled, mut index: usize) -> (usize, usize) {
    let mut depth = 1;
    while let Some(parent) = assembled.expansions[index].parent {
        index = parent;
        depth += 1;
    }
    (index, depth)
}

fn write_record(out: &mut String, image: &Image, record: &LineRecord, marker: &str, text: &str) {
    let bytes: Vec<u8> = (0..record.length)
        .map(|offset| image.byte_at(record.address.wrapping_add(offset)))
        .collect();

    let mut chunks = bytes.chunks(BYTES_PER_LINE);
    let first = chunks.next().unwrap_or(&[]);
    let _ = writeln!(
        out,
        "{:04X}  {:BYTES_WIDTH$}{marker}{text}",
        record.address,
        hex(first)
    );

    // A `DB` of twenty bytes continues underneath, addresses and all, rather
    // than running off the side.
    for (index, chunk) in chunks.enumerate() {
        let address = record
            .address
            .wrapping_add(((index + 1) * BYTES_PER_LINE) as u16);
        let _ = writeln!(out, "{address:04X}  {}", hex(chunk).trim_end());
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        let _ = write!(out, "{byte:02X} ");
    }
    out
}

/// A symbol table sorted by name.
pub fn symbols_by_name(info: &DebugInfo) -> String {
    let mut out = String::new();
    for symbol in &info.symbols {
        let _ = writeln!(out, "{}", symbol_line(info, symbol));
    }
    out
}

/// A symbol table sorted by value, which for labels is address order.
pub fn symbols_by_address(info: &DebugInfo) -> String {
    let mut out = String::new();
    for symbol in info.symbols_by_address() {
        let _ = writeln!(out, "{}", symbol_line(info, symbol));
    }
    out
}

fn symbol_line(info: &DebugInfo, symbol: &Symbol) -> String {
    format!(
        "{:<24} ${:04X}  {:<9} {}:{}",
        symbol.name,
        symbol.value & 0xFFFF,
        symbol.kind.text(),
        info.file_name(symbol.at.file),
        symbol.at.line,
    )
}
