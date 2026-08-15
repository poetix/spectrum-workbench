//! raxoft's `z80test`, run the way a person would run it: boot the ROM, put the
//! tape in, type `LOAD ""`, and read the answer off the screen.
//!
//! This is the only test in the tree that exercises the CPU, the ULA, the
//! keyboard, the screen, the tape and the ROM at once, and it is the only one
//! whose expected values came off real hardware rather than out of another
//! emulator. Each of the five suites runs 160 instruction groups, CRCs the
//! registers and flags each one leaves behind, and compares against a table
//! Patrik Rak captured from a 48K Spectrum with a Zilog Z80 in it.
//!
//! # Why it asserts on one line of the screen
//!
//! The suite prints a line per group as it goes and a summary at the end:
//!
//! ```text
//!   Result: 000 of 160 tests failed.
//! ```
//!
//! That summary is what is asserted on. The per-group lines are printed to the
//! upper screen and scroll, so reading them means catching the screen between
//! two scrolls, and a line caught mid-print comes back with `?` cells in it
//! where a character is half drawn. The summary is the last thing printed and
//! is still there when the program returns to BASIC, so it is the one thing
//! that can be read without racing anything.
//!
//! # Loading
//!
//! Both ways in are here, and they answer different questions. The waveform is
//! the real thing — 4,000-odd frames of the ROM measuring edges, which is the
//! whole of ticket 0016 under load — and it is what `loads_off_the_waveform`
//! covers, once, on the smallest suite. The other four use the `LD-BYTES` trap,
//! which returns the block in the T-state it was asked for. Running all five
//! through the waveform would add about a minute of loading to prove five times
//! over what one test proves once.
//!
//! # Cost
//!
//! `z80full` is a little over an hour of emulated time. These are `#[ignore]`d
//! for that reason, not because they are unreliable.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{Board, rom, skip_message};
use rkw_debug::machine::Machine;
use rkw_spectrum::frame::T_STATES_PER_FRAME;
use rkw_spectrum::{Key, LD_BYTES, ld_bytes};
use rkw_tape::Tap;

/// Frames to let the ROM boot before it will believe a keystroke. The copyright
/// message is up long before this; what takes the time is the ROM settling into
/// its main loop with a `K` cursor.
const BOOT_FRAMES: u64 = 150;

/// How often the screen is examined while the suite runs.
///
/// Two things are read off it and they want different rates. The summary is
/// read once and would tolerate any interval; the names of the failing groups
/// scroll past, and a poll that missed one would report a count with nothing to
/// go with it. A group takes a few dozen frames, so this is comfortably inside
/// the window in which each line is on the screen.
const POLL_FRAMES: u64 = 25;

/// Frames of emulated time before a suite is called hung. `z80full` takes about
/// 200,000, which is 66 minutes of emulated time and a quarter of a minute of
/// real time.
const FRAME_BUDGET: u64 = 400_000;

/// Groups in every suite. Asserted rather than assumed: a suite that reported
/// "0 of 12" would otherwise pass on having run almost nothing.
const GROUPS: u32 = 160;

/// How the program gets into memory.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Load {
    /// Play the tape at the ROM and let it measure the edges, as a real machine
    /// does.
    Waveform,
    /// Answer the ROM's `LD-BYTES` from the image directly.
    Trap,
}

fn tape_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/z80test")
        .join(format!("{name}.tap"))
}

/// The tape, or `None` if nobody has fetched it.
fn tape(name: &str) -> Option<Tap> {
    let bytes = std::fs::read(tape_path(name)).ok()?;
    Some(Tap::parse(&bytes).expect("z80test tapes are well-formed TAP files"))
}

fn tape_skip_message(name: &str) -> String {
    format!(
        "skipping: no z80test tape at {}\n\
         run scripts/fetch-testdata.sh to install one",
        tape_path(name).display()
    )
}

