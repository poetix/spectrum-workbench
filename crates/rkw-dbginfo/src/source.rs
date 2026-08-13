//! Debug info plus the text it talks about, and the questions a front end asks
//! of the pair.
//!
//! [`DebugInfo`] knows that address $8003 came from file 0, line 5. Turning
//! that into something a person reads needs the file as well, and turning
//! `break main.asm:42` into addresses needs to decide which of the files listed
//! in the debug info `main.asm` meant. Both are here, above the format and
//! below any user interface, because the REPL and the DAP adapter (ticket 0023)
//! ask exactly the same questions and neither should be answering them itself.
//!
//! Nothing here consults a machine. A [`Sources`] is what was assembled, not
//! what is running, so every answer is a pure function of the debug info and
//! the text — which is what lets the resolution be tested without an emulator.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::info::{DebugInfo, Position};

/// Where a source line ended up, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// The text of that line, when the file could be read.
    pub text: Option<String>,
    /// The macro expansions this line was assembled inside, innermost first.
    pub frames: Vec<Frame>,
    /// The source has changed since the debug info was written, so `text` may
    /// not be the line that produced the code.
    pub stale: bool,
}

/// One level of macro expansion: which macro, and where it was invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub name: String,
    /// Where the invocation is written.
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// The addresses a source line produced, and which line that turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub file: String,
    /// The line that was asked for.
    pub requested: u32,
    /// The line that had code on it, which is [`Site::requested`] unless that
    /// line produced no bytes and the search moved on.
    pub line: u32,
    /// Every address the line produced, in order. Never empty.
    pub addresses: Vec<u16>,
}

impl Site {
    /// True when the line asked for produced nothing and a later one answered.
    pub fn moved(&self) -> bool {
        self.line != self.requested
    }
}

/// A window of source text, for `list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub file: String,
    /// The line number of `lines[0]`, 1-based.
    pub first: u32,
    pub lines: Vec<String>,
    /// The line to mark, when the window was built around one.
    pub current: Option<u32>,
    pub stale: bool,
}

/// Why a name did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    UnknownFile(String),
    /// The spec matched more than one of the files in the debug info, so it is
    /// refused rather than guessed at.
    AmbiguousFile {
        spec: String,
        matches: Vec<String>,
    },
    /// The line, and every line after it, produced no code.
    NoCode {
        file: String,
        line: u32,
    },
    UnknownSymbol(String),
    /// A symbol exists but is not an address — a constant that does not fit in
    /// sixteen bits.
    NotAnAddress {
        name: String,
        value: i64,
    },
    /// The file is named in the debug info but its text was never read.
    NoText(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::UnknownFile(spec) => {
                write!(f, "no source file matching `{spec}`")
            }
            ResolveError::AmbiguousFile { spec, matches } => {
                write!(f, "`{spec}` matches {}", matches.join(", "))
            }
            ResolveError::NoCode { file, line } => {
                write!(
                    f,
                    "{file}:{line} produced no code, and nor does any line after it"
                )
            }
            ResolveError::UnknownSymbol(name) => write!(f, "no symbol `{name}`"),
            ResolveError::NotAnAddress { name, value } => {
                write!(f, "`{name}` is {value}, which is not an address")
            }
            ResolveError::NoText(file) => write!(f, "the text of {file} was not read"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// One file's text, as far as it is known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileText {
    lines: Vec<String>,
    read: bool,
    /// The file on disk is newer than the debug info describing it.
    stale: bool,
}

/// A program's debug info, and as much of its source text as could be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    info: DebugInfo,
    /// Parallel to `info.files`.
    files: Vec<FileText>,
}

impl Sources {
    pub fn new(info: DebugInfo) -> Sources {
        let files = vec![FileText::default(); info.files.len()];
        Sources { info, files }
    }

    /// Read the text of every file the debug info names.
    ///
    /// Names are relative to wherever the sidecar was written from, which is
    /// not necessarily the process's working directory, so `base` is tried as
    /// well as the name itself. `newer_than` is the sidecar's own timestamp: a
    /// source file modified after it is recorded as stale, because the text
    /// would otherwise be shown against addresses that were assembled from
    /// something else.
    pub fn load(info: DebugInfo, base: Option<&Path>, newer_than: Option<SystemTime>) -> Sources {
        let mut sources = Sources::new(info);
        for index in 0..sources.files.len() {
            let name = sources.info.files[index].clone();
            let Some(path) = find(&name, base) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            sources.set_text(index as u32, &text);
            if let (Some(reference), Ok(modified)) = (
                newer_than,
                std::fs::metadata(&path).and_then(|m| m.modified()),
            ) {
                sources.files[index].stale = modified > reference;
            }
        }
        sources
    }

    /// Supply a file's text directly, for a caller that has it already — the
    /// CLI assembles the program itself and has never been near the disk twice.
    pub fn set_text(&mut self, file: u32, text: &str) {
        let Some(entry) = self.files.get_mut(file as usize) else {
            return;
        };
        entry.lines = text.lines().map(str::to_string).collect();
        entry.read = true;
    }

    pub fn set_text_of(&mut self, name: &str, text: &str) {
        if let Some(index) = self.info.file_index(name) {
            self.set_text(index, text);
        }
    }

    pub fn info(&self) -> &DebugInfo {
        &self.info
    }

