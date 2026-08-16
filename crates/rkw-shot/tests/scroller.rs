//! The scrolling mechanics of `games/scroller`, checked against the machine.
//!
//! These are the assertions the pictures cannot make. A screenshot shows that
//! something moved; what has to be true is that the playfield is *exactly* the
//! window of the ring buffer the game thinks it is, that consecutive frames are
//! that window one line apart, that nothing but a ship survives inside the
//! outline its mask clears, that a ship leaves no attribute behind it, and that
//! the whole frame's work fits in a frame.

use std::path::PathBuf;

use rkw_shot::Rig;
use rkw_spectrum::frame::T_STATES_PER_FRAME;
use rkw_spectrum::screen::pixel_addr;
use rkw_spectrum::{Key, Keyboard, SCREEN_BASE};

const PF_COL: usize = 4;
const PF_W: usize = 24;
const PF_H: usize = 128;
const ATTR_FIELD: u8 = 0b0000_0101;
const ACT_CX: u16 = 0;
const ACT_CY: u16 = 1;
const ACT_ATTR: u16 = 2;
const SPR_W: usize = 2;
const SPR_H: usize = 16;
const SPR_CELLS: u16 = 2;

/// Where an actor is, on the character grid: column, row.
fn actor(rig: &Rig, name: &str) -> (u8, u8) {
    let at = rig.symbol(name);
    (rig.peek(at + ACT_CX), rig.peek(at + ACT_CY))
}

/// A sprite's artwork, a row to a `u16`.
fn artwork(rig: &Rig, name: &str) -> Vec<u16> {
    let at = rig.symbol(name);
    (0..SPR_H)
        .map(|row| {
            let byte = at + (row * SPR_W) as u16;
            u16::from_be_bytes([rig.peek(byte), rig.peek(byte + 1)])
        })
        .collect()
}

/// The sixteen screen rows a ship is standing on, a row to a `u16`.
fn screen_rows(rig: &Rig, cx: u8, cy: u8) -> Vec<u16> {
    let top = usize::from(cy) * 8;
    (0..SPR_H)
        .map(|row| {
            let left = pixel_addr(SCREEN_BASE, PF_COL + usize::from(cx), top + row);
            u16::from_be_bytes([rig.peek(left), rig.peek(left + 1)])
        })
        .collect()
}

fn source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/scroller/main.asm")
}

fn game() -> Rig {
    Rig::assemble(&source()).unwrap_or_else(|e| panic!("{e}"))
}

/// The playfield as it is on the screen: PF_H rows of PF_W bytes.
fn screen_window(rig: &Rig) -> Vec<Vec<u8>> {
    (0..PF_H)
        .map(|y| {
            (0..PF_W)
                .map(|x| rig.peek(pixel_addr(SCREEN_BASE, PF_COL + x, y)))
                .collect()
        })
        .collect()
}

/// The playfield as the ring buffer says it should be: PF_H lines from `src`.
fn buffer_window(rig: &Rig) -> Vec<Vec<u8>> {
    let src = rig.peek_word(rig.symbol("src"));
    (0..PF_H)
        .map(|y| {
            let line = src.wrapping_add((y * PF_W) as u16);
            (0..PF_W)
                .map(|x| rig.peek(line.wrapping_add(x as u16)))
                .collect()
        })
        .collect()
}

/// The pixel rows a ship covers: it stands on the character
/// grid, so those are exactly the sixteen rows of its two cell rows. The
/// sprites are drawn after the blit, so a terrain comparison leaves them alone.
fn covered(rig: &Rig) -> Vec<bool> {
    let mut covered = vec![false; PF_H];
    for name in ["ship", "enemy"] {
        let (_, cy) = actor(rig, name);
        let top = usize::from(cy) * 8;
        for row in top..top + SPR_H {
            if let Some(slot) = covered.get_mut(row) {
                *slot = true;
            }
        }
    }
    covered
}

#[test]
fn the_playfield_is_the_window_the_ring_buffer_says_it_is() {
    let mut rig = game();
    rig.run_frames(120);

    let screen = screen_window(&rig);
    let buffer = buffer_window(&rig);
    let covered = covered(&rig);

    let mut compared = 0;
    for (y, (on_screen, in_buffer)) in screen.iter().zip(&buffer).enumerate() {
        if covered[y] {
            continue;
        }
        assert_eq!(on_screen, in_buffer, "row {y} is not the buffer's line");
        compared += 1;
    }
    // Two blocks at most, so most of the window is terrain and the test is not
    // quietly comparing nothing.
    assert!(compared >= PF_H - 2 * SPR_H, "{compared} rows");
}

