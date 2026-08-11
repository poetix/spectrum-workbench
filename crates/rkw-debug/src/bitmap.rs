//! One bit per address.

/// 65536 bits, one per address in the Z80's space.
///
/// 8 KB, which is a quarter of a small x86 L1d and sounds worse than it is:
/// the bitmap is touched one bit per accessed address, so only the lines
/// covering current activity become resident. It is address space, not working
/// set (ADR-0008).
#[derive(Clone)]
pub struct Bitmap(Box<[u64; Bitmap::WORDS]>);

impl Bitmap {
    const WORDS: usize = 0x1_0000 / 64;

    pub fn new() -> Self {
        Bitmap(Box::new([0; Self::WORDS]))
    }

    /// The whole point: one shift, one mask, one test.
    #[inline]
    pub fn test(&self, addr: u16) -> bool {
        self.0[addr as usize >> 6] & (1u64 << (addr & 63)) != 0
    }

    #[inline]
    pub fn set(&mut self, addr: u16, on: bool) {
        let word = &mut self.0[addr as usize >> 6];
        let bit = 1u64 << (addr & 63);
        if on {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    pub fn clear(&mut self) {
        self.0.fill(0);
    }

    /// How many addresses are armed. For `info`-style output and for tests
    /// that want to know the bitmap agrees with the map.
    pub fn count(&self) -> u32 {
        self.0.iter().map(|w| w.count_ones()).sum()
    }
}

impl Default for Bitmap {
    fn default() -> Self {
        Self::new()
    }
}
