//! The keyboard matrix: eight half-rows of five keys, read through port `0xFE`.
//!
//! There is no keyboard controller on a Spectrum. There are eight wires out of
//! the ULA — the top half of the address bus — and five wires back in, and the
//! forty keys are the crossings between them. To read the keyboard you drive
//! one of the eight low and see which of the five come back low, which is what
//! `IN A,(C)` with `B` holding a single zero bit does.
//!
//! # The decode is the address, not a row number
//!
//! The ROM does not select "row 3". It puts `0xF7FE` on the bus, and `A11`
//! being the low line is what makes the `1`-to-`5` half-row answer. Nothing
//! stops more than one line being low at once, and the ROM relies on that:
//! `LD BC,$00FE` drives *all eight* low and asks "is any key at all down",
//! which is how `KEY-SCAN` starts and how the pause loop waits for a keypress.
//! So [`Keyboard::read`] takes the whole port and ANDs together every half-row
//! whose line is low, rather than indexing an array with a row number. A decode
//! that took the row number works for single keys and fails for everything the
//! ROM does with `CAPS SHIFT`.
//!
//! # Low means pressed
//!
//! A key shorts its half-row line to its column line, so a pressed key reads
//! as a *zero*. The matrix here stores the opposite — a set bit is a key that
//! is down — because that is what the rest of the program means by "pressed",
//! and the inversion happens once, in [`Keyboard::read`]. The five bits it
//! returns are bits 0-4 of the port; bits 5-7 belong to the ULA
//! ([`crate::ula::Ula::read_port_fe`] composes them).
//!
//! # Both shifts are just keys
//!
//! `CAPS SHIFT` and `SYMBOL SHIFT` are two of the forty crossings and nothing
//! more: `CAPS SHIFT` is bit 0 of the `A8` half-row and `SYMBOL SHIFT` is bit 1
//! of the `A15` one. The ROM reads them the same way as any other key and works
//! out what they mean in software. Holding `CAPS SHIFT` and `6` — the `DOWN`
//! cursor key — therefore means two bits low in two different half-rows at the
//! same time, which is why anything that can only report one key at a time
//! cannot type on this machine.

/// Half-rows in the matrix: one per address line from `A8` to `A15`.
pub const HALF_ROWS: usize = 8;

/// Keys in a half-row, which is the width of the read.
pub const KEYS_PER_HALF_ROW: usize = 5;

/// The bits of a half-row that are keys. The other three are the ULA's, and a
/// matrix arriving from outside is masked with this rather than trusted.
const KEY_BITS: u8 = (1 << KEYS_PER_HALF_ROW) - 1;

/// The keys of the matrix, in the order the hardware has them: half-row `A8`
/// first, and within each half-row the bit-0 key first.
///
/// That order is load-bearing. The discriminant *is* the position in the
/// matrix — `half_row` is the discriminant divided by five and `bit` is the
/// remainder — so reordering this list rewires the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Key {
    // A8: the shift half-row.
    CapsShift,
    Z,
    X,
    C,
    V,
    // A9
    A,
    S,
    D,
    F,
    G,
    // A10
    Q,
    W,
    E,
    R,
    T,
    // A11: the digits count up from the left edge...
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    // A12: ...and back down from the right, because the two halves of the
    // number row are wired as mirror images.
    Num0,
    Num9,
    Num8,
    Num7,
    Num6,
    // A13
    P,
    O,
    I,
    U,
    Y,
    // A14
    Enter,
    L,
    K,
    J,
    H,
    // A15
    Space,
    SymbolShift,
    M,
    N,
    B,
}

impl Key {
    /// Every key, in matrix order.
    pub const ALL: [Key; HALF_ROWS * KEYS_PER_HALF_ROW] = [
        Key::CapsShift,
        Key::Z,
        Key::X,
        Key::C,
        Key::V,
        Key::A,
        Key::S,
        Key::D,
        Key::F,
        Key::G,
        Key::Q,
        Key::W,
        Key::E,
        Key::R,
        Key::T,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num0,
        Key::Num9,
        Key::Num8,
        Key::Num7,
        Key::Num6,
        Key::P,
        Key::O,
        Key::I,
        Key::U,
        Key::Y,
        Key::Enter,
        Key::L,
        Key::K,
        Key::J,
        Key::H,
        Key::Space,
        Key::SymbolShift,
        Key::M,
        Key::N,
        Key::B,
    ];

