//! `rkw`: the Spectrum in a window.
//!
//! Argument parsing, a machine built from what it found, and the event loop.
//! Everything else is in [`rkw_gui`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rkw_asm::{SourceMap, assemble};
use rkw_gui::Session;
use rkw_gui::app;
use rkw_spectrum::Spectrum;
use rkw_tape::{Image, Tap, Tzx};
use z80::Cpu;

const USAGE: &str = "\
rkw — a 48K ZX Spectrum in a window

usage: rkw [options]

  --rom FILE          the 48K ROM to boot. Without one the machine runs from
                      an empty memory, which is a black screen and a lesson.
  --asm FILE          assemble a source file into memory and run it, instead
                      of booting. For a game that is not on a tape yet.
  --tape FILE         mount a .tap or .tzx. Type LOAD \"\" and press F6.
  --play              start the tape immediately, for a machine already
                      sitting in a loading loop
  --scale N           window size, in machine pixels per screen pixel (3)
  --fullscreen        start full screen
  -h, --help          this

While it is running:

  F5   pause or resume         F9   mute or unmute
  F6   tape: play or stop      F10  reset
  F7   tape: rewind            F11  full screen
  F8   speed: 1x, 2x, max      F4   quit

Every other key is the Spectrum's. ESCAPE is BREAK, not a way out; CTRL and
ALT are both SYMBOL SHIFT, and the cursor keys are CAPS SHIFT and 5 to 8.";

struct Options {
    rom: Option<PathBuf>,
    asm: Option<PathBuf>,
    tape: Option<PathBuf>,
    play: bool,
    scale: u32,
    fullscreen: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            rom: None,
            asm: None,
            tape: None,
            play: false,
            // Three is 1056x888, which fits on any screen this will run on and
            // is big enough to read 32-column text.
            scale: 3,
            fullscreen: false,
        }
    }
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkw: {message}");
            return ExitCode::FAILURE;
        }
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkw: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--rom" => options.rom = Some(PathBuf::from(value("--rom")?)),
            "--asm" => options.asm = Some(PathBuf::from(value("--asm")?)),
            "--tape" => options.tape = Some(PathBuf::from(value("--tape")?)),
            "--play" => options.play = true,
            "--fullscreen" => options.fullscreen = true,
            "--scale" => {
                let text = value("--scale")?;
                options.scale = text
                    .parse()
                    .map_err(|_| format!("{text} is not a scale"))
                    .and_then(|n: u32| match n {
                        1..=16 => Ok(n),
                        _ => Err(format!("{n} is not a scale between 1 and 16")),
                    })?;
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(Some(options))
}

fn run(options: Options) -> Result<(), String> {
    let mut spectrum = match &options.rom {
        Some(path) => {
            let rom = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            Spectrum::with_rom(&rom).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => Spectrum::new(),
    };
    if let Some(path) = &options.tape {
        spectrum.mount_tape(tape(path)?);
        if options.play {
            spectrum.play_tape();
        }
    }

    // A source file is assembled into the machine and started at its own
    // origin, which is what a game that has no tape yet needs.
    let mut cpu = Cpu::new();
    if let Some(path) = &options.asm {
        cpu.regs.pc = assemble_into(&mut spectrum, path)?;
        cpu.regs.sp = STACK_UNTIL_TOLD_OTHERWISE;
    }

    let (session, no_sound) = Session::starting_at(spectrum, cpu);
    // Said once and not fatal: a machine with a picture and no sound is worth
    // more than an error message.
    if let Some(e) = no_sound {
        eprintln!("rkw: no sound ({e}); running silent");
    }
    app::run(session, options.scale, options.fullscreen).map_err(|e| e.to_string())
}

/// Where the stack goes until the program says otherwise, which every program
/// that sets `SP` in its first instruction does.
const STACK_UNTIL_TOLD_OTHERWISE: u16 = 0xFF00;

/// Assemble `path` into `spectrum`, and answer with where to start.
///
/// The whole of it: there is no debug information to keep, because the window
/// has no breakpoints to hang off it. `rkwdbg` is where a source file goes to
/// be looked at; this is where it goes to be played.
fn assemble_into(spectrum: &mut Spectrum, path: &Path) -> Result<u16, String> {
    let mut map = SourceMap::new();
    let file = map
        .load(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let assembled = assemble(&mut map, file);
    if assembled.has_errors() {
        return Err(assembled
            .diagnostics
            .iter()
            .map(|d| map.render(d))
            .collect::<String>());
    }
    for segment in assembled.image.segments() {
        spectrum.memory.load(segment.origin, &segment.bytes);
    }
    assembled
        .image
        .origin()
        .ok_or_else(|| format!("{}: nothing was assembled", path.display()))
}

/// A tape, read as whatever its extension says it is — and, failing that, as
/// whichever of the two parsers will have it. A file called `.dat` that a
/// friend sent you is far more often a tape than a mistake.
fn tape(path: &Path) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "tzx" => Tzx::parse(&bytes)
            .map(Image::from)
            .map_err(|e| format!("{}: {e}", path.display())),
        "tap" => Tap::parse(&bytes)
            .map(Image::from)
            .map_err(|e| format!("{}: {e}", path.display())),
        _ => Tzx::parse(&bytes)
            .map(Image::from)
            .or_else(|_| Tap::parse(&bytes).map(Image::from))
            .map_err(|e| format!("{}: not a tape: {e}", path.display())),
    }
}
