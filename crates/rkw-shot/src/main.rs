//! `rkwshot`: assemble a program, run it for a while, and write out what it
//! drew.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rkw_shot::{Rig, png};
use rkw_spectrum::{Framebuffer, Key, Keyboard};

/// A key by the name someone would type at a shell.
fn key_named(name: &str) -> Option<Key> {
    let named = match name {
        "space" => Key::Space,
        "enter" => Key::Enter,
        "caps" => Key::CapsShift,
        "sym" => Key::SymbolShift,
        _ => {
            // Everything else is a letter or a digit, and `Key` spells those
            // the way its own Debug does.
            let wanted = name.to_ascii_uppercase();
            let wanted = match wanted.as_str() {
                digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
                    format!("Num{digit}")
                }
                letter => letter.to_string(),
            };
            return Key::ALL
                .iter()
                .copied()
                .find(|key| format!("{key:?}") == wanted);
        }
    };
    Some(named)
}

const USAGE: &str = "\
rkwshot — run a program headless and photograph the frames

usage: rkwshot [options] FILE.asm

  --frames N        frames to run (default 100)
  --out DIR         where the PNGs go (default: no PNGs, just the profile)
  --every N         write a PNG every N frames (default 25)
  --skip N          run N frames before capturing anything, to get past setup
  --keys K,K,...    hold these keys down for the whole run, by name:
                    a-z, 0-9, space, enter, caps, sym
  --sheet FILE      tile every captured frame into one PNG, to see motion
  --columns N       frames across the sheet (default 4)
  --profile         print the border-colour time profile of each frame written
  -h, --help        this

The profile reads the border stripes a program leaves: a routine that sets the
border on the way in and out is measured by how many scanlines it coloured.";

struct Options {
    source: PathBuf,
    frames: u64,
    out: Option<PathBuf>,
    every: u64,
    skip: u64,
    sheet: Option<PathBuf>,
    columns: usize,
    keys: Vec<Key>,
    profile: bool,
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkwshot: {message}");
            return ExitCode::FAILURE;
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprint!("rkwshot: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut source = None;
    let mut frames = 100;
    let mut out = None;
    let mut every = 25;
    let mut skip = 0;
    let mut sheet = None;
    let mut columns = 4;
    let mut keys = Vec::new();
    let mut profile = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--frames" => {
                let text = value("--frames")?;
                frames = text
                    .parse()
                    .map_err(|_| format!("{text} is not a number"))?;
            }
            "--every" => {
                let text = value("--every")?;
                every = text
                    .parse::<u64>()
                    .map_err(|_| format!("{text} is not a number"))?
                    .max(1);
            }
            "--skip" => {
                let text = value("--skip")?;
                skip = text
                    .parse()
                    .map_err(|_| format!("{text} is not a number"))?;
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--sheet" => sheet = Some(PathBuf::from(value("--sheet")?)),
            "--columns" => {
                let text = value("--columns")?;
                columns = text
                    .parse::<usize>()
                    .map_err(|_| format!("{text} is not a number"))?
                    .max(1);
            }
            "--keys" => {
                for name in value("--keys")?.split(',') {
                    let key = key_named(name.trim())
                        .ok_or_else(|| format!("{name} is not a key on this machine"))?;
                    keys.push(key);
                }
            }
            "--profile" => profile = true,
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other if source.is_none() => source = Some(PathBuf::from(other)),
            other => return Err(format!("more than one source file: {other}")),
        }
    }

    let source = source.ok_or("no source file. Try --help".to_string())?;
    Ok(Some(Options {
        source,
        frames,
        out,
        every,
        skip,
        sheet,
        columns,
        keys,
        profile,
    }))
}

fn run(options: &Options) -> Result<(), String> {
    let mut rig = Rig::assemble(&options.source)?;
    if let Some(dir) = &options.out {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}\n", dir.display()))?;
    }

    if !options.keys.is_empty() {
        rig.machine.ula.keyboard = Keyboard::holding(&options.keys);
    }

    let stdout = std::io::stdout();
    let mut log = stdout.lock();
    let mut captured: Vec<Framebuffer> = Vec::new();
    if options.skip > 0 {
        rig.run_frames(options.skip);
    }
    for frame in 1..=options.frames {
        rig.run_frames(1);
        if frame % options.every != 0 && frame != options.frames {
            continue;
        }
        if let Some(dir) = &options.out {
            let path = dir.join(format!("frame-{frame:04}.png"));
            write_frame(&rig, &path)?;
            let _ = writeln!(log, "{}", path.display());
        }
        if options.sheet.is_some() {
            captured.push(rig.frame());
        }
        if options.profile {
            let _ = write!(log, "frame {frame}:\n{}", rig.profile().report());
        }
    }

    if let Some(path) = &options.sheet {
        write_sheet(&captured, options.columns, path)?;
        let _ = writeln!(log, "{}", path.display());
    }
    Ok(())
}

/// Every captured frame in one picture, in reading order.
///
/// A single frame says what the screen looked like; a sheet says what moved,
/// which is the only way to see a scroll in something that does not animate.
fn write_sheet(frames: &[Framebuffer], columns: usize, path: &Path) -> Result<(), String> {
    let first = frames.first().ok_or("no frames to make a sheet from\n")?;
    let (fw, fh) = (first.width(), first.height());
    let columns = columns.min(frames.len()).max(1);
    let rows = frames.len().div_ceil(columns);

    let (width, height) = (columns * fw, rows * fh);
    let mut rgb = vec![0u8; width * height * 3];
    for (index, frame) in frames.iter().enumerate() {
        let (cx, cy) = ((index % columns) * fw, (index / columns) * fh);
        let pixels = frame.to_rgb();
        for y in 0..fh {
            let from = y * fw * 3;
            let to = ((cy + y) * width + cx) * 3;
            rgb[to..to + fw * 3].copy_from_slice(&pixels[from..from + fw * 3]);
        }
    }
    std::fs::write(path, png::encode_rgb(width, height, &rgb))
        .map_err(|e| format!("{}: {e}\n", path.display()))
}

fn write_frame(rig: &Rig, path: &Path) -> Result<(), String> {
    let frame = rig.frame();
    let png = png::encode_rgb(frame.width(), frame.height(), &frame.to_rgb());
    std::fs::write(path, png).map_err(|e| format!("{}: {e}\n", path.display()))
}
