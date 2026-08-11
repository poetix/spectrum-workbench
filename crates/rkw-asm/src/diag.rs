//! Diagnostics: what went wrong, where, and how to show it.
//!
//! A diagnostic is data, not text. The front end builds them as it goes and
//! keeps assembling; whoever is driving decides whether to print them, count
//! them, or hand them to an editor as structured positions. [`SourceMap::render`]
//! is only the default rendering.

use crate::source::{SourceMap, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One problem, anchored to a span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Short text printed against the caret, where the message reads better
    /// split in two: "expected an operand" over the line, "found `,`" under it.
    pub caret_label: Option<String>,
    /// Further explanation, printed after the snippet.
    pub notes: Vec<String>,
    /// Other places that bear on this error — the `(` that was never closed,
    /// the earlier definition of a duplicate label. Rendered as notes naming
    /// their own file, line and column, so a related span in an included file
    /// still points somewhere useful.
    pub related: Vec<(Span, String)>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            span,
            caret_label: None,
            notes: Vec::new(),
            related: Vec::new(),
        }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(span, message)
        }
    }

    pub fn with_caret_label(mut self, label: impl Into<String>) -> Self {
        self.caret_label = Some(label.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_related(mut self, span: Span, note: impl Into<String>) -> Self {
        self.related.push((span, note.into()));
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl SourceMap {
    /// Render a diagnostic with the offending line and a caret under the span:
    ///
    /// ```text
    /// error: expected `)`
    ///  --> main.asm:2:14
    ///   |
    /// 2 |     ld a,(ix+1
    ///   |               ^ unclosed since column 10
    /// ```
    pub fn render(&self, d: &Diagnostic) -> String {
        use std::fmt::Write as _;

        let loc = self.location(d.span);
        let file = self.file(d.span.file);
        let line_text = file.line_text(loc.line);
        let gutter = loc.line.to_string();
        let pad = " ".repeat(gutter.len());

        let mut out = String::new();
        let _ = writeln!(out, "{}: {}", d.severity.label(), d.message);
        let _ = writeln!(out, "{pad}--> {loc}");
        let _ = writeln!(out, "{pad} |");
        let _ = writeln!(out, "{gutter} | {line_text}");

        // Tabs in the source would otherwise put the caret in the wrong place,
        // so copy the line's own leading whitespace into the caret line.
        let indent: String = line_text
            .chars()
            .take((loc.column - 1) as usize)
            .map(|c| if c == '\t' { '\t' } else { ' ' })
            .collect();

        // The span may run past the end of the line (an unterminated string
        // reaching end of file); clamp so the caret stays under real text, and
        // never draw fewer than one caret.
        let line_chars = line_text.chars().count() as u32;
        let remaining = line_chars.saturating_sub(loc.column - 1);
        let width = d.span.len().min(remaining).max(1) as usize;

        let _ = write!(out, "{pad} | {indent}{}", "^".repeat(width));
        match &d.caret_label {
            Some(label) => {
                let _ = writeln!(out, " {label}");
            }
            None => out.push('\n'),
        }
        for note in &d.notes {
            let _ = writeln!(out, "{pad} = note: {note}");
        }
        for (span, note) in &d.related {
            let _ = writeln!(out, "{pad} = note: {note} at {}", self.location(*span));
        }
        out
    }

    /// Render every diagnostic, in order, separated by blank lines.
    pub fn render_all(&self, diagnostics: &[Diagnostic]) -> String {
        diagnostics
            .iter()
            .map(|d| self.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
