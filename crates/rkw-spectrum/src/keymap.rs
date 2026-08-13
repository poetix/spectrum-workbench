//! Host keys to matrix keys: the table a frontend types through.
//!
//! A PC keyboard has keys the Spectrum does not — `BACKSPACE`, cursor keys,
//! punctuation that lives on its own key here and behind `SYMBOL SHIFT` there —
//! and a Spectrum has forty keys that are wired somewhere specific. Bridging
//! the two is a lookup and nothing more, so it is a [`KeyMap`]: a table of host
//! key to the combination of matrix keys it holds down. [`KeyMap::PC`] is the
//! one a UK PC layout wants; a different layout, or a user who would rather
//! have `SYMBOL SHIFT` on `CAPS LOCK`, is a different table and no different
//! code.
//!
//! # A host key maps to a *combination*
//!
//! `BACKSPACE` is `CAPS SHIFT` and `0` together — the ROM has no delete key, it
//! has a shifted `0` — and the cursor keys are `CAPS SHIFT` and `5` to `8`. So
//! the table's values are slices, and one host key press can put two bits down
//! in two different half-rows.
//!
//! # Which is why the matrix is rebuilt, not edited
//!
//! Combinations overlap, and applying them as edits gets that wrong the moment
//! two are held at once. Hold `SHIFT`, then press and release `BACKSPACE`:
//! releasing `BACKSPACE` would let `CAPS SHIFT` up even though the host is
//! still holding `SHIFT` down, and the emulated machine would be left in a
//! state the user never asked for. [`HostKeys`] therefore records which host
//! keys are down and builds the whole matrix from them on every event, which
//! costs a walk of at most a dozen entries and cannot drift.
//!
//! ```
//! use rkw_spectrum::keyboard::Key;
//! use rkw_spectrum::keymap::{HostKey, HostKeys, KeyMap};
//!
//! let mut host = HostKeys::new();
//! host.press(HostKey::Shift);
//! host.press(HostKey::Backspace);
//! host.release(HostKey::Backspace);
//!
//! // SHIFT is still down, so CAPS SHIFT still is.
//! let matrix = host.matrix(&KeyMap::PC);
//! assert!(matrix.is_pressed(Key::CapsShift));
//! assert!(!matrix.is_pressed(Key::Num0));
//! ```
//!
//! # Characters, not scancodes
//!
//! [`HostKey::Char`] carries the character the host's layout produces for a
//! physical key *without modifiers*, lower case — `winit`'s
//! `key_without_modifiers`, and the equivalent elsewhere. Two things follow.
//! The obvious one is that swapping layouts is swapping the table. The less
//! obvious one is that press and release always name the same key: a frontend
//! that reported the shifted character would send `Char('A')` down and
//! `Char('a')` up if the user let go of shift first, and the matrix would keep
//! a key down forever. Shift is delivered as [`HostKey::Shift`], on its own,
//! like every other modifier.

use crate::keyboard::{Key, Keyboard};

/// A key on the host's keyboard.
///
/// Named variants only for keys that have no character, or whose character
/// depends on the layout in a way that would make the table wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostKey {
    /// The unmodified, lower-case character the host layout gives the physical
    /// key: `Char('a')` whether or not shift is down.
    Char(char),
    Enter,
    Space,
    Backspace,
    Escape,
    /// Either shift key.
    Shift,
    /// Either control key.
    Ctrl,
    /// Either alt key, which is `SYMBOL SHIFT` as well, so that the shift a
    /// user reaches for with their right hand works.
    Alt,
    Left,
    Right,
    Up,
    Down,
}

