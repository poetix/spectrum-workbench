//! `rkwdbg`: the terminal front end.
//!
//! Loads something, makes a session, and hands both to the shell. Everything
//! interesting is in `rkw_debug::cmd` and in [`rkw_cli`]; this file is argument
//! parsing and the exit code.

use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use rkw_cli::Shell;
use rkw_cli::load::{self, LoadError, Program};
use rkw_debug::cmd::Session;
use rkw_debug::emu::Config;
use z80::{Cpu, FlatMemory};

const USAGE: &str = "\
rkwdbg — a gdb-style debugger for the Z80 core

usage: rkwdbg [options] [FILE.asm]

  FILE.asm            assemble and load a source file
  --load ADDR=FILE    load raw bytes at an address (repeatable)
  --debug FILE        read a .rkwdbg sidecar, for a binary assembled elsewhere
  --pc ADDR           where to start; defaults to the lowest address loaded
  --sp ADDR           initial stack pointer (default $FF00)
  --limit T           T-states one run may take before handing back, 0 for none
  -x, --script FILE   run a file of commands before the prompt (repeatable)
  --batch             run the scripts and exit, without a prompt
  -h, --help          this

Addresses are $hex, 0xhex, %binary or decimal. Where there is debug
information, --pc also takes a label.

A source file assembled here brings its own debug information. A binary brings
whatever `FILE.rkwdbg` sits beside it, unless --debug says otherwise.";

/// Somewhere well clear of a 48K screen and its system variables, and where
/// the debugger's own tests put it.
const DEFAULT_SP: u16 = 0xFF00;

struct Options {
    source: Option<PathBuf>,
    binaries: Vec<(u16, PathBuf)>,
    scripts: Vec<PathBuf>,
    debug: Option<PathBuf>,
    /// Kept as text because it may be a label, and what a label is worth is
    /// not known until the debug information has been read.
    pc: Option<String>,
    sp: u16,
    limit: Option<u64>,
    batch: bool,
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkwdbg: {message}");
            return ExitCode::FAILURE;
        }
    };
    match run(options) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rkwdbg: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The debug information a program brought with it: what was assembled here,
/// or the sidecar sitting beside a binary.
fn sources_of(
    programs: &[Program],
    options: &Options,
) -> Result<Option<rkw_dbginfo::Sources>, LoadError> {
    if let Some(sources) = programs.iter().find_map(|p| p.sources.clone()) {
        return Ok(Some(sources));
    }
    for (_, path) in &options.binaries {
        let sidecar = load::sidecar_of(path);
        if sidecar.is_file() {
            return Ok(Some(load::debug_file(&sidecar)?));
        }
    }
    Ok(None)
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut options = Options {
        source: None,
        binaries: Vec::new(),
        scripts: Vec::new(),
        debug: None,
        pc: None,
        sp: DEFAULT_SP,
        limit: None,
        batch: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--load" => {
                let spec = value("--load")?;
                let (addr, path) = spec
                    .split_once('=')
                    .ok_or("--load wants ADDR=FILE".to_string())?;
                let addr = load::number(addr).ok_or(format!("{addr} is not an address"))?;
                options.binaries.push((addr, PathBuf::from(path)));
            }
            "-x" | "--script" => options.scripts.push(PathBuf::from(value("--script")?)),
            "--debug" => options.debug = Some(PathBuf::from(value("--debug")?)),
            "--pc" => options.pc = Some(value("--pc")?),
            "--sp" => {
                let text = value("--sp")?;
                options.sp = load::number(&text).ok_or(format!("{text} is not an address"))?;
            }
            "--limit" => {
                let text = value("--limit")?;
                let limit: u64 = text
                    .parse()
                    .map_err(|_| format!("{text} is not a number"))?;
                options.limit = Some(limit);
            }
            "--batch" => options.batch = true,
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other if options.source.is_none() => options.source = Some(PathBuf::from(other)),
            other => return Err(format!("more than one source file: {other}")),
        }
    }
    Ok(Some(options))
}

fn run(options: Options) -> Result<ExitCode, LoadError> {
    let mut mem = FlatMemory::new();
    let mut programs: Vec<Program> = Vec::new();
    if let Some(path) = &options.source {
        programs.push(load::assemble_file(&mut mem, path)?);
    }
    for (origin, path) in &options.binaries {
        programs.push(load::binary_file(&mut mem, path, *origin)?);
    }

    let sources = match &options.debug {
        Some(path) => Some(load::debug_file(path)?),
        None => sources_of(&programs, &options)?,
    };

    // A label is an address as far as everything downstream is concerned, but
    // it cannot be resolved until there is something to resolve it against.
    let entry = match (&options.pc, &sources) {
        (Some(text), _) if load::number(text).is_some() => load::number(text),
        (Some(text), Some(sources)) => {
            Some(sources.address_of(text).map_err(|e| LoadError::DebugInfo {
                path: "--pc".into(),
                message: e.to_string(),
            })?)
        }
        (Some(text), None) => {
            return Err(LoadError::DebugInfo {
                path: "--pc".into(),
                message: format!("{text} is not an address, and there is no debug information"),
            });
        }
        (None, _) => programs.iter().filter_map(|p| p.entry).min(),
    }
    .unwrap_or(0);

    let mut cpu = Cpu::new();
    cpu.regs.pc = entry;
    cpu.regs.sp = options.sp;

    let mut session = Session::new(cpu, mem, Config::default());
    if let Some(limit) = options.limit {
        // Zero means "no limit", which is right for someone who can interrupt
        // the process and wrong for a script.
        session.set_run_limit((limit > 0).then_some(limit));
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stale: Vec<String> = sources
        .iter()
        .flat_map(|s| s.stale())
        .map(str::to_string)
        .collect();
    if let Some(sources) = sources {
        session.set_sources(sources);
    }
    let mut shell = Shell::new(session);

    for program in &programs {
        if !program.notes.is_empty() {
            write!(out, "{}", program.notes)?;
        }
        for loaded in &program.loaded {
            writeln!(
                out,
                "Loaded {} bytes at ${:04X} from {}",
                loaded.len,
                loaded.origin,
                loaded.path.display()
            )?;
        }
    }
    // Said once, at the top, rather than at every stop: the addresses are
    // still the binary's, and it is the text beside them that has moved on.
    for file in &stale {
        writeln!(
            out,
            "warning: {file} has changed since the debug information was written"
        )?;
    }
    writeln!(out, "Entry point ${entry:04X}. `help` lists the commands.")?;

    let mut quit = false;
    for script in &options.scripts {
        if shell.source(script, &mut out)? == rkw_cli::Flow::Quit {
            quit = true;
            break;
        }
    }
    if !options.batch && !quit {
        let stdin = io::stdin();
        shell.repl(&mut BufReader::new(stdin.lock()), &mut out)?;
    }
    out.flush()?;

    // A script that reported an error is a failed test, and the exit code is
    // the only part of that a build server reads.
    Ok(if shell.errors() > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
