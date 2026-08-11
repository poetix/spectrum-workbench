//! Debug information: what the debugger needs to talk about source.
//!
//! Two questions, and the second is why this is more than a list. *What source
//! produced the instruction at this address* has one answer. *What addresses
//! did this line produce* has as many answers as the line has expansions — a
//! line inside a macro used five times produced five, and a breakpoint on it
//! means all five. Both directions are indexed here.
//!
//! The format is documented in `docs/debug-info.md` and versioned, because the
//! debugger is a separate program that will be built against it. It is text:
//! read once at load, so parsing speed buys nothing, while being greppable buys
//! a great deal the first time something disagrees about which line an address
//! belongs to.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::assemble::Assembled;
use crate::source::SourceMap;
use crate::symbols::SymbolKind;

/// The format version this module writes and accepts.
pub const VERSION: u32 = 1;

/// A position in a source file. 1-based, and columns count characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub file: u32,
    pub line: u32,
    pub column: u32,
}

/// A run of bytes and the source that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub address: u16,
    pub length: u16,
    pub at: Position,
    /// The macro expansion this was assembled inside.
    pub expansion: Option<u32>,
}

impl Line {
    /// True if `address` falls inside this entry. Computed in `u32` because a
    /// run ending at $FFFF would otherwise wrap to nothing.
    pub fn covers(&self, address: u16) -> bool {
        let start = u32::from(self.address);
        (start..start + u32::from(self.length)).contains(&u32::from(address))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Label,
    Constant,
    Variable,
}

impl Kind {
    pub fn text(self) -> &'static str {
        match self {
            Kind::Label => "label",
            Kind::Constant => "constant",
            Kind::Variable => "variable",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "label" => Kind::Label,
            "constant" => Kind::Constant,
            "variable" => Kind::Variable,
            _ => return None,
        })
    }
}

impl From<SymbolKind> for Kind {
    fn from(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Label => Kind::Label,
            SymbolKind::Const => Kind::Constant,
            SymbolKind::Variable => Kind::Variable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub value: i64,
    pub kind: Kind,
    pub at: Position,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub name: String,
    pub invoked_at: Position,
    pub defined_at: Position,
    pub parent: Option<u32>,
}

/// Everything the debugger is told about a program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugInfo {
    pub files: Vec<String>,
    /// Sorted by address, so a lookup is a binary search.
    pub lines: Vec<Line>,
    /// Sorted by name.
    pub symbols: Vec<Symbol>,
    pub expansions: Vec<Expansion>,
    /// Source position to the addresses it produced. Derived from `lines`
    /// rather than stored, because it is the same information the other way
    /// round and two copies could disagree.
    by_source: HashMap<(u32, u32), Vec<u16>>,
}