/// A host key and the matrix keys it holds down.
pub type Entry = (HostKey, &'static [Key]);

/// A table from host keys to matrix keys.
#[derive(Debug, Clone, Copy)]
pub struct KeyMap {
    entries: &'static [Entry],
}

impl Default for KeyMap {
    fn default() -> Self {
        KeyMap::PC
    }
}

impl KeyMap {
    /// A table of a caller's own. The lookup is a scan, so order the entries
    /// however reads best; duplicates are resolved by the first match.
    pub const fn new(entries: &'static [Entry]) -> KeyMap {
        KeyMap { entries }
    }

    /// The matrix keys `host` holds down, empty if the table does not have it.
    pub fn keys(&self, host: HostKey) -> &'static [Key] {
        self.entries
            .iter()
            .find(|(key, _)| *key == host)
            .map(|(_, keys)| *keys)
            .unwrap_or(&[])
    }

    /// The table itself, for a frontend that wants to show a user what their
    /// keyboard does.
    pub fn entries(&self) -> &'static [Entry] {
        self.entries
    }

    /// A UK PC keyboard.
    ///
    /// Letters and digits go where they say. The rest is what the ROM would
    /// make you type: `BACKSPACE` is `CAPS SHIFT`+`0`, the cursor keys are
    /// `CAPS SHIFT`+`5` to `8`, `ESCAPE` is `BREAK`, and the punctuation on a
    /// host key of its own becomes the `SYMBOL SHIFT` combination that prints
    /// the same character.
    ///
    /// Characters that only exist on the Spectrum in extended mode — `[`, `]`,
    /// `{`, `}`, the copyright sign — are deliberately absent: there is no one
    /// combination that types them, and a frontend guessing at a mode change is
    /// worse than a key that does nothing.
    pub const PC: KeyMap = KeyMap::new(&[
        (HostKey::Char('a'), &[Key::A]),
        (HostKey::Char('b'), &[Key::B]),
        (HostKey::Char('c'), &[Key::C]),
        (HostKey::Char('d'), &[Key::D]),
        (HostKey::Char('e'), &[Key::E]),
        (HostKey::Char('f'), &[Key::F]),
        (HostKey::Char('g'), &[Key::G]),
        (HostKey::Char('h'), &[Key::H]),
        (HostKey::Char('i'), &[Key::I]),
        (HostKey::Char('j'), &[Key::J]),
        (HostKey::Char('k'), &[Key::K]),
        (HostKey::Char('l'), &[Key::L]),
        (HostKey::Char('m'), &[Key::M]),
        (HostKey::Char('n'), &[Key::N]),
        (HostKey::Char('o'), &[Key::O]),
        (HostKey::Char('p'), &[Key::P]),
        (HostKey::Char('q'), &[Key::Q]),
        (HostKey::Char('r'), &[Key::R]),
        (HostKey::Char('s'), &[Key::S]),
        (HostKey::Char('t'), &[Key::T]),
        (HostKey::Char('u'), &[Key::U]),
        (HostKey::Char('v'), &[Key::V]),
        (HostKey::Char('w'), &[Key::W]),
        (HostKey::Char('x'), &[Key::X]),
        (HostKey::Char('y'), &[Key::Y]),
        (HostKey::Char('z'), &[Key::Z]),
        (HostKey::Char('0'), &[Key::Num0]),
        (HostKey::Char('1'), &[Key::Num1]),
        (HostKey::Char('2'), &[Key::Num2]),
        (HostKey::Char('3'), &[Key::Num3]),
        (HostKey::Char('4'), &[Key::Num4]),
        (HostKey::Char('5'), &[Key::Num5]),
        (HostKey::Char('6'), &[Key::Num6]),
        (HostKey::Char('7'), &[Key::Num7]),
        (HostKey::Char('8'), &[Key::Num8]),
        (HostKey::Char('9'), &[Key::Num9]),
        (HostKey::Enter, &[Key::Enter]),
        (HostKey::Space, &[Key::Space]),
        (HostKey::Shift, &[Key::CapsShift]),
        (HostKey::Ctrl, &[Key::SymbolShift]),
        (HostKey::Alt, &[Key::SymbolShift]),
        // DELETE.
        (HostKey::Backspace, &[Key::CapsShift, Key::Num0]),
        // BREAK, which is how you stop a BASIC program.
        (HostKey::Escape, &[Key::CapsShift, Key::Space]),
        (HostKey::Left, &[Key::CapsShift, Key::Num5]),
        (HostKey::Down, &[Key::CapsShift, Key::Num6]),
        (HostKey::Up, &[Key::CapsShift, Key::Num7]),
        (HostKey::Right, &[Key::CapsShift, Key::Num8]),
        // Punctuation that has a key of its own on a PC and lives behind
        // SYMBOL SHIFT here.
        (HostKey::Char(','), &[Key::SymbolShift, Key::N]),
        (HostKey::Char('.'), &[Key::SymbolShift, Key::M]),
        (HostKey::Char(';'), &[Key::SymbolShift, Key::O]),
        (HostKey::Char('\''), &[Key::SymbolShift, Key::Num7]),
        (HostKey::Char('-'), &[Key::SymbolShift, Key::J]),
        (HostKey::Char('='), &[Key::SymbolShift, Key::L]),
        (HostKey::Char('/'), &[Key::SymbolShift, Key::V]),
        (HostKey::Char('\\'), &[Key::SymbolShift, Key::D]),
    ]);
}