/// Run to `target`, answering `LD-BYTES` out of the tape image when the ROM
/// calls it.
///
/// The trap is checked before the step rather than after, because it stands in
/// for the routine at that address: by the time the instruction there has run,
/// the ROM is already measuring a pulse that is not coming.
fn run_to_trapping(board: &mut Board, target: u64) {
    while board.machine.t_states() < target {
        let event = board
            .machine
            .next_event()
            .expect("the ULA schedules frames");
        let deadline = event.min(target);
        while board.machine.t_states() < deadline {
            if board.cpu.regs.pc == LD_BYTES {
                ld_bytes(&mut board.cpu, &mut board.machine);
            }
            board.cpu.step(&mut board.machine);
        }
        if board.machine.t_states() >= event {
            board.machine.service_event();
        }
    }
}

fn run_frames(board: &mut Board, load: Load, frames: u64) {
    let target = board.machine.t_states() + frames * T_STATES_PER_FRAME;
    match load {
        Load::Waveform => board.run_to(target),
        Load::Trap => run_to_trapping(board, target),
    }
}

/// Hold `keys` down, then let go — [`Board::press`] with the trap installed.
///
/// It has to be spelled again here rather than reused, because the trap has to
/// be in place *while the keys are being typed*. `LOAD ""` starts running on
/// the ENTER that ends it, so a run that only installed the trap afterwards
/// would find the ROM already inside `LD-BYTES`, measuring the edges of a tape
/// that is not playing, and would sit there until the frame budget ran out.
fn press(board: &mut Board, load: Load, keys: &[Key]) {
    for key in keys {
        board.machine.ula.keyboard.press(*key);
    }
    run_frames(board, load, common::HOLD_FRAMES);
    for key in keys {
        board.machine.ula.keyboard.release(*key);
    }
    run_frames(board, load, common::GAP_FRAMES);
}

fn type_text(board: &mut Board, load: Load, text: &str) {
    for c in text.chars() {
        press(board, load, &common::keys_for(c));
    }
}

/// What the suite printed, and what it means.
#[derive(Debug, PartialEq, Eq)]
struct Report {
    failed: u32,
    total: u32,
    /// The names of the groups that failed, in the order they were printed.
    /// Collected as they go past rather than read at the end, because by then
    /// they have scrolled off — and a count with no names is a failure report
    /// that says nothing about what to fix.
    failures: Vec<String>,
}

