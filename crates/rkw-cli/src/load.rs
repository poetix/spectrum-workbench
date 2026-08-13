//! Getting a program into the machine: assembler source, or raw bytes.
//!
//! Assembling here rather than making the user run the assembler first is what
//! the scriptability argument in ADR-0013 is for — assemble, run to a label,
//! assert a register, in one command.
//!
//! Debug information comes with the program rather than being asked for. When
//! the source is assembled here it is built from the same [`SourceMap`], so the
//! text the debugger shows is the text that was assembled and cannot be stale.
//! When a binary is loaded instead, the sidecar it was assembled with is read
//! from disk, and then it can be — so the source files are timestamped against
//! it and anything newer is reported rather than shown as if it ran.

use std::fmt;
use std::path::Path;

use rkw_asm::{SourceMap, assemble, debug_info};
use rkw_dbginfo::{DebugInfo, Sources};
use z80::FlatMemory;

use crate::Loaded;

/// What was put into memory, and where execution should start.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Program {
    pub loaded: Vec<Loaded>,
    /// The lowest address the program occupies, which is the best guess at an
    /// entry point available without debug info.
    pub entry: Option<u16>,
    /// Diagnostics that did not stop the assembly, already rendered.
    pub notes: String,
    /// What the debugger needs to talk about source, when anything said.
    pub sources: Option<Sources>,
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// Assembly failed. The rendered diagnostics, ready to print.
    Assembly(String),
    /// A `.rkwdbg` sidecar that could be read and not understood.
    DebugInfo {
        path: String,
        message: String,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
            LoadError::Assembly(text) => write!(f, "{}", text.trim_end()),
            LoadError::DebugInfo { path, message } => write!(f, "{path}: {message}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> LoadError {
        LoadError::Io(e)
    }
}

/// Assemble a source file and load every segment it produced.
pub fn assemble_file(mem: &mut FlatMemory, path: &Path) -> Result<Program, LoadError> {
    let mut map = SourceMap::new();
    let file = map.load(path)?;
    let mut assembled = assemble(&mut map, file);
    let rendered: String = assembled
        .diagnostics
        .iter()
        .map(|d| map.render(d))
        .collect();
    if assembled.has_errors() {
        return Err(LoadError::Assembly(rendered));
    }
    let mut program = Program {
        entry: assembled.image.origin(),
        notes: rendered,
        sources: Some(assembled_sources(&map, &mut assembled)),
        ..Program::default()
    };
    for segment in assembled.image.segments() {
        mem.load(segment.origin, &segment.bytes);
        program.loaded.push(Loaded {
            path: path.to_path_buf(),
            origin: segment.origin,
            len: segment.bytes.len(),
        });
    }
    Ok(program)
}

/// The debug info for something just assembled, with the text taken from the
/// source map rather than read again — it is the same file, and reading it
/// twice is the one way the two could disagree.
fn assembled_sources(map: &SourceMap, assembled: &mut rkw_asm::Assembled) -> Sources {
    let mut sources = Sources::new(debug_info(map, assembled));
    for index in 0..map.file_count() {
        sources.set_text(index, map.file(map.file_id(index)).text());
    }
    sources
}

/// Read a `.rkwdbg` sidecar and the source files it names.
///
/// Timestamps are compared against the sidecar's own: a source file modified
/// since is reported, because the addresses are still those of the binary and
/// the text beside them would no longer be what produced them.
pub fn debug_file(path: &Path) -> Result<Sources, LoadError> {
    let text = std::fs::read_to_string(path)?;
    let info = DebugInfo::parse(&text).map_err(|message| LoadError::DebugInfo {
        path: path.display().to_string(),
        message,
    })?;
    let written = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Ok(Sources::load(info, path.parent(), written))
}

/// The sidecar the assembler would have written beside a binary.
pub fn sidecar_of(path: &Path) -> std::path::PathBuf {
    path.with_extension("rkwdbg")
}

/// Load raw bytes at an address, as a tape-less way of running something that
/// has already been assembled.
pub fn binary_file(mem: &mut FlatMemory, path: &Path, origin: u16) -> Result<Program, LoadError> {
    let bytes = std::fs::read(path)?;
    mem.load(origin, &bytes);
    Ok(Program {
        loaded: vec![Loaded {
            path: path.to_path_buf(),
            origin,
            len: bytes.len(),
        }],
        entry: Some(origin),
        notes: String::new(),
        sources: None,
    })
}

/// `$8000`, `0x8000`, `%1010` or plain decimal, for the command line — the same
/// spellings the debugger's own parser takes.
pub fn number(text: &str) -> Option<u16> {
    let (radix, digits) = if let Some(rest) = text.strip_prefix('$') {
        (16, rest)
    } else if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        (16, rest)
    } else if let Some(rest) = text.strip_prefix('%') {
        (2, rest)
    } else {
        (10, text)
    };
    u32::from_str_radix(digits, radix)
        .ok()
        .filter(|v| *v <= 0xFFFF)
        .map(|v| v as u16)
}
