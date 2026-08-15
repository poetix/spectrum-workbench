//! The keyboard as the emulated program sees it: `IN` instructions, the ROM's
//! own scanning idioms, and the host key mapping driving all of it.
//!
//! The unit tests in `keyboard.rs` check the decode against the published
//! matrix. These check that the decode is reached — that the whole 16-bit
//! address arrives at the ULA rather than the `0xFE` in its low byte, which is
//! the difference between a keyboard that works and one that answers every
//! half-row with the same five bits.

use rkw_debug::machine::Machine;
use rkw_spectrum::keyboard::{HALF_ROWS, Key, half_row_port};
use rkw_spectrum::keymap::{HostKey, HostKeys, KeyMap};
use rkw_spectrum::{Keyboard, Spectrum};
use z80::Cpu;

/// Load `code` at `0x8000` and run it until it `HALT`s, or give up after
/// enough steps that a loop which was meant to end has clearly not.
fn run(machine: &mut Spectrum, code: &[u8]) -> Cpu {
    let mut cpu = Cpu::new();
    machine.memory.load(0x8000, code);
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    for _ in 0..10_000 {
        if machine.memory.read(cpu.regs.pc) == 0x76 {
            return cpu;
        }
        cpu.step(machine);
    }
    panic!("the program never reached its HALT");
}

/// The ROM's scan, in miniature: `B` walks a single zero bit down from `A8`,
/// and each `IN A,(C)` reads the half-row that bit selects.
///
/// ```text
/// 8000  21 00 90           ld hl,$9000
/// 8003  01 FE FE           ld bc,$FEFE
/// 8006  1E 08              ld e,8
/// 8008  ED 78      scan:   in a,(c)
/// 800A  77                 ld (hl),a
/// 800B  23                 inc hl
/// 800C  CB 00              rlc b
/// 800E  1D                 dec e
/// 800F  20 F7              jr nz,scan
/// 8011  76                 halt
/// ```
#[rustfmt::skip]
const SCAN: &[u8] = &[
    0x21, 0x00, 0x90,
    0x01, 0xFE, 0xFE,
    0x1E, 0x08,
    0xED, 0x78,
    0x77,
    0x23,
    0xCB, 0x00,
    0x1D,
    0x20, 0xF7,
    0x76,
];

/// The eight bytes [`SCAN`] leaves at `0x9000`, one per half-row.
fn scan(keyboard: Keyboard) -> [u8; HALF_ROWS] {
    let mut machine = Spectrum::new();
    machine.ula.keyboard = keyboard;
    run(&mut machine, SCAN);
    let mut rows = [0; HALF_ROWS];
    for (half_row, byte) in rows.iter_mut().enumerate() {
        *byte = machine.memory.read(0x9000 + half_row as u16);
    }
    rows
}

#[test]
fn each_half_row_answers_for_its_own_five_keys() {
    // Nothing down: five ones and the three bits the ULA fills in, eight times.
    assert_eq!(scan(Keyboard::new()), [0xFF; HALF_ROWS]);

    // One key on each half-row at a different bit, so a decode that returned
    // the same row eight times, or the rows in the wrong order, cannot pass.
    let keys = [
        Key::CapsShift,
        Key::S,
        Key::E,
        Key::Num4,
        Key::Num6,
        Key::P,
        Key::L,
        Key::M,
    ];
    let rows = scan(Keyboard::holding(&keys));
    for (half_row, key) in keys.iter().enumerate() {
        assert_eq!(key.half_row(), half_row, "the test's own table");
        assert_eq!(
            rows[half_row],
            0xFF & !key.mask(),
            "{key:?} on half-row {half_row}"
        );
    }
}

#[test]
fn a_shifted_key_shows_up_on_both_of_its_half_rows() {
    // CAPS SHIFT and 6 together, which is what the ROM reads as cursor down —
    // and the thing a keyboard that can only report one key at a time gets
    // wrong.
    let rows = scan(Keyboard::holding(&[Key::CapsShift, Key::Num6]));
    assert_eq!(rows[Key::CapsShift.half_row()], 0xFE);
    assert_eq!(rows[Key::Num6.half_row()], 0xEF);
    for (half_row, byte) in rows.iter().enumerate() {
        if half_row != Key::CapsShift.half_row() && half_row != Key::Num6.half_row() {
            assert_eq!(*byte, 0xFF, "half-row {half_row}");
        }
    }
}