/// The suite's own summary line, if it has been printed yet.
///
/// `Result: 011 of 160 tests failed.` — or `Result: all tests passed.`, which
/// is the same thing with a zero in it.
fn summary(screen: &[String]) -> Option<(u32, u32)> {
    let line = screen.iter().find(|line| line.contains("Result:"))?;
    if line.contains("all tests passed.") {
        return Some((0, GROUPS));
    }
    let rest = line.split("Result:").nth(1)?.trim();
    let (failed, rest) = rest.split_once(" of ")?;
    let (total, tail) = rest.split_once(' ')?;
    // Only once the whole sentence is there: the count is printed a digit at a
    // time and a screen caught mid-print would parse to something arbitrary.
    if !tail.starts_with("tests failed.") {
        return None;
    }
    Some((failed.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// The group names on the screen that are marked FAILED.
///
/// The suite prints the name at the left of the line and the verdict hard
/// right, so a failure is a line ending in `FAILED` and the name is what is
/// left when the leading group number is taken off.
fn failures(screen: &[String]) -> Vec<String> {
    screen
        .iter()
        .filter(|line| line.ends_with("FAILED"))
        .filter_map(|line| {
            let name = line.trim_end_matches("FAILED").trim();
            let name = name.split_once(' ').map_or(name, |(_number, rest)| rest);
            // A `?` is a cell the scrape could not match against the ROM font,
            // which means the line was caught with a character half drawn. The
            // same line is read again whole on a later poll, so dropping it
            // loses nothing and keeps `INIR ?` out of the failure list.
            let name = name.trim();
            (!name.is_empty() && !name.contains('?')).then(|| name.to_string())
        })
        .collect()
}

/// Boot, load and run one suite. `None` when there is no ROM or no tape.
fn run_suite(name: &str, load: Load) -> Option<Report> {
    let rom = match rom() {
        Some(rom) => rom,
        None => {
            eprintln!("{}", skip_message());
            return None;
        }
    };
    let Some(tap) = tape(name) else {
        eprintln!("{}", tape_skip_message(name));
        return None;
    };

    let mut board = Board::new(&rom);
    board.run_frames(BOOT_FRAMES);
    board.machine.mount_tape(Arc::new(tap));

    // `J` is LOAD at the K cursor, and the two quotes are SYMBOL SHIFT + P.
    // The tape has to be running before the ENTER that starts the load.
    if load == Load::Waveform {
        board.machine.play_tape();
    }
    type_text(&mut board, load, "J\"\"\n");

    let mut seen: Vec<String> = Vec::new();
    let deadline = board.machine.t_states() + FRAME_BUDGET * T_STATES_PER_FRAME;
    while board.machine.t_states() < deadline {
        run_frames(&mut board, load, POLL_FRAMES);
        let screen = board.screen_text();

        for name in failures(&screen) {
            if !seen.contains(&name) {
                seen.push(name);
            }
        }

        if let Some((failed, total)) = summary(&screen) {
            return Some(Report {
                failed,
                total,
                failures: seen,
            });
        }

        // The ROM stops every screenful and waits for a key. ENTER, not SPACE:
        // SPACE at that prompt is BREAK, and the suite would stop where it
        // stood and report nothing.
        if screen.iter().any(|line| line.contains("scroll?")) {
            press(&mut board, load, &[Key::Enter]);
        }
    }

    panic!(
        "{name} did not finish within {FRAME_BUDGET} frames; screen was:\n{}",
        board.screen_text().join("\n")
    );
}

/// Run `name` and require every group to pass.
fn assert_suite_passes(name: &str, load: Load) {
    let Some(report) = run_suite(name, load) else {
        return;
    };
    assert_eq!(
        report.total, GROUPS,
        "{name} reported {} groups, not {GROUPS}",
        report.total
    );
    assert_eq!(
        report.failed,
        0,
        "{name}: {} of {} groups failed: {}",
        report.failed,
        report.total,
        if report.failures.is_empty() {
            "(names scrolled off the screen)".to_string()
        } else {
            report.failures.join(", ")
        }
    );
}

#[test]
#[ignore = "runs about an hour of emulated time; use --ignored"]
fn z80doc() {
    assert_suite_passes("z80doc", Load::Trap);
}

#[test]
#[ignore = "runs about an hour of emulated time; use --ignored"]
fn z80docflags() {
    assert_suite_passes("z80docflags", Load::Trap);
}

#[test]
#[ignore = "runs about an hour of emulated time; use --ignored"]
fn z80flags() {
    assert_suite_passes("z80flags", Load::Trap);
}

#[test]
#[ignore = "runs about an hour of emulated time; use --ignored"]
fn z80memptr() {
    assert_suite_passes("z80memptr", Load::Trap);
}

#[test]
#[ignore = "runs about an hour of emulated time; use --ignored"]
fn z80ccf() {
    assert_suite_passes("z80ccf", Load::Trap);
}

#[test]
#[ignore = "runs about an hour of emulated time; use --ignored"]
fn z80full() {
    assert_suite_passes("z80full", Load::Trap);
}

/// The same suite, loaded the way a real machine loads it: 4,000-odd frames of
/// the ROM timing edges off the tape rather than a trap that hands it the
/// block.
///
/// One suite is enough. What this covers that the others do not is the loader,
/// and the loader does not care which suite it is loading.
#[test]
#[ignore = "loads at real tape speed on top of the run; use --ignored"]
fn z80doc_off_the_waveform() {
    assert_suite_passes("z80doc", Load::Waveform);
}