#[test]
fn a_frame_moves_the_window_by_exactly_one_pixel_row() {
    let mut rig = game();
    rig.run_frames(120);
    let before = screen_window(&rig);
    let covered_before = covered(&rig);

    rig.run_frames(1);
    let after = screen_window(&rig);
    let covered_after = covered(&rig);

    // What was on row y is on row y+1 a frame later: the terrain moves down
    // the screen, which is the direction a vertical scroller flies in.
    let mut moved = 0;
    for y in 0..PF_H - 1 {
        if covered_before[y] || covered_after[y + 1] {
            continue;
        }
        assert_eq!(before[y], after[y + 1], "row {y} did not move down one");
        moved += 1;
    }
    assert!(moved > 64, "only {moved} rows were checked");

    // And new lines really are being made, or a window that had stopped
    // walking would pass the test above. Two *consecutive* lines can be
    // identical — the canyon walls only wander every fourth one — so this
    // asks over a stretch of them rather than across a single frame.
    let mut fresh = 0;
    let mut previous = screen_window(&rig)[0].clone();
    for _ in 0..16 {
        rig.run_frames(1);
        let top = screen_window(&rig)[0].clone();
        if top != previous {
            fresh += 1;
        }
        previous = top;
    }
    assert!(fresh > 0, "no new line came into view in sixteen frames");
}

#[test]
fn the_ring_buffer_mirror_matches_the_lines_it_mirrors() {
    let mut rig = game();
    rig.run_frames(300); // more than a lap of the 256-line ring

    let buf = rig.symbol("BUF");
    let mirror = (PF_H * PF_W) as u16; // the mirrored region is PF_H lines long
    for offset in 0..mirror {
        let line = buf.wrapping_add(offset);
        assert_eq!(
            rig.peek(line),
            rig.peek(line.wrapping_add(6144)),
            "the mirror of ${line:04X} disagrees with it"
        );
    }
}

#[test]
fn a_ship_stamps_its_cells_and_hands_them_back_when_it_moves() {
    let mut rig = game();
    rig.run_frames(60);

    // Bright white on black: the ship's own colour, in all four of its cells.
    let stamped = rig.peek_word(rig.symbol("ship") + ACT_ATTR);
    for row in 0..SPR_CELLS {
        for column in 0..SPR_CELLS {
            let cell = stamped + row * 32 + column;
            assert_eq!(rig.peek(cell), 0b0100_0111, "cell {row},{column}");
        }
    }

    // Hold O long enough to leave those cells behind entirely.
    let (before, _) = actor(&rig, "ship");
    rig.machine.ula.keyboard = Keyboard::holding(&[Key::O]);
    rig.run_frames(20);
    rig.machine.ula.keyboard = Keyboard::new();
    rig.run_frames(2);

    let (after, _) = actor(&rig, "ship");
    assert!(after + 2 <= before, "the ship did not clear its old cells");

    // The cells it left are the playfield's again, not a white smear.
    for row in 0..SPR_CELLS {
        for column in 0..SPR_CELLS {
            let cell = stamped + row * 32 + column;
            assert_eq!(rig.peek(cell), ATTR_FIELD, "an attribute was left behind");
        }
    }
}

/// The whole of the clash argument, as an assertion.
///
/// A ship's cells hold the ship and nothing else: the artwork where the artwork
/// is, black everywhere else, because the plotter writes rather than masks. So
/// there is no terrain pixel anywhere inside the four cells the ship has
/// stamped with its own colour, and a clash is not near-avoided but impossible.
fn cells_hold_nothing_but_the_ship(rig: &Rig, name: &str, art: &str) {
    let (cx, cy) = actor(rig, name);
    let shape = artwork(rig, art);
    let on_screen = screen_rows(rig, cx, cy);

    for row in 0..SPR_H {
        assert_eq!(
            on_screen[row], shape[row],
            "{name} at ({cx},{cy}), row {row}: the cells hold something that is not the ship"
        );
    }
}

