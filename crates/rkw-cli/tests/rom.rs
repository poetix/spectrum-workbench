//! `--rom`: the debugger with a Spectrum under it rather than a flat 64K.
//!
//! What the boot itself does is `rkw_spectrum`'s test to own — this one is
//! about the wiring, which is that a session, a shell and every command in them
//! work over a machine with hardware in it without knowing that they do.
//!
//! Skipped when the ROM has not been fetched; see `scripts/fetch-rom.sh`.

use std::path::PathBuf;

use rkw_cli::Shell;
use rkw_cli::load;
use rkw_debug::cmd::Session;
use rkw_debug::emu::Config;
use rkw_spectrum::Spectrum;
use z80::Cpu;

/// The fixture `rkw-spectrum`'s own tests use. Shared rather than fetched twice:
/// it is the same image, and one copy is one thing to keep out of the tree.
fn rom_path() -> PathBuf {
    match std::env::var_os("RKW_48K_ROM") {
        Some(path) => PathBuf::from(path),
        None => {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rkw-spectrum/tests/fixtures/48.rom")
        }
    }
}

/// T-states to give the ROM to boot: 150 frames, as in the boot test.
const BOOT_T: u64 = 150 * 69_888;

fn booted() -> Option<Shell<Spectrum>> {
    if !rom_path().is_file() {
        eprintln!(
            "skipping: no 48K ROM at {}\nrun scripts/fetch-rom.sh to install one",
            rom_path().display()
        );
        return None;
    }
    let (machine, loaded) = load::rom_file(&rom_path()).expect("the fixture is a 48K ROM");
    assert_eq!(loaded.origin, 0x0000);
    assert_eq!(loaded.len, 16_384);

    let mut cpu = Cpu::new();
    cpu.reset();
    let mut session = Session::new(cpu, machine, Config::default());
    session.set_run_limit(Some(BOOT_T));
    Some(Shell::new(session))
}

fn script(shell: &mut Shell<Spectrum>, text: &str) -> String {
    let mut out = Vec::new();
    shell.script(text, &mut out).expect("writing to a Vec");
    String::from_utf8(out).expect("the formatter writes UTF-8")
}

#[test]
fn the_debugger_boots_the_rom_and_stops_where_it_is_told() {
    let Some(mut shell) = booted() else { return };

    // A breakpoint in the ROM: `KEY-SCAN`, which nothing reaches until the
    // interrupt handler is running, so stopping there is proof that the frame
    // interrupt arrived and was taken.
    let text = script(&mut shell, "break $028E\ncontinue\n");
    assert!(text.contains("028E"), "{text}");
    assert_eq!(shell.errors(), 0, "{text}");
    assert_eq!(shell.session().regs().pc, 0x028E);
    // Inside the handler, with interrupts disabled until it returns.
    assert!(!shell.session().regs().iff1);
}

#[test]
fn the_machine_under_the_session_is_a_spectrum() {
    let Some(mut shell) = booted() else { return };
    let text = script(&mut shell, "continue\n");
    assert_eq!(shell.errors(), 0, "{text}");

    let machine = shell.session().machine();
    // The ROM got far enough to set the border white and to write its own
    // system variables, which is the boot proper.
    assert_eq!(machine.ula.border(), 7);
    assert_eq!(machine.memory.read(0x5C36), 0x00, "CHARS low byte");
    assert_eq!(machine.memory.read(0x5C37), 0x3C, "CHARS high byte");
    assert!(machine.ula.frames() > 100, "the frame clock did not run");
    // The ROM is still the ROM: a million instructions of it have run, and the
    // map refused every write any of them made below $4000.
    assert_eq!(
        machine.memory.read(0x0000),
        0xF3,
        "DI, the first instruction"
    );
}
