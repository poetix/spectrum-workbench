//! What a key on the host's keyboard means here.
//!
//! Two answers: a key on the emulated machine, which goes to
//! [`HostKeys`](rkw_spectrum::HostKeys) and from there into the matrix, or a
//! command to the frontend itself. Which of the two it is has to be settled
//! before the matrix is touched, because a key that did both would type into
//! BASIC every time the user paused.
//!
//! # Why the frontend's own keys are all function keys
//!
//! A Spectrum uses every letter, every digit, both shifts, `ENTER` and
//! `SPACE` — and `BREAK` is `CAPS SHIFT` and `SPACE`, so even the modifiers
//! are spoken for. There is nothing left to put a frontend command on except
//! the keys the machine has never heard of, which is what the function keys
//! are. No modifier combination is used either: `SYMBOL SHIFT` and a letter is
//! how a Spectrum types a comma.
//!
//! # Layout, not scancode
//!
//! [`translate`] is given the key `winit` reports with the modifiers taken
//! off, so `A` and `a` are one key and a French layout types what is printed
//! on it. That is the same character
//! [`HostKey::Char`](rkw_spectrum::HostKey) is documented to carry, and it is
//! what makes press and release name the same key when the user lets go of
//! shift first.

use rkw_spectrum::HostKey;
use winit::keyboard::{Key, NamedKey};

/// Something the frontend does rather than the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    /// Stop the machine, or start it again.
    Pause,
    /// Reset the CPU, as the real machine's reset line does.
    Reset,
    /// Full screen, or back.
    Fullscreen,
    /// The next speed round: 1×, 2×, as fast as it will go.
    Speed,
    /// Silence, or sound again.
    Mute,
    /// Start the tape, or stop it if it is running.
    Tape,
    /// Wind the tape back to its first block.
    Rewind,
    /// Close the window.
    Quit,
}

/// What a key press turns out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A key on the emulated keyboard.
    Machine(HostKey),
    /// A command to the frontend. Repeats are ignored by the caller, because
    /// holding down a fullscreen key should not flap.
    Frontend(Hotkey),
    /// A key this frontend has no use for.
    Ignored,
}

/// What `key` means, with the host's modifiers already taken off.
pub fn translate(key: &Key) -> Action {
    match key {
        Key::Named(named) => match named {
            NamedKey::F5 => Action::Frontend(Hotkey::Pause),
            NamedKey::F6 => Action::Frontend(Hotkey::Tape),
            NamedKey::F7 => Action::Frontend(Hotkey::Rewind),
            NamedKey::F8 => Action::Frontend(Hotkey::Speed),
            NamedKey::F9 => Action::Frontend(Hotkey::Mute),
            NamedKey::F10 => Action::Frontend(Hotkey::Reset),
            NamedKey::F11 => Action::Frontend(Hotkey::Fullscreen),
            NamedKey::F4 => Action::Frontend(Hotkey::Quit),
            NamedKey::Enter => Action::Machine(HostKey::Enter),
            NamedKey::Space => Action::Machine(HostKey::Space),
            NamedKey::Backspace => Action::Machine(HostKey::Backspace),
            // The Spectrum's BREAK, and not a way out of the window: a program
            // being stopped is what the user means by it far more often.
            NamedKey::Escape => Action::Machine(HostKey::Escape),
            NamedKey::Shift => Action::Machine(HostKey::Shift),
            NamedKey::Control => Action::Machine(HostKey::Ctrl),
            NamedKey::Alt => Action::Machine(HostKey::Alt),
            NamedKey::ArrowLeft => Action::Machine(HostKey::Left),
            NamedKey::ArrowRight => Action::Machine(HostKey::Right),
            NamedKey::ArrowUp => Action::Machine(HostKey::Up),
            NamedKey::ArrowDown => Action::Machine(HostKey::Down),
            _ => Action::Ignored,
        },
        // One character only: a dead key that resolved to a string is not a
        // key the matrix has, and neither is anything above the BMP.
        Key::Character(text) => match one_char(text) {
            Some(c) => Action::Machine(HostKey::Char(c.to_ascii_lowercase())),
            None => Action::Ignored,
        },
        _ => Action::Ignored,
    }
}

/// The single character `text` is, if it is one.
fn one_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_letter_is_a_key_on_the_machine_whichever_case_the_host_reports() {
        assert_eq!(
            translate(&Key::Character("a".into())),
            Action::Machine(HostKey::Char('a'))
        );
        // Belt and braces: `key_without_modifiers` is documented to be lower
        // case already, and a platform that disagreed would otherwise leave a
        // key held down forever.
        assert_eq!(
            translate(&Key::Character("A".into())),
            Action::Machine(HostKey::Char('a'))
        );
    }

    #[test]
    fn the_keys_a_spectrum_has_are_all_the_machines() {
        for (named, expected) in [
            (NamedKey::Enter, HostKey::Enter),
            (NamedKey::Space, HostKey::Space),
            (NamedKey::Backspace, HostKey::Backspace),
            (NamedKey::Escape, HostKey::Escape),
            (NamedKey::Shift, HostKey::Shift),
            (NamedKey::Control, HostKey::Ctrl),
            (NamedKey::Alt, HostKey::Alt),
            (NamedKey::ArrowUp, HostKey::Up),
        ] {
            assert_eq!(
                translate(&Key::Named(named)),
                Action::Machine(expected),
                "{named:?}"
            );
        }
    }

    #[test]
    fn the_frontends_own_keys_are_function_keys_and_nothing_else() {
        assert_eq!(
            translate(&Key::Named(NamedKey::F5)),
            Action::Frontend(Hotkey::Pause)
        );
        assert_eq!(
            translate(&Key::Named(NamedKey::F11)),
            Action::Frontend(Hotkey::Fullscreen)
        );
        assert_eq!(
            translate(&Key::Named(NamedKey::F6)),
            Action::Frontend(Hotkey::Tape)
        );
        assert_eq!(
            translate(&Key::Named(NamedKey::F7)),
            Action::Frontend(Hotkey::Rewind)
        );
        // Nothing a Spectrum can type is one of ours.
        for c in "abcdefghijklmnopqrstuvwxyz0123456789".chars() {
            assert!(matches!(
                translate(&Key::Character(c.to_string().into())),
                Action::Machine(_)
            ));
        }
    }

    #[test]
    fn a_dead_key_or_a_multi_character_string_is_not_a_key() {
        assert_eq!(translate(&Key::Character("".into())), Action::Ignored);
        assert_eq!(translate(&Key::Character("ch".into())), Action::Ignored);
        assert_eq!(translate(&Key::Named(NamedKey::F1)), Action::Ignored);
    }
}