    /// Which of the eight half-rows the key is on: 0 is the line `A8` drives,
    /// 7 is `A15`.
    pub fn half_row(self) -> usize {
        self as usize / KEYS_PER_HALF_ROW
    }

    /// Which of the five bits of the read the key is, 0-4.
    pub fn bit(self) -> u8 {
        (self as usize % KEYS_PER_HALF_ROW) as u8
    }

    /// The mask the key contributes to its half-row.
    pub fn mask(self) -> u8 {
        1 << self.bit()
    }

    /// The address a read that selects this key's half-row *alone* puts on the
    /// bus: `0xFEFE` for the `CAPS SHIFT` half-row, `0x7FFE` for the `SPACE`
    /// one. What the ROM does; what a test wants to write.
    pub fn port(self) -> u16 {
        half_row_port(self.half_row())
    }

    /// The key at `half_row` and `bit`, or `None` if either is off the matrix.
    pub fn at(half_row: usize, bit: u8) -> Option<Key> {
        if half_row >= HALF_ROWS || bit as usize >= KEYS_PER_HALF_ROW {
            return None;
        }
        Some(Key::ALL[half_row * KEYS_PER_HALF_ROW + bit as usize])
    }
}

/// The address that selects `half_row` and nothing else. The low byte is
/// `0xFE` by convention — any even port reaches the ULA — and the high byte
/// has one bit low.
pub fn half_row_port(half_row: usize) -> u16 {
    let high = !(1u8 << half_row);
    (u16::from(high) << 8) | 0x00FE
}

/// Which keys are down.
///
/// Eight bytes, one per half-row, in which a *set* bit is a key that is down.
/// Copy, because a frontend rebuilding the matrix from the host keys it is
/// holding (see [`crate::keymap`]) wants to hand a whole new one over rather
/// than mutate the machine's in place.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Keyboard {
    rows: [u8; HALF_ROWS],
}

impl Keyboard {
    /// Nothing pressed.
    pub fn new() -> Keyboard {
        Keyboard::default()
    }

    /// A keyboard with these keys held, which is what the host key mapping
    /// produces and what most tests want to say.
    pub fn holding(keys: &[Key]) -> Keyboard {
        let mut keyboard = Keyboard::new();
        for key in keys {
            keyboard.press(*key);
        }
        keyboard
    }

    /// Hold `key` down. Pressing a key that is already down does nothing,
    /// which is what the hardware does with the auto-repeat the host sends.
    pub fn press(&mut self, key: Key) {
        self.rows[key.half_row()] |= key.mask();
    }

    /// Let `key` up.
    pub fn release(&mut self, key: Key) {
        self.rows[key.half_row()] &= !key.mask();
    }

    /// Hold or release `key`.
    pub fn set(&mut self, key: Key, pressed: bool) {
        if pressed {
            self.press(key);
        } else {
            self.release(key);
        }
    }

    /// Let every key up — what a frontend does when the window loses focus,
    /// since key-up events stop arriving and the emulated machine would
    /// otherwise be left leaning on a key forever.
    pub fn clear(&mut self) {
        self.rows = [0; HALF_ROWS];
    }

    /// Is `key` down?
    pub fn is_pressed(&self, key: Key) -> bool {
        self.rows[key.half_row()] & key.mask() != 0
    }

    /// Is any key at all down? The question `LD BC,$00FE` asks.
    pub fn any_pressed(&self) -> bool {
        self.rows.iter().any(|&row| row != 0)
    }

    /// The five keyboard bits of a read of `port`: bits 0-4, in which a *low*
    /// bit is a key that is down, and bits 5-7 zero for the ULA to fill in.
    ///
    /// Every half-row whose address line is low contributes, so a port with
    /// several lines low — `0x00FE` has all eight — answers for all of them at
    /// once, and a port with none low reports no keys however many are held.
    pub fn read(&self, port: u16) -> u8 {
        let selected = !(port >> 8) as u8;
        let mut pressed = 0;
        for (half_row, bits) in self.rows.iter().enumerate() {
            if selected & (1 << half_row) != 0 {
                pressed |= bits;
            }
        }
        !pressed & 0x1F
    }

