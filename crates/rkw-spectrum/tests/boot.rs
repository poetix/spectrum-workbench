//! Booting the real 48K ROM: the integration test the rest of this crate is
//! for.
//!
//! Everything here is skipped when there is no ROM to run — see
//! `scripts/fetch-rom.sh` — because the image is Amstrad's and is not in this
//! repository.
//!
//! # What these tests are actually checking
//!
//! Not the ROM: the ROM works. They check that a machine assembled out of
//! [`rkw_spectrum`]'s parts is enough of a Spectrum for sixteen kilobytes of
//! 1982 machine code, written against the hardware and nothing else, to run on
//! it unmodified. That exercises the memory map, the interrupt cadence, the
//! keyboard matrix and the display decode at once, and in a way no test written
//! against this crate's own API can: the ROM does not know what the API thinks
//! it should do.

mod common;

use common::Board;
use rkw_spectrum::Key;

/// Frames to give the ROM to reach the copyright message.
///
/// Most of it is the RAM check at `$11DA`, which fills and verifies all 49,152
/// bytes at about 116 T-states each — a second and a half of emulated time, and
/// the pause a real machine makes at power-on. The copyright line appears at
/// frame 84; running 150 leaves enough margin that a failure here means the
/// boot went wrong rather than that it was still going. Contention (0020) has
/// since slowed the RAM check down and the margin absorbed it, which is what it
/// was for.
const BOOT_FRAMES: u64 = 150;

/// Where the ROM's copyright message ends up: the bottom line of the display.
const COPYRIGHT_ROW: usize = 23;

const COPYRIGHT: &str = "© 1982 Sinclair Research Ltd";

/// The boot screen, as a number.
///
/// Pinned rather than compared against a stored image, for the same reason the
/// ROM is not in the tree. It covers what the text assertions cannot — the
/// pixels within each cell, the attributes, the white border — and it will
/// change if the display decode, the palette or the frame geometry does, at
/// which point the text assertions above it say whether the change was right.
const BOOT_HASH: u64 = 0x01a6_e7e8_00d8_2dee;

/// A machine booted to the prompt, or `None` when there is no ROM.
fn booted() -> Option<Board> {
    let rom = common::rom()?;
    let mut board = Board::new(&rom);
    board.run_frames(BOOT_FRAMES);
    Some(board)
}

/// The skip that every test here starts with.
macro_rules! booted {
    () => {
        match booted() {
            Some(board) => board,
            None => {
                eprintln!("{}", common::skip_message());
                return;
            }
        }
    };
}

fn peek16(board: &Board, addr: u16) -> u16 {
    u16::from_le_bytes([
        board.machine.memory.read(addr),
        board.machine.memory.read(addr + 1),
    ])
}

#[test]
fn the_rom_runs_from_reset_and_sets_its_system_variables_up() {
    let rom = match common::rom() {
        Some(rom) => rom,
        None => {
            eprintln!("{}", common::skip_message());
            return;
        }
    };
    let mut board = Board::new(&rom);
    assert_eq!(board.cpu.regs.pc, 0x0000, "a Z80 comes up at zero");
    assert!(!board.cpu.regs.iff1, "and with interrupts off");

    board.run_frames(BOOT_FRAMES);

    // The three system variables that say the initialisation ran to the end.
    // CHARS is the font pointer, and it is 256 below the glyph for space
    // because the ROM indexes it by character code.
    assert_eq!(peek16(&board, 0x5C36), 0x3C00, "CHARS");
    // P-RAMT is the top of physical RAM, which the RAM check found by walking
    // up until it stopped reading back what it wrote — so this is the memory
    // map's answer, not a constant the ROM knows.
    assert_eq!(peek16(&board, 0x5CB4), 0xFFFF, "P-RAMT");
    // RAMTOP is where BASIC's own space ends, below the user-defined graphics
    // and the printer buffer.
    assert_eq!(peek16(&board, 0x5CB2), 0xFF57, "RAMTOP");

    // The ROM sets the border white as one of its first acts, which is the
    // only visible sign of life until the copyright line appears.
    assert_eq!(board.machine.ula.border(), 7);
    // Interrupts are on and being taken: the ROM's own frame counter is
    // maintained by the handler at $0038 and by nothing else.
    let frames = u32::from_le_bytes([
        board.machine.memory.read(0x5C78),
        board.machine.memory.read(0x5C79),
        board.machine.memory.read(0x5C7A),
        0,
    ]);
    assert!(frames > 0, "FRAMES is still zero: no interrupt was taken");
}

#[test]
fn the_copyright_message_is_on_the_screen_and_nothing_else_is() {
    let board = booted!();
    let screen = board.screen_text();

    assert_eq!(screen[COPYRIGHT_ROW], COPYRIGHT);
    // A 48K machine at the prompt shows the copyright line and an empty
    // screen: the `K` cursor is not drawn until a key has been pressed.
    for (row, line) in screen.iter().enumerate() {
        if row != COPYRIGHT_ROW {
            assert_eq!(line, "", "row {row}");
        }
    }
}

