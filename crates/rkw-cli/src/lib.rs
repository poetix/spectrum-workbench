//! The shell: a read loop, an output stream, and files of commands.
//!
//! Everything a command *means* is in `rkw_debug::cmd`; what is here is the
//! part that could not be tested without a terminal if it were mixed in with
//! the rest — reading lines from somewhere, writing text to somewhere, and
//! following a `source` to another file. Keeping it this thin is what ADR-0013
//! means by "the REPL is a thin shell over the command layer", and it is why
//! the tests in this crate can run a whole session against a `&str` and assert
//! on a `Vec<u8>`.
//!
//! ```
//! use rkw_cli::Shell;
//! use rkw_debug::cmd::Session;
//! use rkw_debug::emu::Config;
//! use z80::{Cpu, FlatMemory};
//!
//! let mut mem = FlatMemory::new();
//! mem.load(0x8000, &[0x3E, 0x2A, 0x76]); // LD A,42 ; HALT
//! let mut cpu = Cpu::new();
//! cpu.regs.pc = 0x8000;
//!
//! let mut shell = Shell::new(Session::new(cpu, mem, Config::default()));
//! let mut out = Vec::new();
//! shell.script("continue\nregs\n", &mut out).unwrap();
//!
//! assert_eq!(shell.errors(), 0);
//! assert!(String::from_utf8(out).unwrap().contains("AF=2A"));
//! ```

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use rkw_debug::cmd::format::{parse_error, render};
use rkw_debug::cmd::{Error, Outcome, Session};
use rkw_debug::machine::Machine;

pub mod load;

/// What to do after a line: the only reason the shell has a return value at
/// all is `quit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// How deep `source` may nest. A file that sources itself is a mistake, and an
/// unbounded one is a stack overflow rather than a message.
pub const MAX_DEPTH: usize = 16;

pub const PROMPT: &str = "(rkw) ";

pub struct Shell<M: Machine> {
    session: Session<M>,
    /// Lines are echoed when they came from a file, so that the output of a
    /// script says what produced it. Interactive input is already on screen.
    echo: bool,
    depth: usize,
    errors: usize,
    /// The last command typed, so an empty line repeats it as gdb's does.
    last: Option<String>,
}

impl<M: Machine> Shell<M> {
    pub fn new(session: Session<M>) -> Shell<M> {
        Shell {
            session,
            echo: false,
            depth: 0,
            errors: 0,
            last: None,
        }
    }

    pub fn session(&self) -> &Session<M> {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session<M> {
        &mut self.session
    }

    /// How many commands have failed, over the shell's whole life. A batch run
    /// exits non-zero on this, which is what makes a script a test.
    pub fn errors(&self) -> usize {
        self.errors
    }

    /// Run one line and write whatever it produced.
    pub fn line<W: Write>(&mut self, line: &str, out: &mut W) -> io::Result<Flow> {
        if self.echo {
            writeln!(out, "{PROMPT}{}", line.trim_end())?;
        }
        match self.session.run_line(line) {
            Ok(None) => Ok(Flow::Continue),
            Ok(Some(Outcome::Quit)) => Ok(Flow::Quit),
            // The executor hands scripts back rather than reading them, so
            // this is where a file becomes lines.
            Ok(Some(Outcome::Script(path))) => self.source(Path::new(&path), out),
            Ok(Some(outcome)) => {
                let text = render(&outcome);
                if !text.is_empty() {
                    writeln!(out, "{text}")?;
                }
                Ok(Flow::Continue)
            }
            Err(error) => {
                self.errors += 1;
                match error {
                    Error::Parse(e) => writeln!(out, "{}", parse_error(line, &e))?,
                    Error::Exec(e) => writeln!(out, "error: {e}")?,
                }
                Ok(Flow::Continue)
            }
        }
    }

    /// Run every line of some text. Used for `-x` scripts and for the bodies
    /// of `source`.
    pub fn script<W: Write>(&mut self, text: &str, out: &mut W) -> io::Result<Flow> {
        for line in text.lines() {
            if self.line(line, out)? == Flow::Quit {
                return Ok(Flow::Quit);
            }
        }
        Ok(Flow::Continue)
    }

    /// Read a file and run it, echoing as it goes.
    ///
    /// A file that cannot be read is an error the shell counts rather than one
    /// that ends the session: in a REPL the answer is to fix the path and try
    /// again, and in a batch run the exit code has already been decided.
    pub fn source<W: Write>(&mut self, path: &Path, out: &mut W) -> io::Result<Flow> {
        if self.depth >= MAX_DEPTH {
            self.errors += 1;
            writeln!(
                out,
                "error: {} is more than {MAX_DEPTH} files deep; is it sourcing itself?",
                path.display()
            )?;
            return Ok(Flow::Continue);
        }
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                self.errors += 1;
                writeln!(out, "error: {}: {e}", path.display())?;
                return Ok(Flow::Continue);
            }
        };
        let was_echoing = std::mem::replace(&mut self.echo, true);
        self.depth += 1;
        let flow = self.script(&text, out);
        self.depth -= 1;
        self.echo = was_echoing;
        flow
    }

    /// The read loop. Prompts, runs, and repeats the last command on an empty
    /// line, which is what makes stepping bearable.
    pub fn repl<R: BufRead, W: Write>(&mut self, input: &mut R, out: &mut W) -> io::Result<()> {
        loop {
            write!(out, "{PROMPT}")?;
            out.flush()?;
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                writeln!(out)?;
                return Ok(());
            }
            let line = match (line.trim(), &self.last) {
                ("", Some(last)) => last.clone(),
                ("", None) => continue,
                (typed, _) => {
                    self.last = Some(typed.to_string());
                    typed.to_string()
                }
            };
            if self.line(&line, out)? == Flow::Quit {
                return Ok(());
            }
        }
    }
}

/// Where a session's memory came from, for the banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub path: PathBuf,
    pub origin: u16,
    pub len: usize,
}