    /// The half-rows, a set bit per key held, for a snapshot writer.
    pub fn rows(&self) -> &[u8; HALF_ROWS] {
        &self.rows
    }

    /// A keyboard from half-rows written down elsewhere, such as a snapshot.
    pub fn from_rows(rows: [u8; HALF_ROWS]) -> Keyboard {
        Keyboard {
            rows: rows.map(|row| row & KEY_BITS),
        }
    }

    /// The whole matrix as one word: five bits per half-row, the first
    /// half-row lowest, a set bit per key held.
    ///
    /// This is the shape input travels in (ADR-0024). Forty bits fit above a
    /// command's kind byte, which is the reason a keypress can go through the
    /// command ring at all and therefore the reason it lands in the replay
    /// log.
    pub fn matrix(&self) -> u64 {
        self.rows
            .iter()
            .enumerate()
            .fold(0, |bits, (half_row, row)| {
                bits | u64::from(row & KEY_BITS) << (half_row * KEYS_PER_HALF_ROW)
            })
    }

    /// The inverse of [`Keyboard::matrix`]. Bits above the fortieth are not a
    /// key and are dropped.
    pub fn from_matrix(matrix: u64) -> Keyboard {
        let mut rows = [0; HALF_ROWS];
        for (half_row, row) in rows.iter_mut().enumerate() {
            *row = (matrix >> (half_row * KEYS_PER_HALF_ROW)) as u8 & KEY_BITS;
        }
        Keyboard { rows }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The matrix as published, half-row by half-row, bit 0 first. The point of
    /// writing it out again is that it is the one thing here that cannot be
    /// derived from anything else: if this table and [`Key::ALL`] agree, the
    /// wiring is right, and if they do not, no amount of correct decoding
    /// helps.
    const MATRIX: [(u16, [Key; KEYS_PER_HALF_ROW]); HALF_ROWS] = [
        (0xFEFE, [Key::CapsShift, Key::Z, Key::X, Key::C, Key::V]),
        (0xFDFE, [Key::A, Key::S, Key::D, Key::F, Key::G]),
        (0xFBFE, [Key::Q, Key::W, Key::E, Key::R, Key::T]),
        (
            0xF7FE,
            [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5],
        ),
        (
            0xEFFE,
            [Key::Num0, Key::Num9, Key::Num8, Key::Num7, Key::Num6],
        ),
        (0xDFFE, [Key::P, Key::O, Key::I, Key::U, Key::Y]),
        (0xBFFE, [Key::Enter, Key::L, Key::K, Key::J, Key::H]),
        (
            0x7FFE,
            [Key::Space, Key::SymbolShift, Key::M, Key::N, Key::B],
        ),
    ];

    #[test]
    fn every_key_is_where_the_matrix_says_it_is() {
        for (half_row, (port, keys)) in MATRIX.iter().enumerate() {
            assert_eq!(half_row_port(half_row), *port, "half-row {half_row}");
            for (bit, key) in keys.iter().enumerate() {
                assert_eq!(key.half_row(), half_row, "{key:?}");
                assert_eq!(key.bit(), bit as u8, "{key:?}");
                assert_eq!(key.port(), *port, "{key:?}");
                assert_eq!(Key::at(half_row, bit as u8), Some(*key));
            }
        }
        assert_eq!(Key::at(HALF_ROWS, 0), None);
        assert_eq!(Key::at(0, KEYS_PER_HALF_ROW as u8), None);
    }

    #[test]
    fn a_pressed_key_reads_low_on_its_own_half_row_and_nowhere_else() {
        for key in Key::ALL {
            let keyboard = Keyboard::holding(&[key]);
            assert_eq!(
                keyboard.read(key.port()),
                0x1F & !key.mask(),
                "{key:?} on its own half-row"
            );
            for other in 0..HALF_ROWS {
                if other != key.half_row() {
                    assert_eq!(
                        keyboard.read(half_row_port(other)),
                        0x1F,
                        "{key:?} leaked into half-row {other}"
                    );
                }
            }
        }
    }

    #[test]
    fn nothing_pressed_reads_all_ones_and_only_five_bits_are_ever_set() {
        let keyboard = Keyboard::new();
        assert!(!keyboard.any_pressed());
        for half_row in 0..HALF_ROWS {
            assert_eq!(keyboard.read(half_row_port(half_row)), 0x1F);
        }
        let all = Keyboard::holding(&Key::ALL);
        assert_eq!(all.read(0x00FE), 0x00);
        assert_eq!(all.read(0xFEFE) & 0xE0, 0x00);
    }

    #[test]
    fn a_read_that_selects_no_half_row_sees_no_keys() {
        // A15 to A8 all high: the ULA still answers, but nothing is being
        // asked, so every key reads up however hard it is being held.
        let keyboard = Keyboard::holding(&[Key::A, Key::Space]);
        assert_eq!(keyboard.read(0xFFFE), 0x1F);
    }

    #[test]
    fn selecting_several_half_rows_ands_them_together() {
        // What KEY-SCAN's opening `LD BC,$00FE; IN A,(C)` does: all eight lines
        // low, so the answer is "is anything down", spread across five bits.
        let mut keyboard = Keyboard::holding(&[Key::G]);
        assert!(keyboard.any_pressed());
        assert_eq!(keyboard.read(0x00FE), 0x1F & !Key::G.mask());

        // Two keys on different half-rows and different bits both show up.
        keyboard.press(Key::Enter);
        assert_eq!(
            keyboard.read(0x00FE),
            0x1F & !Key::G.mask() & !Key::Enter.mask()
        );

        // Two lines of the eight, chosen by hand: A9 (G) and A14 (ENTER).
        let both = 0xFFFEu16 & !(1 << (8 + 1)) & !(1 << (8 + 6));
        assert_eq!(
            keyboard.read(both),
            0x1F & !Key::G.mask() & !Key::Enter.mask()
        );
        // The same two keys are invisible to a read of the half-rows between
        // them.
        assert_eq!(keyboard.read(Key::Q.port()), 0x1F);
    }

    #[test]
    fn a_shifted_key_is_two_bits_in_two_half_rows() {
        // CAPS SHIFT + 6, which is what the ROM reads as cursor down.
        let keyboard = Keyboard::holding(&[Key::CapsShift, Key::Num6]);
        assert_eq!(keyboard.read(Key::CapsShift.port()), 0x1E);
        assert_eq!(keyboard.read(Key::Num6.port()), 0x0F);
        // And SYMBOL SHIFT + P, which is a quote.
        let keyboard = Keyboard::holding(&[Key::SymbolShift, Key::P]);
        assert_eq!(keyboard.read(Key::SymbolShift.port()), 0x1D);
        assert_eq!(keyboard.read(Key::P.port()), 0x1E);
        // Both shifts at once — BREAK is CAPS SHIFT and SPACE, and the two live
        // in the same half-row as their respective shifts.
        let keyboard = Keyboard::holding(&[Key::CapsShift, Key::Space]);
        assert_eq!(keyboard.read(0x7FFE), 0x1E);
        assert_eq!(keyboard.read(0xFEFE), 0x1E);
    }

    #[test]
    fn releasing_takes_one_key_up_and_leaves_the_rest_down() {
        let mut keyboard = Keyboard::holding(&[Key::CapsShift, Key::Num0]);
        keyboard.press(Key::CapsShift); // Auto-repeat: already down, still down.
        keyboard.release(Key::Num0);
        assert!(keyboard.is_pressed(Key::CapsShift));
        assert!(!keyboard.is_pressed(Key::Num0));
        assert_eq!(keyboard.read(Key::Num0.port()), 0x1F);

        keyboard.set(Key::Num0, true);
        assert!(keyboard.is_pressed(Key::Num0));
        keyboard.set(Key::Num0, false);
        assert!(!keyboard.is_pressed(Key::Num0));

        // Releasing a key that is already up is not an error and does not
        // disturb its neighbours.
        keyboard.release(Key::Num0);
        assert_eq!(keyboard, Keyboard::holding(&[Key::CapsShift]));

        keyboard.clear();
        assert_eq!(keyboard, Keyboard::new());
        assert!(!keyboard.any_pressed());
    }
}
