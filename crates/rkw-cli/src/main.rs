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
  --pc ADDR           where to start; defaults to the lowest address loaded
  --sp ADDR           initial stack pointer (default $FF00)
  --limit T           T-states one run may take before handing back, 0 for none
  -x, --script FILE   run a file of commands before the prompt (repeatable)
  --batch             run the scripts and exit, without a prompt
  -h, --help          this

Addresses are $hex, 0xhex, %binary or decimal.";

/// Somewhere well clear of a 48K screen and its system variables, and where
/// the debugger's own tests put it.
const DEFAULT_SP: u16 = 0xFF00;

struct Options {
    source: Option<PathBuf>,
    binaries: Vec<(u16, PathBuf)>,
    scripts: Vec<PathBuf>,
    pc: Option<u16>,
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

fn parse_args() -> Result<Option<Options>, String> {
    let mut options = Options {
        source: None,
        binaries: Vec::new(),
        scripts: Vec::new(),
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
            "--pc" => {
                let text = value("--pc")?;
                options.pc = Some(load::number(&text).ok_or(format!("{text} is not an address"))?);
            }
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

    let entry = options
        .pc
        .or_else(|| programs.iter().filter_map(|p| p.entry).min())
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
