//! Keys arriving from a frontend, through the command ring, into the matrix.
//!
//! `tests/keyboard.rs` checks what an emulated program reads once the matrix
//! is set. This checks the other half: that a frontend on another thread can
//! set it at all, and that doing it through the command ring keeps the two
//! properties ADR-0024 claims for it — the machine sees the keypress at a
//! T-state it agrees with, and the keypress is in the command log, so a
//! recorded session replays with the typing in it.

use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu, RunState};
use rkw_debug::{Debugger, StopReason};
use rkw_spectrum::keyboard::{Key, half_row_port};
use rkw_spectrum::keymap::{HostKey, HostKeys, KeyMap};
use rkw_spectrum::{Keyboard, Spectrum};
use z80::Cpu;
use z80::disasm::Peek;

/// Let a frame boundary go by, then read the half-row `CAPS SHIFT` is on into
/// `A`, store it, and stop.
///
/// ```text
/// 8000  0E 16              ld c,22
/// 8002  06 00              ld b,0
/// 8004  10 FE              djnz $        ; 3323 T
/// 8006  0D                 dec c
/// 8007  20 F9              jr nz,$-7
/// 8009  01 FE FE           ld bc,$FEFE
/// 800C  ED 78              in a,(c)
/// 800E  32 00 90           ld ($9000),a
/// 8011  76                 halt
/// ```
///
/// The delay is the point, and 22 turns of it is 73,607 T-states — one frame
/// and a bit. A matrix handed over by a frontend is latched and applied at the
/// top of the next frame ([`rkw_spectrum::Ula::set_keyboard`]), so a program
/// that read the keyboard the instant the command arrived would read the
/// matrix from before it. A real program does not do that either: it scans
/// from the interrupt handler, which is to say just after a frame boundary.
#[rustfmt::skip]
const READ_HALF_ROW_NEXT_FRAME: &[u8] = &[
    0x0E, 0x16,
    0x06, 0x00,
    0x10, 0xFE,
    0x0D,
    0x20, 0xF9,
    0x01, 0xFE, 0xFE,
    0xED, 0x78,
    0x32, 0x00, 0x90,
    0x76,
];

fn machine() -> (Cpu, Spectrum) {
    let mut spectrum = Spectrum::new();
    spectrum.memory.load(0x8000, READ_HALF_ROW_NEXT_FRAME);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    (cpu, spectrum)
}

fn config() -> Config {
    Config {
        log_capacity: 64,
        ..Config::default()
    }
}

/// The matrix a frontend would send for these host keys.
fn matrix_for(keys: &[HostKey]) -> u64 {
    let mut host = HostKeys::new();
    for key in keys {
        host.press(*key);
    }
    host.matrix(&KeyMap::PC).matrix()
}

#[test]
fn a_matrix_sent_as_a_command_is_what_the_program_reads() {
    let (cpu, spectrum) = machine();
    let (mut emu, mut handle) = Emu::new(cpu, spectrum, Debugger::new(), config());

    // `Z` is bit 1 of the half-row `0xFEFE` selects, and a pressed key reads
    // as a *low* bit.
    handle
        .send(Command::Keys(matrix_for(&[HostKey::Char('z')])))
        .unwrap();
    handle.send(Command::Resume).unwrap();
    while emu.slice() == RunState::Running {}

    assert_eq!(emu.stop_reason(), Some(StopReason::Halted));
    assert_eq!(emu.machine.peek(0x9000) & 0x1F, 0b1_1101);
    assert!(emu.machine.ula.keyboard.is_pressed(Key::Z));
}

#[test]
fn a_matrix_of_zero_lets_every_key_up() {
    let (cpu, spectrum) = machine();
    let (mut emu, mut handle) = Emu::new(cpu, spectrum, Debugger::new(), config());
    emu.machine.ula.keyboard = Keyboard::holding(&[Key::CapsShift, Key::V]);

    // What the window sends when it loses focus.
    handle.send(Command::Keys(0)).unwrap();
    handle.send(Command::Resume).unwrap();
    while emu.slice() == RunState::Running {}

    assert!(!emu.machine.ula.keyboard.any_pressed());
    assert_eq!(emu.machine.peek(0x9000) & 0x1F, 0x1F);
}

/// Every half-row and every bit, so that a matrix packed one way and unpacked
/// another would show up here rather than as one key on one row typing the
/// wrong letter.
#[test]
fn every_key_in_the_matrix_survives_the_trip_through_a_command() {
    for key in Key::ALL {
        let (cpu, spectrum) = machine();
        let (mut emu, mut handle) = Emu::new(cpu, spectrum, Debugger::new(), config());

        let matrix = Keyboard::holding(&[key]).matrix();
        handle.send(Command::Keys(matrix)).unwrap();
        handle.send(Command::Resume).unwrap();
        while emu.slice() == RunState::Running {}

        assert!(
            emu.machine.ula.keyboard.is_pressed(key),
            "{key:?} did not arrive"
        );
        assert_eq!(
            emu.machine.ula.keyboard.read(half_row_port(key.half_row())) & key.mask(),
            0,
            "{key:?} arrived on the wrong bit"
        );
    }
}

/// The reason input goes through the command ring rather than a shared atomic:
/// a session with typing in it replays to the same machine.
#[test]
fn typing_is_in_the_command_log_and_replays() {
    let (cpu, spectrum) = machine();
    let (mut emu, mut handle) = Emu::new(cpu, spectrum, Debugger::new(), config());
    handle.send(Command::Resume).unwrap();

    // Type into a machine that is running, so the keys land at whatever
    // T-states the slices happen to reach — which is the point: the log
    // records those, and the replay uses them.
    let mut sent = 0;
    for keys in [
        vec![HostKey::Char('a')],
        vec![HostKey::Char('a'), HostKey::Shift],
        vec![],
        vec![HostKey::Space],
    ] {
        handle.send(Command::Keys(matrix_for(&keys))).unwrap();
        sent += 1;
        emu.slice();
    }
    while emu.slice() == RunState::Running {}

    let log: Vec<_> = emu.log().to_vec();
    let keys_logged = log
        .iter()
        .filter(|s| matches!(s.command, Command::Keys(_)))
        .count();
    assert_eq!(keys_logged, sent, "the log lost a keypress");

    let (cpu, spectrum) = machine();
    let (mut replayed, _handle) = Emu::new(cpu, spectrum, Debugger::new(), config());
    replayed.replay(&log, emu.machine.t_states());

    assert_eq!(replayed.cpu.regs, emu.cpu.regs);
    assert_eq!(replayed.machine.peek(0x9000), emu.machine.peek(0x9000));
    assert_eq!(
        replayed.machine.ula.keyboard, emu.machine.ula.keyboard,
        "the replayed machine is holding different keys"
    );
}
