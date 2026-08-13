//! The 48K memory map: 16K of ROM at the bottom, 48K of RAM above it.
//!
//! There is no paging on this machine, so the whole of the map is one decision
//! made per write: below `0x4000` the address decodes to ROM, and a write to
//! ROM does nothing at all. It does not fault, it does not warn, and the
//! instruction that made it carries on — which matters, because software
//! genuinely does write to ROM addresses (a `LDIR` that runs off the bottom of
//! a buffer, the ROM's own `LD (HL),A` with a stale pointer) and expects to
//! survive it.
//!
//! # Writing and poking are different operations
//!
//! [`Memory::write`] is the machine writing, and it is what the bus calls.
//! [`Memory::poke`] is a loader or a debugger writing, and it is not subject to
//! the map at all: loading a ROM image, restoring a snapshot and patching a ROM
//! routine from the debugger are all things done *to* the machine rather than
//! *by* it, and a ROM image that had to be written through a bus which refuses
//! ROM writes could not be loaded.
//!
//! [`Peek`] is the third of these, and is the read that nothing can notice —
//! the same distinction the debugger already draws (ADR-0018). It reads ROM and
//! RAM alike, because reading is never the half that the map restricts.

use z80::disasm::Peek;

/// Where ROM ends and RAM begins.
pub const ROM_SIZE: usize = 0x4000;

/// The Z80's flat 64K.
pub const ADDRESS_SPACE: usize = 0x1_0000;

/// The first address the screen is at by convention. Nothing in the decode
/// depends on it — that is the point of [`crate::screen`] — but a loader and a
/// debugger both need to be able to name it.
pub const SCREEN_BASE: u16 = 0x4000;

/// The 48K address space, with ROM write-protected.
#[derive(Clone)]
pub struct Memory {
    bytes: Box<[u8; ADDRESS_SPACE]>,
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

impl Memory {
    /// All zeroes, which is a machine with no ROM in it. A CPU reset into this
    /// executes `NOP`s from `0x0000` until something loads one.
    pub fn new() -> Memory {
        Memory {
            bytes: Box::new([0; ADDRESS_SPACE]),
        }
    }

    /// The byte at `addr`, whether it is ROM or RAM.
    pub fn read(&self, addr: u16) -> u8 {
        self.bytes[addr as usize]
    }

    /// The machine writing. Ignored below [`ROM_SIZE`].
    pub fn write(&mut self, addr: u16, value: u8) {
        if (addr as usize) < ROM_SIZE {
            return;
        }
        self.bytes[addr as usize] = value;
    }

    /// A loader or a debugger writing, which the map does not apply to.
    pub fn poke(&mut self, addr: u16, value: u8) {
        self.bytes[addr as usize] = value;
    }

    /// Copy `bytes` in at `addr`, wrapping at the top of the address space as
    /// the Z80's own arithmetic does. Pokes rather than writes, so this loads
    /// ROM images as readily as RAM.
    pub fn load(&mut self, addr: u16, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.poke(addr.wrapping_add(i as u16), *b);
        }
    }

    /// Put a ROM image at `0x0000`.
    ///
    /// Longer than 16K is refused rather than truncated: a 32K image is a 128K
    /// ROM pair, and quietly loading half of one would boot to something that
    /// looked almost right (ticket 0015).
    pub fn load_rom(&mut self, rom: &[u8]) -> Result<(), RomTooLarge> {
        if rom.len() > ROM_SIZE {
            return Err(RomTooLarge { len: rom.len() });
        }
        self.load(0x0000, rom);
        Ok(())
    }

    /// The whole address space, for a snapshot writer or a bulk hash.
    pub fn bytes(&self) -> &[u8; ADDRESS_SPACE] {
        &self.bytes
    }
}

/// A ROM image that will not fit in the 16K the map gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomTooLarge {
    pub len: usize,
}

impl core::fmt::Display for RomTooLarge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "ROM image is {} bytes; the 48K map has room for {ROM_SIZE}",
            self.len
        )
    }
}

impl std::error::Error for RomTooLarge {}

impl Peek for Memory {
    fn peek(&self, addr: u16) -> u8 {
        self.read(addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_below_four_thousand_does_nothing_and_a_poke_does() {
        let mut mem = Memory::new();
        mem.write(0x0000, 0xFF);
        mem.write(0x3FFF, 0xFF);
        assert_eq!(mem.read(0x0000), 0x00);
        assert_eq!(mem.read(0x3FFF), 0x00);

        mem.poke(0x0000, 0xFF);
        assert_eq!(mem.read(0x0000), 0xFF);
    }

    #[test]
    fn ram_starts_where_rom_stops() {
        let mut mem = Memory::new();
        mem.write(0x4000, 0x5A);
        mem.write(0xFFFF, 0xA5);
        assert_eq!(mem.read(0x4000), 0x5A);
        assert_eq!(mem.read(0xFFFF), 0xA5);
    }

    #[test]
    fn a_rom_image_larger_than_the_map_is_refused() {
        let mut mem = Memory::new();
        assert_eq!(mem.load_rom(&[0xC9; ROM_SIZE]), Ok(()));
        assert_eq!(mem.read(0x3FFF), 0xC9);
        assert_eq!(
            mem.load_rom(&[0x00; ROM_SIZE + 1]),
            Err(RomTooLarge { len: ROM_SIZE + 1 })
        );
        // Refused, not half-applied.
        assert_eq!(mem.read(0x0000), 0xC9);
    }

    #[test]
    fn a_load_wraps_at_the_top_of_the_address_space() {
        let mut mem = Memory::new();
        mem.load(0xFFFE, &[1, 2, 3, 4]);
        assert_eq!(mem.read(0xFFFE), 1);
        assert_eq!(mem.read(0xFFFF), 2);
        // Wrapped into ROM, which a poke is allowed to reach.
        assert_eq!(mem.read(0x0000), 3);
        assert_eq!(mem.read(0x0001), 4);
    }
}