    /// The files whose text on disk is newer than the debug info, which is the
    /// one thing that makes everything else here quietly wrong.
    pub fn stale(&self) -> Vec<&str> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, text)| text.stale)
            .map(|(index, _)| self.info.file_name(index as u32))
            .collect()
    }

    fn line_text(&self, file: u32, line: u32) -> Option<&str> {
        let entry = self.files.get(file as usize)?;
        entry
            .lines
            .get(line.checked_sub(1)? as usize)
            .map(String::as_str)
    }

    fn is_stale(&self, file: u32) -> bool {
        self.files.get(file as usize).is_some_and(|f| f.stale)
    }

    /// Which of the debug info's files a spec names.
    ///
    /// An exact match wins outright. Otherwise the spec has to be a whole
    /// trailing run of path components — `main.asm` matches `src/main.asm` and
    /// not `domain.asm` — and it has to match one file, because a debugger that
    /// picked one of two `util.asm`s would be wrong half the time silently.
    pub fn file_matching(&self, spec: &str) -> Result<u32, ResolveError> {
        if let Some(index) = self.info.file_index(spec) {
            return Ok(index);
        }
        let matches: Vec<u32> = (0..self.info.files.len() as u32)
            .filter(|index| suffix_match(self.info.file_name(*index), spec))
            .collect();
        match matches.as_slice() {
            [] => Err(ResolveError::UnknownFile(spec.to_string())),
            [one] => Ok(*one),
            many => Err(ResolveError::AmbiguousFile {
                spec: spec.to_string(),
                matches: many
                    .iter()
                    .map(|index| self.info.file_name(*index).to_string())
                    .collect(),
            }),
        }
    }

    /// Every address a source line produced, one-to-many.
    pub fn site(&self, spec: &str, line: u32) -> Result<Site, ResolveError> {
        let file = self.file_matching(spec)?;
        let name = self.info.file_name(file).to_string();
        let found = self
            .info
            .line_with_code(file, line)
            .ok_or(ResolveError::NoCode {
                file: name.clone(),
                line,
            })?;
        Ok(Site {
            file: name,
            requested: line,
            line: found,
            addresses: self.info.addresses_of(file, found).to_vec(),
        })
    }

    /// What a symbol names, as an address.
    pub fn address_of(&self, name: &str) -> Result<u16, ResolveError> {
        let symbol = self
            .info
            .symbol(name)
            .ok_or_else(|| ResolveError::UnknownSymbol(name.to_string()))?;
        u16::try_from(symbol.value).map_err(|_| ResolveError::NotAnAddress {
            name: name.to_string(),
            value: symbol.value,
        })
    }

    /// The source that produced the instruction at an address, with the macro
    /// expansions that led there.
    pub fn locate(&self, address: u16) -> Option<Located> {
        let line = self.info.line_at(address)?;
        Some(self.at(line.at, line.expansion))
    }

    fn at(&self, position: Position, expansion: Option<u32>) -> Located {
        let frames = expansion
            .map(|index| {
                self.info
                    .expansion_chain(index)
                    .into_iter()
                    .map(|e| Frame {
                        name: e.name.clone(),
                        file: self.info.file_name(e.invoked_at.file).to_string(),
                        line: e.invoked_at.line,
                        column: e.invoked_at.column,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Located {
            file: self.info.file_name(position.file).to_string(),
            line: position.line,
            column: position.column,
            text: self
                .line_text(position.file, position.line)
                .map(str::to_string),
            frames,
            stale: self.is_stale(position.file),
        }
    }

    /// A window of `radius` lines either side of one, for `list`.
    pub fn listing(&self, spec: &str, line: u32, radius: u32) -> Result<Listing, ResolveError> {
        let file = self.file_matching(spec)?;
        self.listing_at(file, line, radius)
    }

    pub fn listing_at(&self, file: u32, line: u32, radius: u32) -> Result<Listing, ResolveError> {
        let name = self.info.file_name(file).to_string();
        let entry = self
            .files
            .get(file as usize)
            .filter(|entry| entry.read)
            .ok_or_else(|| ResolveError::NoText(name.clone()))?;
        let first = line.saturating_sub(radius).max(1);
        let last = line.saturating_add(radius).min(entry.lines.len() as u32);
        let lines = if first > last {
            Vec::new()
        } else {
            entry.lines[(first - 1) as usize..last as usize].to_vec()
        };
        Ok(Listing {
            file: name,
            first,
            lines,
            current: (line >= first && line <= last).then_some(line),
            stale: entry.stale,
        })
    }
}

/// True when `spec` is a whole trailing run of `name`'s path components.
fn suffix_match(name: &str, spec: &str) -> bool {
    let Some(rest) = name.strip_suffix(spec) else {
        return false;
    };
    rest.ends_with(['/', '\\'])
}

/// Where a file named in the debug info might actually be: as named, or
/// relative to wherever the sidecar came from.
fn find(name: &str, base: Option<&Path>) -> Option<PathBuf> {
    let as_named = PathBuf::from(name);
    if as_named.is_file() {
        return Some(as_named);
    }
    let base = base?;
    let joined = base.join(name);
    if joined.is_file() {
        return Some(joined);
    }
    let by_name = base.join(as_named.file_name()?);
    by_name.is_file().then_some(by_name)
}
