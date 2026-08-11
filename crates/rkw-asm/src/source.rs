//! Source text, file identity and byte spans.
//!
//! Everything the front end produces — tokens, AST nodes, diagnostics — points
//! back at the original text through a [`Span`], which carries the file it came
//! from as well as its byte range. That is what lets an error raised while
//! assembling an `INCLUDE`d file three levels down still name the right file,
//! and it is why the span is three fields rather than two.
//!
//! Byte offsets, not line/column: a lexer that maintained a line counter would
//! have to keep it correct through every branch, and line/column is only wanted
//! at the moment something is printed. [`SourceMap::location`] recovers it then,
//! from a line-start index built once per file.

use std::path::{Path, PathBuf};

/// A file registered with a [`SourceMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(u32);

impl FileId {
    pub(crate) fn from_index(index: u32) -> Self {
        FileId(index)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A byte range within one source file.
///
/// `start` and `end` are byte offsets and always fall on character boundaries,
/// because the lexer only ever splits between characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Self {
        Self { file, start, end }
    }

    /// A zero-width span, for pointing at a position rather than at text —
    /// "expected `)` here", where the `)` is exactly what is missing.
    pub fn at(file: FileId, at: u32) -> Self {
        Self::new(file, at, at)
    }

    /// The span covering both, used to give a composite node the extent of
    /// everything it was built from.
    pub fn to(self, other: Span) -> Span {
        debug_assert_eq!(self.file, other.file, "cannot join spans across files");
        Span::new(
            self.file,
            self.start.min(other.start),
            self.end.max(other.end),
        )
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// One registered file: its name, its text, and the offset of each line start.
pub struct SourceFile {
    name: String,
    /// Where it was read from, if it was read from anywhere. `INCLUDE` and
    /// `INCBIN` resolve their paths against the *including* file's directory
    /// rather than the process working directory, so a source tree can be
    /// assembled from anywhere.
    path: Option<PathBuf>,
    text: String,
    /// Byte offset of the start of each line. Always begins with 0, so the
    /// number of entries is the number of lines.
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The directory a relative path in this file should be resolved against.
    pub fn directory(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The text of `line` (1-based), without its terminator.
    pub fn line_text(&self, line: u32) -> &str {
        let idx = (line.max(1) - 1) as usize;
        let Some(&start) = self.line_starts.get(idx) else {
            return "";
        };
        let end = self
            .line_starts
            .get(idx + 1)
            .map_or(self.text.len(), |&e| e as usize);
        self.text[start as usize..end].trim_end_matches(['\n', '\r'])
    }

    /// The 1-based line containing `offset`.
    fn line_of(&self, offset: u32) -> u32 {
        // partition_point gives the number of line starts at or before the
        // offset, which is exactly the 1-based line number.
        self.line_starts.partition_point(|&s| s <= offset).max(1) as u32
    }
}

/// The set of files the front end has been given.
///
/// Files are added once and never removed, so a [`FileId`] is valid for the
/// lifetime of the map and spans can be stored freely.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a file and return its id.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        self.add_at(name, None, text)
    }

    /// Read a file from disk and register it under its path.
    pub fn load(&mut self, path: impl AsRef<Path>) -> std::io::Result<FileId> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        Ok(self.add_at(path.display().to_string(), Some(path.to_path_buf()), text))
    }

    fn add_at(
        &mut self,
        name: impl Into<String>,
        path: Option<PathBuf>,
        text: impl Into<String>,
    ) -> FileId {
        let text: String = text.into();
        let mut line_starts = vec![0u32];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| (i + 1) as u32),
        );
        // A trailing newline does not open a line that anything can point into.
        // Empty text still has one (empty) line, so never pop the last entry.
        if line_starts.len() > 1 && line_starts.last() == Some(&(text.len() as u32)) {
            line_starts.pop();
        }
        self.files.push(SourceFile {
            name: name.into(),
            path,
            text,
            line_starts,
        });
        FileId((self.files.len() - 1) as u32)
    }

    pub fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.index()]
    }

    /// How many files have been registered. Ids are `0..file_count`, in the
    /// order they were added, which is the order a listing walks them.
    pub fn file_count(&self) -> u32 {
        self.files.len() as u32
    }

    pub fn file_id(&self, index: u32) -> FileId {
        assert!(index < self.file_count(), "no such file");
        FileId::from_index(index)
    }

    /// The text a span covers.
    pub fn snippet(&self, span: Span) -> &str {
        let text = self.file(span.file).text();
        let start = (span.start as usize).min(text.len());
        let end = (span.end as usize).clamp(start, text.len());
        &text[start..end]
    }

    /// Where a span starts, in the terms a person and an editor both use.
    pub fn location(&self, span: Span) -> Location<'_> {
        let file = self.file(span.file);
        let line = file.line_of(span.start);
        let line_start = file.line_starts[(line - 1) as usize] as usize;
        let upto = &file.text[line_start..(span.start as usize).min(file.text.len())];
        Location {
            file: &file.name,
            line,
            // Columns count characters, not bytes: a comment in Cyrillic ahead
            // of the error should not push the reported column past the end of
            // the line as the reader sees it.
            column: upto.chars().count() as u32 + 1,
        }
    }
}

/// A human-facing position: 1-based line and character column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location<'a> {
    pub file: &'a str,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for Location<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}