#[test]
fn in_a_n_puts_the_accumulator_on_the_high_byte() {
    // `IN A,($FE)` addresses the port with A, so `LD A,$7F` selects the SPACE
    // half-row exactly as `LD BC,$7FFE; IN A,(C)` does. Software really does
    // read the keyboard this way, and it only works if the whole address
    // reaches the ULA.
    let mut machine = Spectrum::new();
    machine.ula.keyboard.press(Key::Space);
    #[rustfmt::skip]
    let cpu = run(&mut machine, &[
        0x3E, 0x7F,        // ld a,$7F
        0xDB, 0xFE,        // in a,($FE)
        0x32, 0x00, 0x90,  // ld ($9000),a
        0x3E, 0xFE,        // ld a,$FE
        0xDB, 0xFE,        // in a,($FE)
        0x32, 0x01, 0x90,  // ld ($9001),a
        0x76,              // halt
    ]);
    assert_eq!(cpu.regs.a, 0xFF);
    assert_eq!(machine.memory.read(0x9000), 0xFF & !Key::Space.mask());
    assert_eq!(machine.memory.read(0x9001), 0xFF);
}

#[test]
fn a_program_waiting_for_any_key_waits_and_then_stops_waiting() {
    // `LD BC,$00FE` drives all eight lines low at once, which asks "is
    // anything down at all" — how the ROM's pause loop and the start of
    // KEY-SCAN both work.
    //
    // ```text
    // 8000  01 FE 00   wait:  ld bc,$00FE
    // 8003  ED 78             in a,(c)
    // 8005  E6 1F             and $1F
    // 8007  FE 1F             cp $1F
    // 8009  28 F5             jr z,wait
    // 800B  76                halt
    // ```
    #[rustfmt::skip]
    const WAIT: &[u8] = &[
        0x01, 0xFE, 0x00,
        0xED, 0x78,
        0xE6, 0x1F,
        0xFE, 0x1F,
        0x28, 0xF5,
        0x76,
    ];

    let mut machine = Spectrum::new();
    let mut cpu = Cpu::new();
    machine.memory.load(0x8000, WAIT);
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;

    // Nothing held: it goes round for as long as it is left alone.
    for _ in 0..1_000 {
        cpu.step(&mut machine);
    }
    assert_ne!(machine.memory.read(cpu.regs.pc), 0x76, "it stopped waiting");

    // A key goes down between two of its passes, as a real one does.
    machine.ula.keyboard.press(Key::G);
    for _ in 0..100 {
        if machine.memory.read(cpu.regs.pc) == 0x76 {
            break;
        }
        cpu.step(&mut machine);
    }
    assert_eq!(machine.memory.read(cpu.regs.pc), 0x76, "it never noticed");
    assert_eq!(cpu.regs.a, 0x1F & !Key::G.mask());
}

#[test]
fn what_the_host_holds_is_what_the_program_reads() {
    // The whole path: host key events, through the table, into the matrix, out
    // of an `IN`. CTRL and P is SYMBOL SHIFT and P, which is a quote.
    let mut host = HostKeys::new();
    host.press(HostKey::Ctrl);
    host.press(HostKey::Char('p'));

    let rows = scan(host.matrix(&KeyMap::PC));
    assert_eq!(
        rows[Key::SymbolShift.half_row()],
        0xFF & !Key::SymbolShift.mask()
    );
    assert_eq!(rows[Key::P.half_row()], 0xFF & !Key::P.mask());

    // Letting the letter up leaves the shift down, because the host is still
    // holding it.
    host.release(HostKey::Char('p'));
    let rows = scan(host.matrix(&KeyMap::PC));
    assert_eq!(
        rows[Key::SymbolShift.half_row()],
        0xFF & !Key::SymbolShift.mask()
    );
    assert_eq!(rows[Key::P.half_row()], 0xFF);

    // And the window losing focus lets go of everything.
    host.clear();
    assert_eq!(scan(host.matrix(&KeyMap::PC)), [0xFF; HALF_ROWS]);
}