impl DebugInfo {
    /// Collect the debug information for an assembled program.
    pub fn new(map: &SourceMap, assembled: &mut Assembled) -> Self {
        let files = (0..map.file_count())
            .map(|index| map.file(map.file_id(index)).name().to_string())
            .collect();

        let position = |span: crate::source::Span| {
            let location = map.location(span);
            Position {
                file: span.file.index() as u32,
                line: location.line,
                column: location.column,
            }
        };

        let mut lines: Vec<Line> = assembled
            .lines
            .iter()
            .map(|record| Line {
                address: record.address,
                length: record.length,
                at: position(record.span),
                expansion: record.expansion.map(|index| index as u32),
            })
            .collect();
        lines.sort_by_key(|line| (line.address, line.at));

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

        let spans: Vec<_> = assembled
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
                    kind: kind.into(),
                    at,
                }
            })
            .collect();

        let mut info = Self {
            files,
            lines,
            symbols: spans,
            expansions,
            by_source: HashMap::new(),
        };
        info.index();
        info
    }

    fn index(&mut self) {
        self.by_source.clear();
        for line in &self.lines {
            self.by_source
                .entry((line.at.file, line.at.line))
                .or_default()
                .push(line.address);
        }
        for addresses in self.by_source.values_mut() {
            addresses.sort_unstable();
            addresses.dedup();
        }
    }

    /// The source that produced the instruction at `address`.
    pub fn line_at(&self, address: u16) -> Option<&Line> {
        // The last entry starting at or before the address is the only one that
        // can cover it, since they do not nest.
        let at = self.lines.partition_point(|line| line.address <= address);
        self.lines[..at]
            .iter()
            .rev()
            .find(|line| line.covers(address))
    }

    /// Every address a source line produced, in order.
    ///
    /// One-to-many: a line inside a macro used five times produced five.
    pub fn addresses_of(&self, file: u32, line: u32) -> &[u16] {
        self.by_source
            .get(&(file, line))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn file_index(&self, name: &str) -> Option<u32> {
        self.files.iter().position(|f| f == name).map(|i| i as u32)
    }

    pub fn file_name(&self, index: u32) -> &str {
        self.files.get(index as usize).map_or("?", String::as_str)
    }

    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols
            .binary_search_by(|symbol| symbol.name.as_str().cmp(name))
            .ok()
            .map(|at| &self.symbols[at])
    }

    /// The symbols in address order, which is how a disassembly wants them.
    pub fn symbols_by_address(&self) -> Vec<&Symbol> {
        let mut symbols: Vec<&Symbol> = self.symbols.iter().collect();
        symbols.sort_by_key(|symbol| (symbol.value, symbol.name.as_str()));
        symbols
    }

    /// The expansions enclosing `index`, innermost first.
    pub fn expansion_chain(&self, index: u32) -> Vec<&Expansion> {
        let mut chain = Vec::new();
        let mut next = Some(index);
        while let Some(index) = next {
            let Some(expansion) = self.expansions.get(index as usize) else {
                break;
            };
            chain.push(expansion);
            next = expansion.parent;
        }
        chain
    }

    // -- the file format ----------------------------------------------------

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "rkw-debug\t{VERSION}");
        for (index, name) in self.files.iter().enumerate() {
            let _ = writeln!(out, "file\t{index}\t{}", escape(name));
        }
        for (index, expansion) in self.expansions.iter().enumerate() {
            let _ = writeln!(
                out,
                "expansion\t{index}\t{}\t{}\t{}\t{}",
                escape(&expansion.name),
                position(expansion.invoked_at),
                position(expansion.defined_at),
                optional(expansion.parent),
            );
        }
        for line in &self.lines {
            let _ = writeln!(
                out,
                "line\t{}\t{}\t{}\t{}",
                line.address,
                line.length,
                position(line.at),
                optional(line.expansion),
            );
        }
        for symbol in &self.symbols {
            let _ = writeln!(
                out,
                "symbol\t{}\t{}\t{}\t{}",
                escape(&symbol.name),
                symbol.value,
                symbol.kind.text(),
                position(symbol.at),
            );
        }
        out
    }

    pub fn write(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, self.to_text())
    }

    /// Read back what [`DebugInfo::to_text`] wrote.
    ///
    /// Unknown record types are ignored rather than refused, which is what
    /// makes adding one a compatible change; an unknown *version* is refused,
    /// because that says the records themselves mean something else.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut info = Self::default();
        let mut seen_header = false;

        for (number, line) in text.lines().enumerate() {
            let number = number + 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let fail = |what: &str| Err(format!("line {number}: {what}"));

            if !seen_header {
                if fields.first() != Some(&"rkw-debug") {
                    return fail("expected a `rkw-debug` header");
                }
                match fields.get(1).and_then(|v| v.parse::<u32>().ok()) {
                    Some(VERSION) => seen_header = true,
                    Some(other) => {
                        return fail(&format!("version {other} is not version {VERSION}"));
                    }
                    None => return fail("the version is missing"),
                }
                continue;
            }

            match fields[0] {
                "file" => {
                    let [_, _index, name] = fields[..] else {
                        return fail("a `file` record is an index and a name");
                    };
                    info.files.push(unescape(name));
                }
                "expansion" => {
                    if fields.len() != 10 {
                        return fail("an `expansion` record has ten fields");
                    }
                    info.expansions.push(Expansion {
                        name: unescape(fields[2]),
                        invoked_at: read_position(&fields[3..6], number)?,
                        defined_at: read_position(&fields[6..9], number)?,
                        parent: read_optional(fields[9], number)?,
                    });
                }
                "line" => {
                    if fields.len() != 7 {
                        return fail("a `line` record has seven fields");
                    }
                    info.lines.push(Line {
                        address: read(fields[1], number)? as u16,
                        length: read(fields[2], number)? as u16,
                        at: read_position(&fields[3..6], number)?,
                        expansion: read_optional(fields[6], number)?,
                    });
                }
                "symbol" => {
                    if fields.len() != 7 {
                        return fail("a `symbol` record has seven fields");
                    }
                    let Some(kind) = Kind::parse(fields[3]) else {
                        return fail(&format!("`{}` is not a kind of symbol", fields[3]));
                    };
                    info.symbols.push(Symbol {
                        name: unescape(fields[1]),
                        value: fields[2].parse().map_err(|_| {
                            format!("line {number}: `{}` is not a number", fields[2])
                        })?,
                        kind,
                        at: read_position(&fields[4..7], number)?,
                    });
                }
                // Deliberately not an error: see the note on compatibility in
                // docs/debug-info.md.
                _ => {}
            }
        }

        if !seen_header {
            return Err("no `rkw-debug` header".into());
        }
        info.lines.sort_by_key(|line| (line.address, line.at));
        info.symbols.sort_by(|a, b| a.name.cmp(&b.name));
        info.index();
        Ok(info)
    }
}

fn position(at: Position) -> String {
    format!("{}\t{}\t{}", at.file, at.line, at.column)
}

fn optional(index: Option<u32>) -> String {
    match index {
        Some(index) => index.to_string(),
        None => "-".to_string(),
    }
}

fn read(field: &str, number: usize) -> Result<u32, String> {
    field
        .parse()
        .map_err(|_| format!("line {number}: `{field}` is not a number"))
}

fn read_position(fields: &[&str], number: usize) -> Result<Position, String> {
    Ok(Position {
        file: read(fields[0], number)?,
        line: read(fields[1], number)?,
        column: read(fields[2], number)?,
    })
}

fn read_optional(field: &str, number: usize) -> Result<Option<u32>, String> {
    match field {
        "-" => Ok(None),
        _ => Ok(Some(read(field, number)?)),
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
