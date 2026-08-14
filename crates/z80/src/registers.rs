//! Register file and flag definitions.

/// Flag bit positions in `F`.
///
/// `Y` and `X` are the undocumented bits 5 and 3. They are copies of bits 5 and
/// 3 of some result value, and real Spectrum code (and every conformance test
/// suite) depends on them, so they are modelled from the start.
pub mod flag {
    pub const S: u8 = 0b1000_0000;
    pub const Z: u8 = 0b0100_0000;
    pub const Y: u8 = 0b0010_0000;
    pub const H: u8 = 0b0001_0000;
    pub const X: u8 = 0b0000_1000;
    pub const P: u8 = 0b0000_0100;
    pub const V: u8 = P; // same bit; parity for logic ops, overflow for arithmetic
    pub const N: u8 = 0b0000_0010;
    pub const C: u8 = 0b0000_0001;

    /// The two undocumented bits, which are copied wholesale from a result.
    pub const XY: u8 = X | Y;
}

/// The eight 8-bit register slots addressed by the `r[]` field of an opcode.
///
/// Slot 6 is `(HL)` in the encoding; it is not a register, so it is absent here
/// and handled by the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg8 {
    B,
    C,
    D,
    E,
    H,
    L,
    A,
    /// Undocumented: high byte of IX, reachable via a DD prefix.
    Ixh,
    Ixl,
    Iyh,
    Iyl,
}

/// 16-bit register pairs addressed by the `rp[]`/`rp2[]` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg16 {
    Bc,
    De,
    Hl,
    Sp,
    Af,
    Ix,
    Iy,
}

/// Which register pair the `HL` slot of an instruction currently refers to.
///
/// A `DD` prefix rewrites `HL` to `IX` (and `(HL)` to `(IX+d)`) for the
/// instruction that follows; `FD` does the same for `IY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Hl,
    Ix,
    Iy,
}

impl Index {
    /// The 16-bit register this index selects.
    pub fn reg16(self) -> Reg16 {
        match self {
            Index::Hl => Reg16::Hl,
            Index::Ix => Reg16::Ix,
            Index::Iy => Reg16::Iy,
        }
    }

    /// Rewrite the `H` and `L` register slots for this index.
    ///
    /// Only applies when the instruction does *not* also use `(index+d)`; the
    /// decoder is responsible for that distinction.
    pub fn map(self, r: Reg8) -> Reg8 {
        match (self, r) {
            (Index::Ix, Reg8::H) => Reg8::Ixh,
            (Index::Ix, Reg8::L) => Reg8::Ixl,
            (Index::Iy, Reg8::H) => Reg8::Iyh,
            (Index::Iy, Reg8::L) => Reg8::Iyl,
            _ => r,
        }
    }

    pub fn is_indexed(self) -> bool {
        self != Index::Hl
    }
}

/// Interrupt mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterruptMode {
    /// Execute the instruction placed on the data bus by the interrupting device.
    Im0,
    /// Restart to 0x0038.
    #[default]
    Im1,
    /// Vector through the table at `I:databus`.
    Im2,
}

/// The complete architectural state of the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regs {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,

    // Shadow set, reached via EX AF,AF' and EXX.
    pub a_: u8,
    pub f_: u8,
    pub b_: u8,
    pub c_: u8,
    pub d_: u8,
    pub e_: u8,
    pub h_: u8,
    pub l_: u8,

    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,

    /// Interrupt vector base.
    pub i: u8,
    /// Memory refresh counter. Bit 7 is held; only bits 0-6 increment.
    pub r: u8,

    /// The undocumented internal pointer, variously called WZ or MEMPTR. It is
    /// observable through `BIT n,(HL)`, which copies its high byte into flags
    /// X and Y.
    pub wz: u16,

    /// Value of `F` as written so far by the instruction currently executing,
    /// or 0 if it has not touched flags. Cleared at every instruction
    /// boundary.
    pub q: u8,

    /// What `q` held at the end of the previous instruction. `SCF` and `CCF`
    /// consult it to decide where their undocumented X/Y bits come from, so it
    /// has to survive the clearing of `q` at the start of the instruction
    /// doing the consulting.
    pub q_prev: u8,

    pub iff1: bool,
    pub iff2: bool,
    pub im: InterruptMode,

    /// True while the CPU is stopped on a `HALT`, executing NOPs until an
    /// interrupt arrives.
    pub halted: bool,
}

impl Default for Regs {
    fn default() -> Self {
        // Power-on state: everything set. The Spectrum ROM does not rely on
        // this, but reset semantics should be explicit rather than zeroes.
        Self {
            a: 0xFF,
            f: 0xFF,
            b: 0xFF,
            c: 0xFF,
            d: 0xFF,
            e: 0xFF,
            h: 0xFF,
            l: 0xFF,
            a_: 0xFF,
            f_: 0xFF,
            b_: 0xFF,
            c_: 0xFF,
            d_: 0xFF,
            e_: 0xFF,
            h_: 0xFF,
            l_: 0xFF,
            ix: 0xFFFF,
            iy: 0xFFFF,
            sp: 0xFFFF,
            pc: 0x0000,
            i: 0x00,
            r: 0x00,
            wz: 0x0000,
            q: 0x00,
            q_prev: 0x00,
            iff1: false,
            iff2: false,
            im: InterruptMode::Im0,
            halted: false,
        }
    }
}