#[test]
fn a_ship_s_cells_hold_nothing_but_the_ship() {
    let mut rig = game();
    // Spread over a lap of the terrain, so the ships are checked against
    // canyon wall, open channel and marker rows alike.
    for _ in 0..12 {
        rig.run_frames(19);
        cells_hold_nothing_but_the_ship(&rig, "ship", "ship_gfx");
        cells_hold_nothing_but_the_ship(&rig, "enemy", "enemy_gfx");
    }
}

/// Frame-filling is what makes the write above look like a ship rather than a
/// black box, so it is a property of the artwork worth holding on to: every
/// row of a sprite's own two cells carries something, and the shape reaches
/// both edges somewhere.
#[test]
fn the_artwork_fills_the_square_it_is_drawn_in() {
    let rig = game();
    for art in ["ship_gfx", "enemy_gfx"] {
        let shape = artwork(&rig, art);
        let lit: u32 = shape.iter().map(|row| row.count_ones()).sum();
        let coverage = 100 * lit / (SPR_H as u32 * 16);
        assert!(coverage > 60, "{art} covers only {coverage}% of its square");
        assert!(
            shape.iter().any(|row| row & 0x8000 != 0),
            "{art} never reaches its left edge"
        );
        assert!(
            shape.iter().any(|row| row & 1 != 0),
            "{art} never reaches its right edge"
        );
    }
}

#[test]
fn a_ship_moves_on_the_character_grid() {
    let mut rig = game();
    rig.run_frames(60);

    let (before, _) = actor(&rig, "ship");
    rig.machine.ula.keyboard = Keyboard::holding(&[Key::P]);
    rig.run_frames(1);
    rig.machine.ula.keyboard = Keyboard::new();
    rig.run_frames(1);

    let (after, _) = actor(&rig, "ship");
    assert_eq!(after, before + 1, "a step is one character column");

    // And it is a step, not a slide: the repeat rate holds it there.
    rig.machine.ula.keyboard = Keyboard::holding(&[Key::P]);
    rig.run_frames(2);
    let (still, _) = actor(&rig, "ship");
    assert_eq!(
        still, after,
        "the ship stepped again inside the repeat delay"
    );
}

#[test]
fn firing_puts_a_bullet_on_the_screen_at_a_pixel_position() {
    let mut rig = game();
    rig.run_frames(60);

    rig.machine.ula.keyboard = Keyboard::holding(&[Key::Space]);
    rig.run_frames(2);

    let bullets = rig.symbol("bullets");
    assert_eq!(rig.peek(bullets), 1, "no bullet was fired");
    let x = rig.peek(bullets + 1);
    let y = rig.peek(bullets + 2);
    // Pixel-positioned, and fired from the middle of a two-cell ship, so it
    // sits across a cell boundary rather than on one.
    assert_ne!(x % 8, 0, "the bullet is character-aligned after all");

    // Two pixels of it are on the screen, and they are inside the cell its
    // pixel position puts them in.
    let address = pixel_addr(SCREEN_BASE, PF_COL + usize::from(x / 8), usize::from(y));
    let shifted = 0xC000u16 >> (x % 8);
    let left = rig.peek(address);
    assert_eq!(
        left & (shifted >> 8) as u8,
        (shifted >> 8) as u8,
        "the bullet is not where its coordinates say"
    );
}

#[test]
fn every_frame_of_work_fits_inside_a_frame() {
    let mut rig = game();
    rig.run_frames(120);

    let profile = rig.profile();
    // Black is the border while the game is halted, waiting for the next
    // interrupt. If there is none of it, the frame overran.
    assert!(
        profile.lines(0) > 0,
        "no idle time in the frame:\n{}",
        profile.report()
    );

    let busy: u64 = (1..8).map(|colour| profile.t_states(colour)).sum();
    assert!(
        busy < T_STATES_PER_FRAME,
        "the frame's work is {busy} T-states:\n{}",
        profile.report()
    );

    // The blit is the red stripe, and it is the thing worth watching: if it
    // ever grows past four fifths of a frame there is no game left to write.
    let blit = profile.t_states(2);
    assert!(
        blit < T_STATES_PER_FRAME * 4 / 5,
        "the blit is {blit} T-states, {:.1}% of a frame",
        profile.percent(2)
    );
}