#[test]
fn the_boot_screen_is_the_same_picture_every_time() {
    let board = booted!();
    assert_eq!(
        common::hash(&board.machine.frame()),
        BOOT_HASH,
        "the boot screen has changed; check the text assertions above first"
    );

    // Determinism is the property that makes the number above worth pinning:
    // nothing in the machine is seeded by the clock or by the host.
    let again = booted!();
    assert_eq!(
        again.machine.frame().pixels(),
        board.machine.frame().pixels()
    );
    assert_eq!(again.machine.t_states(), board.machine.t_states());
}

#[test]
fn the_prompt_takes_keyboard_input() {
    let mut board = booted!();
    assert!(!board.screen_contains("PRINT"));

    // At the `K` cursor a letter is a keyword, so `P` is `PRINT` — one
    // keystroke, five characters and a trailing space on the edit line.
    board.press(&[Key::P]);

    assert!(
        board.screen_contains("PRINT"),
        "nothing was typed:\n{}",
        board.screen_text().join("\n")
    );

    // And the cursor has followed it along, which is the ROM telling us it is
    // in `L` mode now and expecting the rest of a statement.
    assert!(board.screen_contains("PRINT L"));
}

/// Type `PRINT`, then press DELETE with the press landing at each of `offsets`
/// T-states into a frame, and report the offsets after which a `0` appeared on
/// the screen — which is DELETE read as half of itself.
fn zeroes_typed_by_delete(offsets: impl Iterator<Item = u64>, latched: bool) -> Option<Vec<u64>> {
    let rom = common::rom()?;
    let mut caught = Vec::new();
    for offset in offsets {
        let mut board = Board::new(&rom);
        board.run_frames(BOOT_FRAMES);
        board.press(&[Key::P]);
        assert!(board.screen_contains("PRINT"), "nothing to delete");

        if latched {
            board.press_latched(&[Key::CapsShift, Key::Num0], offset);
        } else {
            board.press_immediately(&[Key::CapsShift, Key::Num0], offset);
        }
        if board.screen_contains("0") {
            caught.push(offset);
        }
    }
    Some(caught)
}

#[test]
fn delete_landing_mid_frame_deletes_rather_than_typing_a_zero() {
    // DELETE is CAPS SHIFT and 0 on two half-rows the ROM's KEY-SCAN reads
    // hundreds of T-states apart, so a matrix written straight in as the host
    // event arrives can be read as the digit alone. The next press then deletes
    // the 0 and the one after it deletes what the user meant to delete: the
    // "delete emits a 0 first" the frontend was seeing.
    //
    // The sweep is the first thousand T-states of the frame, which is where
    // the handler at $0038 does its scanning: written straight in, the presses
    // that tear are the ones landing between roughly 190 and 450 T-states past
    // the interrupt. That is a quarter of one per cent of a frame, and it is
    // why the frontend saw this now and then rather than every time.
    let offsets = || (0..1_024).step_by(32).map(|t| t as u64);

    let Some(torn) = zeroes_typed_by_delete(offsets(), false) else {
        eprintln!("{}", common::skip_message());
        return;
    };
    assert!(
        !torn.is_empty(),
        "the sweep never landed mid-scan, so it proves nothing"
    );

    let latched = zeroes_typed_by_delete(offsets(), true).expect("the ROM was there a moment ago");
    assert!(
        latched.is_empty(),
        "a latched DELETE still typed a 0 at {latched:?}"
    );
}

#[test]
fn a_basic_program_can_be_typed_and_run() {
    let mut board = booted!();

    // `10 PRINT "hello"`, keystroke by keystroke. The `P` is one key because
    // the cursor is still in `K` mode after the line number, and the quotes are
    // SYMBOL SHIFT and `P` held together. The letters come out lower case: the
    // cursor is in `L` mode by then, and nothing here holds CAPS SHIFT.
    board.type_text("10P\"HELLO\"\n");
    // ENTER moved it out of the edit line and into the program, which the ROM
    // lists from the top of the screen with `>` marking the current line.
    assert!(
        board.screen_contains("10>PRINT \"hello\""),
        "the line was not entered:\n{}",
        board.screen_text().join("\n")
    );

    // `R` at the `K` cursor is `RUN`.
    board.type_text("R\n");

    let screen = board.screen_text();
    assert_eq!(
        screen[0],
        "hello",
        "the program did not run:\n{}",
        screen.join("\n")
    );
    // The report line, which is the ROM saying the program ended cleanly at
    // statement 1 of line 10.
    assert!(
        screen.iter().any(|line| line.starts_with("0 OK, 10:1")),
        "no report line:\n{}",
        screen.join("\n")
    );
}
