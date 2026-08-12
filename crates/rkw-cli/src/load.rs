//! Getting a program into the machine: assembler source, or raw bytes.
//!
//! Assembling here rather than making the user run the assembler first is what
//! the scriptability argument in ADR-0013 is for — assemble, run to an address,
//! assert a register, in one command. Resolving a *label* rather than an
//! address is ticket 0011 and needs the debug info, which is why this stops at
//! loading the image.

use std::fmt;
use std::path::Path;

use rkw_asm::{SourceMap, assemble};
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
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// Assembly failed. The rendered diagnostics, ready to print.
    Assembly(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
            LoadError::Assembly(text) => write!(f, "{}", text.trim_end()),
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
    let assembled = assemble(&mut map, file);
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