/// How many host keys can be held at once.
///
/// Ten fingers and a chin. The limit exists so that nothing allocates on the
/// path a key event takes (ADR-0007), not because holding more would mean
/// anything.
pub const MAX_HELD: usize = 12;

/// The host keys currently down, and the matrix they make.
///
/// A frontend owns one of these, feeds it every key event, and hands
/// [`HostKeys::matrix`] to the machine — as a command rather than by touching
/// the machine directly, once ticket 0026 makes input part of the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostKeys {
    held: [Option<HostKey>; MAX_HELD],
    len: usize,
}

impl HostKeys {
    /// Nothing held.
    pub fn new() -> HostKeys {
        HostKeys::default()
    }

    /// `host` is down. A repeat of a key already down does nothing.
    ///
    /// Past [`MAX_HELD`] the oldest key held is forgotten to make room. That is
    /// the wrong answer to a genuine twelve-finger chord and the right one to
    /// the way the limit is actually reached, which is a key-up event lost to a
    /// window losing focus mid-press: the stuck key ages out instead of jamming
    /// the machine for good.
    pub fn press(&mut self, host: HostKey) {
        if self.is_held(host) {
            return;
        }
        if self.len == MAX_HELD {
            self.held.copy_within(1.., 0);
            self.len -= 1;
        }
        self.held[self.len] = Some(host);
        self.len += 1;
    }

    /// `host` is up. Releasing a key that was not down does nothing.
    pub fn release(&mut self, host: HostKey) {
        let Some(at) = self.held[..self.len].iter().position(|k| *k == Some(host)) else {
            return;
        };
        self.held.copy_within(at + 1.., at);
        self.len -= 1;
        self.held[self.len] = None;
    }

    /// Everything is up: what a frontend does when the window loses focus and
    /// key-up events stop arriving.
    pub fn clear(&mut self) {
        *self = HostKeys::new();
    }

    /// Is `host` down?
    pub fn is_held(&self, host: HostKey) -> bool {
        self.held[..self.len].contains(&Some(host))
    }