/// Run [`SCAN`], applying `change` to the machine just before the `IN` that
/// reads half-row `before` — so the change lands in the middle of a scan, which
/// is where a host key event actually arrives.
fn scan_interrupted_at(before: usize, change: impl FnOnce(&mut Spectrum)) -> [u8; HALF_ROWS] {
    let mut machine = Spectrum::new();
    let mut cpu = Cpu::new();
    machine.memory.load(0x8000, SCAN);
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;

    let mut change = Some(change);
    let mut reads = 0;
    for _ in 0..10_000 {
        if machine.memory.read(cpu.regs.pc) == 0x76 {
            break;
        }
        // 0x8008 is the `in a,(c)`, and `reads` is the half-row it is about to
        // take: the scan starts at $FEFE and rotates upwards.
        if cpu.regs.pc == 0x8008 {
            if reads == before {
                change.take().expect("one change")(&mut machine);
            }
            reads += 1;
        }
        cpu.step(&mut machine);
    }
    assert_eq!(reads, HALF_ROWS, "the scan did not finish");

    let mut rows = [0; HALF_ROWS];
    for (half_row, byte) in rows.iter_mut().enumerate() {
        *byte = machine.memory.read(0x9000 + half_row as u16);
    }
    rows
}

#[test]
fn a_key_pressed_part_way_through_a_scan_is_not_half_read() {
    // The bug this guards: DELETE is CAPS SHIFT and 0, the two are on
    // half-rows 0 and 4, and the ROM's scan reads them hundreds of T-states
    // apart. A matrix that changed in between would be read half from each
    // side of the change, and half of DELETE is the digit 0 — which the ROM
    // dutifully types.
    let delete = Keyboard::holding(&[Key::CapsShift, Key::Num0]);
    let shift_row = Key::CapsShift.half_row();
    let zero_row = Key::Num0.half_row();
    assert!(shift_row < zero_row, "the test's own premise");

    // Written straight into the matrix, which is the debugger's path and not a
    // frontend's, that is exactly what happens: no shift, and a 0.
    let torn = scan_interrupted_at(zero_row, |machine| machine.ula.keyboard = delete);
    assert_eq!(
        torn[shift_row], 0xFF,
        "CAPS SHIFT was read before the press"
    );
    assert_eq!(torn[zero_row], 0xFF & !Key::Num0.mask(), "0 after it");

    // Latched, the scan cannot see it at all: it belongs to the next frame.
    let whole = scan_interrupted_at(zero_row, |machine| machine.ula.set_keyboard(delete));
    assert_eq!(whole, [0xFF; HALF_ROWS]);

    // Landing anywhere else in the scan makes no difference either, which is
    // the property — not that this particular pair is special.
    for half_row in 0..HALF_ROWS {
        let rows = scan_interrupted_at(half_row, |machine| machine.ula.set_keyboard(delete));
        assert_eq!(
            rows, [0xFF; HALF_ROWS],
            "latched before half-row {half_row}"
        );
    }

    // And once the frame ends, both keys are there together.
    let rows = scan_interrupted_at(0, |machine| {
        machine.ula.set_keyboard(delete);
        machine.ula.end_frame();
    });
    assert_eq!(rows[shift_row], 0xFF & !Key::CapsShift.mask());
    assert_eq!(rows[zero_row], 0xFF & !Key::Num0.mask());
}

#[test]
fn a_frontend_hands_the_machine_host_keys_and_they_arrive_next_frame() {
    let mut machine = Spectrum::new();
    let mut host = HostKeys::new();
    host.press(HostKey::Backspace);

    machine.set_keys(host.matrix(&KeyMap::PC).matrix());
    assert_eq!(machine.ula.keyboard, Keyboard::new(), "not yet");
    machine.ula.end_frame();
    assert_eq!(
        machine.ula.keyboard,
        Keyboard::holding(&[Key::CapsShift, Key::Num0])
    );

    host.release(HostKey::Backspace);
    machine.set_keys(host.matrix(&KeyMap::PC).matrix());
    machine.ula.end_frame();
    assert_eq!(machine.ula.keyboard, Keyboard::new());
}

#[test]
fn the_ear_bit_is_read_alongside_the_keyboard() {
    let mut machine = Spectrum::new();
    machine.ula.keyboard.press(Key::Enter);
    machine.ula.set_ear(Some(false));

    let rows = scan(machine.ula.keyboard);
    assert_eq!(rows[Key::Enter.half_row()], 0xFE, "a fresh machine's EAR");

    // The same read on the machine whose EAR is low: bit 6 down, the keyboard
    // bits unchanged, and the two undriven bits still up.
    let value = machine
        .ula
        .read_port_fe(half_row_port(Key::Enter.half_row()));
    assert_eq!(value, 0xBE);
    machine.ula.set_ear(Some(true));
    assert_eq!(
        machine
            .ula
            .read_port_fe(half_row_port(Key::Enter.half_row())),
        0xFE
    );
}
