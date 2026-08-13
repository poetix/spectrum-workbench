//! Collecting debug information: what the debugger needs to talk about source.
//!
//! The format, its indexes and the questions asked of it are in `rkw-dbginfo`,
//! because they are a contract between this program and a different one and
//! belong to neither (ADR-0019). What is left here is the part that is
//! genuinely the assembler's: turning an [`Assembled`] and its [`SourceMap`]
//! into the records that go in the file.

// Re-exported rather than merely used, so that `rkw_asm::debug` remains one
// place to look for everything about the sidecar.
pub use rkw_dbginfo::{DebugInfo, Expansion, Kind, Line, Position, Symbol, VERSION};

use crate::assemble::Assembled;
use crate::source::{SourceMap, Span};
use crate::symbols::SymbolKind;

/// Collect the debug information for an assembled program.
pub fn info(map: &SourceMap, assembled: &mut Assembled) -> DebugInfo {
    let files = (0..map.file_count())
        .map(|index| map.file(map.file_id(index)).name().to_string())
        .collect();

    let position = |span: Span| {
        let location = map.location(span);
        Position {
            file: span.file.index() as u32,
            line: location.line,
            column: location.column,
        }
    };

    let lines = assembled
        .lines
        .iter()
        .map(|record| Line {
            address: record.address,
            length: record.length,
            at: position(record.span),
            expansion: record.expansion.map(|index| index as u32),
        })
        .collect();

    let expansions = assembled
        .expansions
        .iter()
        .map(|expansion| Expansion {
            name: expansion.name.clone(),
            invoked_at: position(expansion.invoked_at),
            defined_at: position(expansion.defined_at),
            parent: expansion.parent.map(|index| index as u32),
        })
        .collect();

    let symbols = assembled
        .symbols
        .entries()
        .into_iter()
        .map(|(name, value, kind)| {
            let at = assembled
                .symbols
                .span_of(&name)
                .map(position)
                .unwrap_or(Position {
                    file: 0,
                    line: 1,
                    column: 1,
                });
            Symbol {
                name,
                value,
                kind: kind_of(kind),
                at,
            }
        })
        .collect();

    DebugInfo::from_parts(files, lines, symbols, expansions)
}

fn kind_of(kind: SymbolKind) -> Kind {
    match kind {
        SymbolKind::Label => Kind::Label,
        SymbolKind::Const => Kind::Constant,
        SymbolKind::Variable => Kind::Variable,
    }
}