    /// The host keys held, oldest first.
    pub fn held(&self) -> impl Iterator<Item = HostKey> + '_ {
        self.held[..self.len].iter().filter_map(|k| *k)
    }

    /// The matrix these host keys make under `map`.
    pub fn matrix(&self, map: &KeyMap) -> Keyboard {
        let mut keyboard = Keyboard::new();
        for host in self.held() {
            for key in map.keys(host) {
                keyboard.press(*key);
            }
        }
        keyboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_letter_is_a_key_and_a_missing_key_is_nothing() {
        assert_eq!(KeyMap::PC.keys(HostKey::Char('q')), &[Key::Q]);
        assert_eq!(KeyMap::PC.keys(HostKey::Char('0')), &[Key::Num0]);
        // Uppercase is not in the table on purpose: a frontend sends the
        // unmodified character and SHIFT separately.
        assert_eq!(KeyMap::PC.keys(HostKey::Char('Q')), &[]);
        assert_eq!(KeyMap::PC.keys(HostKey::Char('[')), &[]);
    }

    #[test]
    fn every_entry_names_keys_and_no_host_key_is_in_the_table_twice() {
        let mut seen: Vec<HostKey> = Vec::new();
        for (host, keys) in KeyMap::PC.entries() {
            assert!(!keys.is_empty(), "{host:?} maps to nothing");
            assert!(!seen.contains(host), "{host:?} is in the table twice");
            seen.push(*host);
        }
        // Every key of the matrix is reachable from the host keyboard, which is
        // the property that makes the machine typeable.
        let reachable: Vec<Key> = KeyMap::PC
            .entries()
            .iter()
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect();
        for key in Key::ALL {
            assert!(reachable.contains(&key), "{key:?} cannot be typed");
        }
    }

    #[test]
    fn a_host_key_can_hold_two_matrix_keys_down() {
        let mut host = HostKeys::new();
        host.press(HostKey::Backspace);
        let matrix = host.matrix(&KeyMap::PC);
        assert_eq!(matrix, Keyboard::holding(&[Key::CapsShift, Key::Num0]));
        // Which is what the ROM reads as DELETE: two half-rows, one bit each.
        assert_eq!(matrix.read(Key::CapsShift.port()), 0x1E);
        assert_eq!(matrix.read(Key::Num0.port()), 0x1E);
    }

    #[test]
    fn releasing_a_combination_leaves_a_modifier_the_host_still_holds() {
        // The reason the matrix is rebuilt rather than edited.
        let mut host = HostKeys::new();
        host.press(HostKey::Shift);
        host.press(HostKey::Backspace);
        assert_eq!(
            host.matrix(&KeyMap::PC),
            Keyboard::holding(&[Key::CapsShift, Key::Num0])
        );

        host.release(HostKey::Backspace);
        assert_eq!(
            host.matrix(&KeyMap::PC),
            Keyboard::holding(&[Key::CapsShift])
        );
        assert!(host.is_held(HostKey::Shift));

        host.release(HostKey::Shift);
        assert_eq!(host.matrix(&KeyMap::PC), Keyboard::new());
    }

    #[test]
    fn typing_a_shifted_character_is_two_host_keys() {
        let mut host = HostKeys::new();
        host.press(HostKey::Ctrl);
        host.press(HostKey::Char('p'));
        // SYMBOL SHIFT + P, which is a quote.
        assert_eq!(
            host.matrix(&KeyMap::PC),
            Keyboard::holding(&[Key::SymbolShift, Key::P])
        );
        // And either alt is the same shift, so a right hand works.
        host.clear();
        host.press(HostKey::Alt);
        host.press(HostKey::Char('p'));
        assert_eq!(
            host.matrix(&KeyMap::PC),
            Keyboard::holding(&[Key::SymbolShift, Key::P])
        );
    }

    #[test]
    fn holding_and_releasing_keeps_the_rest_in_order() {
        let mut host = HostKeys::new();
        for c in ['a', 'b', 'c'] {
            host.press(HostKey::Char(c));
        }
        host.press(HostKey::Char('b')); // A repeat, not a second press.
        assert_eq!(
            host.held().collect::<Vec<_>>(),
            vec![HostKey::Char('a'), HostKey::Char('b'), HostKey::Char('c')]
        );

        host.release(HostKey::Char('b'));
        host.release(HostKey::Char('z')); // Never pressed.
        assert_eq!(
            host.held().collect::<Vec<_>>(),
            vec![HostKey::Char('a'), HostKey::Char('c')]
        );
        assert_eq!(
            host.matrix(&KeyMap::PC),
            Keyboard::holding(&[Key::A, Key::C])
        );

        host.clear();
        assert_eq!(host, HostKeys::new());
        assert_eq!(host.held().count(), 0);
    }

    #[test]
    fn past_the_limit_the_oldest_key_held_is_forgotten() {
        let mut host = HostKeys::new();
        let keys: Vec<HostKey> = "abcdefghijklmnop".chars().map(HostKey::Char).collect();
        for key in &keys {
            host.press(*key);
        }
        assert_eq!(host.held().count(), MAX_HELD);
        assert_eq!(
            host.held().collect::<Vec<_>>(),
            keys[keys.len() - MAX_HELD..]
        );
        assert!(!host.is_held(HostKey::Char('a')));
        assert!(host.is_held(HostKey::Char('p')));
    }

    #[test]
    fn a_table_of_a_callers_own_replaces_the_default() {
        // A layout where the letters are somewhere else, as a table and not as
        // a patch to any code.
        const DVORAK_ISH: KeyMap = KeyMap::new(&[
            (HostKey::Char('a'), &[Key::A]),
            (HostKey::Char('o'), &[Key::S]),
        ]);
        let mut host = HostKeys::new();
        host.press(HostKey::Char('o'));
        assert_eq!(host.matrix(&DVORAK_ISH), Keyboard::holding(&[Key::S]));
        assert_eq!(host.matrix(&KeyMap::PC), Keyboard::holding(&[Key::O]));
        assert_eq!(KeyMap::default().keys(HostKey::Enter), &[Key::Enter]);
    }
}