macro_rules! pair {
    ($get:ident, $set:ident, $hi:ident, $lo:ident) => {
        #[inline]
        pub fn $get(&self) -> u16 {
            u16::from_be_bytes([self.$hi, self.$lo])
        }
        #[inline]
        pub fn $set(&mut self, v: u16) {
            let [hi, lo] = v.to_be_bytes();
            self.$hi = hi;
            self.$lo = lo;
        }
    };
}

impl Regs {
    pair!(af, set_af, a, f);
    pair!(bc, set_bc, b, c);
    pair!(de, set_de, d, e);
    pair!(hl, set_hl, h, l);

    #[inline]
    pub fn get8(&self, r: Reg8) -> u8 {
        match r {
            Reg8::B => self.b,
            Reg8::C => self.c,
            Reg8::D => self.d,
            Reg8::E => self.e,
            Reg8::H => self.h,
            Reg8::L => self.l,
            Reg8::A => self.a,
            Reg8::Ixh => (self.ix >> 8) as u8,
            Reg8::Ixl => self.ix as u8,
            Reg8::Iyh => (self.iy >> 8) as u8,
            Reg8::Iyl => self.iy as u8,
        }
    }

    #[inline]
    pub fn set8(&mut self, r: Reg8, v: u8) {
        match r {
            Reg8::B => self.b = v,
            Reg8::C => self.c = v,
            Reg8::D => self.d = v,
            Reg8::E => self.e = v,
            Reg8::H => self.h = v,
            Reg8::L => self.l = v,
            Reg8::A => self.a = v,
            Reg8::Ixh => self.ix = (self.ix & 0x00FF) | (u16::from(v) << 8),
            Reg8::Ixl => self.ix = (self.ix & 0xFF00) | u16::from(v),
            Reg8::Iyh => self.iy = (self.iy & 0x00FF) | (u16::from(v) << 8),
            Reg8::Iyl => self.iy = (self.iy & 0xFF00) | u16::from(v),
        }
    }

    #[inline]
    pub fn get16(&self, r: Reg16) -> u16 {
        match r {
            Reg16::Bc => self.bc(),
            Reg16::De => self.de(),
            Reg16::Hl => self.hl(),
            Reg16::Sp => self.sp,
            Reg16::Af => self.af(),
            Reg16::Ix => self.ix,
            Reg16::Iy => self.iy,
        }
    }

    #[inline]
    pub fn set16(&mut self, r: Reg16, v: u16) {
        match r {
            Reg16::Bc => self.set_bc(v),
            Reg16::De => self.set_de(v),
            Reg16::Hl => self.set_hl(v),
            Reg16::Sp => self.sp = v,
            Reg16::Af => self.set_af(v),
            Reg16::Ix => self.ix = v,
            Reg16::Iy => self.iy = v,
        }
    }

    #[inline]
    pub fn flag(&self, mask: u8) -> bool {
        self.f & mask != 0
    }

    #[inline]
    pub fn set_flag(&mut self, mask: u8, on: bool) {
        if on {
            self.f |= mask;
        } else {
            self.f &= !mask;
        }
    }

    /// `I` and `R` as the 16-bit value the chip puts on the address bus during
    /// the refresh half of an M1 cycle — and leaves there for any internal
    /// cycles that follow it, which is why anything arbitrating on addresses
    /// needs it.
    #[inline]
    pub fn ir(&self) -> u16 {
        (u16::from(self.i) << 8) | u16::from(self.r)
    }

    /// Bump the refresh counter, preserving bit 7 as the real chip does.
    #[inline]
    pub fn bump_r(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7F);
    }

    /// Reset the CPU. Note this is *not* the same as power-on: the alternate
    /// registers and the main register file keep their values on a real reset.
    pub fn reset(&mut self) {
        self.pc = 0x0000;
        self.i = 0x00;
        self.r = 0x00;
        self.iff1 = false;
        self.iff2 = false;
        self.im = InterruptMode::Im0;
        self.halted = false;
        self.wz = 0x0000;
        self.q = 0x00;
        self.q_prev = 0x00;
    }

    pub fn exx(&mut self) {
        core::mem::swap(&mut self.b, &mut self.b_);
        core::mem::swap(&mut self.c, &mut self.c_);
        core::mem::swap(&mut self.d, &mut self.d_);
        core::mem::swap(&mut self.e, &mut self.e_);
        core::mem::swap(&mut self.h, &mut self.h_);
        core::mem::swap(&mut self.l, &mut self.l_);
    }

    pub fn ex_af(&mut self) {
        core::mem::swap(&mut self.a, &mut self.a_);
        core::mem::swap(&mut self.f, &mut self.f_);
    }
}
