//! Flag-setting arithmetic and logic.
//!
//! Every operation here writes `F` and then mirrors it into `Q`. `Q` is the
//! undocumented latch that `SCF`/`CCF` consult when deciding their X/Y bits:
//! if the previous instruction touched flags, those bits come from `A` alone,
//! otherwise they are OR-ed with the flags already present. Instructions that
//! do *not* affect flags leave `Q` clear, which the core arranges by resetting
//! it at the start of every instruction.
//!
//! Note that neither conformance suite in `tests/` actually exercises this.
//! The Fuse suite predates the discovery of `Q` and expects the simpler
//! behaviour; `zexall` was measured to pass with the rule deliberately broken,
//! so its CRCs do not depend on it. The behaviour here follows the published
//! research and is covered only by the unit test below. Raxoft's `z80ccf`
//! test is the real validator, and it needs a Spectrum to run on.

use crate::registers::{Regs, flag};

#[inline]
fn parity(v: u8) -> bool {
    v.count_ones() % 2 == 0
}

/// Copy bits 3 and 5 of `src` into flags X and Y.
#[inline]
fn set_xy(f: &mut u8, src: u8) {
    *f = (*f & !flag::XY) | (src & flag::XY);
}

impl Regs {
    /// Even parity of a byte, exposed for the block I/O group's flag rules.
    #[inline]
    pub fn parity_of(v: u8) -> bool {
        parity(v)
    }

    /// Record the flag write for the benefit of the next `SCF`/`CCF`.
    #[inline]
    fn latch_q(&mut self) {
        self.q = self.f;
    }

    // ---- 8-bit arithmetic ------------------------------------------------

    #[inline]
    pub fn alu_add(&mut self, value: u8) {
        self.add_common(value, 0);
    }

    #[inline]
    pub fn alu_adc(&mut self, value: u8) {
        let carry = u8::from(self.flag(flag::C));
        self.add_common(value, carry);
    }

    fn add_common(&mut self, value: u8, carry: u8) {
        let a = self.a;
        let wide = u16::from(a) + u16::from(value) + u16::from(carry);
        let result = wide as u8;
        let half = (a & 0x0F) + (value & 0x0F) + carry;
        // Overflow: operands agree in sign but the result disagrees with them.
        let overflow = (!(a ^ value) & (a ^ result) & 0x80) != 0;

        let mut f = 0;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if half > 0x0F {
            f |= flag::H;
        }
        if overflow {
            f |= flag::V;
        }
        if wide > 0xFF {
            f |= flag::C;
        }
        set_xy(&mut f, result);

        self.a = result;
        self.f = f;
        self.latch_q();
    }

    #[inline]
    pub fn alu_sub(&mut self, value: u8) {
        let a = self.sub_common(value, 0);
        self.a = a;
    }

    #[inline]
    pub fn alu_sbc(&mut self, value: u8) {
        let carry = u8::from(self.flag(flag::C));
        let a = self.sub_common(value, carry);
        self.a = a;
    }

    /// `CP` is a subtract that discards the result — but its X/Y flags come
    /// from the *operand*, not from the result, which is the one place the
    /// undocumented bits diverge between `SUB` and `CP`.
    #[inline]
    pub fn alu_cp(&mut self, value: u8) {
        self.sub_common(value, 0);
        let mut f = self.f;
        set_xy(&mut f, value);
        self.f = f;
        self.latch_q();
    }

    fn sub_common(&mut self, value: u8, carry: u8) -> u8 {
        let a = self.a;
        let wide = i16::from(a) - i16::from(value) - i16::from(carry);
        let result = wide as u8;
        let half = i16::from(a & 0x0F) - i16::from(value & 0x0F) - i16::from(carry);
        let overflow = ((a ^ value) & (a ^ result) & 0x80) != 0;

        let mut f = flag::N;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if half < 0 {
            f |= flag::H;
        }
        if overflow {
            f |= flag::V;
        }
        if wide < 0 {
            f |= flag::C;
        }
        set_xy(&mut f, result);

        self.f = f;
        self.latch_q();
        result
    }

    #[inline]
    pub fn alu_and(&mut self, value: u8) {
        self.a &= value;
        self.logic_flags(flag::H);
    }

    #[inline]
    pub fn alu_or(&mut self, value: u8) {
        self.a |= value;
        self.logic_flags(0);
    }

    #[inline]
    pub fn alu_xor(&mut self, value: u8) {
        self.a ^= value;
        self.logic_flags(0);
    }

    fn logic_flags(&mut self, extra: u8) {
        let r = self.a;
        let mut f = extra;
        f |= r & flag::S;
        if r == 0 {
            f |= flag::Z;
        }
        if parity(r) {
            f |= flag::P;
        }
        set_xy(&mut f, r);
        self.f = f;
        self.latch_q();
    }

    /// `INC r`. Carry is deliberately preserved.
    #[inline]
    pub fn alu_inc(&mut self, value: u8) -> u8 {
        let result = value.wrapping_add(1);
        let mut f = self.f & flag::C;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if result & 0x0F == 0 {
            f |= flag::H;
        }
        if result == 0x80 {
            f |= flag::V;
        }
        set_xy(&mut f, result);
        self.f = f;
        self.latch_q();
        result
    }

    /// `DEC r`. Carry is deliberately preserved.
    #[inline]
    pub fn alu_dec(&mut self, value: u8) -> u8 {
        let result = value.wrapping_sub(1);
        let mut f = (self.f & flag::C) | flag::N;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if result & 0x0F == 0x0F {
            f |= flag::H;
        }
        if result == 0x7F {
            f |= flag::V;
        }
        set_xy(&mut f, result);
        self.f = f;
        self.latch_q();
        result
    }

    #[inline]
    pub fn alu_neg(&mut self) {
        let value = self.a;
        self.a = 0;
        self.alu_sub(value);
    }

    /// Decimal adjust. The table here is the standard one derived from the
    /// Zilog documentation, extended with the H flag behaviour after `SUB`.
    pub fn alu_daa(&mut self) {
        let a = self.a;
        let n = self.flag(flag::N);
        let mut correction = 0u8;
        let mut carry = self.flag(flag::C);

        if self.flag(flag::H) || (a & 0x0F) > 9 {
            correction |= 0x06;
        }
        if carry || a > 0x99 {
            correction |= 0x60;
            carry = true;
        }

        let result = if n {
            a.wrapping_sub(correction)
        } else {
            a.wrapping_add(correction)
        };

        // H after DAA records the half-borrow/half-carry the correction itself
        // caused, not the one that was there before.
        let half = if n {
            self.flag(flag::H) && (a & 0x0F) < 6
        } else {
            (a & 0x0F) > 9
        };

        let mut f = self.f & flag::N;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if half {
            f |= flag::H;
        }
        if parity(result) {
            f |= flag::P;
        }
        if carry {
            f |= flag::C;
        }
        set_xy(&mut f, result);

        self.a = result;
        self.f = f;
        self.latch_q();
    }

    #[inline]
    pub fn alu_cpl(&mut self) {
        self.a = !self.a;
        let mut f = self.f | flag::H | flag::N;
        set_xy(&mut f, self.a);
        self.f = f;
        self.latch_q();
    }

    /// `SCF`/`CCF` X/Y bits: if the previous instruction wrote flags, the bits
    /// come from `A`; if it did not, they are OR-ed with whatever was in `F`.
    /// That is the `Q` quirk, and CPU test suites check it.
    ///
    /// This reads `q_prev`, not `q`: the core clears `q` when the instruction
    /// starts, so by the time `SCF` runs its own latch is already empty and
    /// only the saved copy still says what the instruction before it did.
    fn scf_ccf_xy(&mut self, mut f: u8) -> u8 {
        let base = if self.q_prev == 0 {
            self.f & flag::XY
        } else {
            0
        };
        f = (f & !flag::XY) | ((self.a & flag::XY) | base);
        f
    }

    #[inline]
    pub fn alu_scf(&mut self) {
        let f = (self.f & (flag::S | flag::Z | flag::P)) | flag::C;
        self.f = self.scf_ccf_xy(f);
        self.latch_q();
    }

    #[inline]
    pub fn alu_ccf(&mut self) {
        let carry = self.flag(flag::C);
        let mut f = self.f & (flag::S | flag::Z | flag::P);
        if carry {
            f |= flag::H;
        }
        if !carry {
            f |= flag::C;
        }
        self.f = self.scf_ccf_xy(f);
        self.latch_q();
    }

    // ---- 16-bit arithmetic -----------------------------------------------

    /// `ADD HL,rr`. S, Z and V are untouched; X/Y come from the result's high
    /// byte.
    pub fn alu_add16(&mut self, lhs: u16, rhs: u16) -> u16 {
        let wide = u32::from(lhs) + u32::from(rhs);
        let result = wide as u16;
        let half = (lhs & 0x0FFF) + (rhs & 0x0FFF);

        let mut f = self.f & (flag::S | flag::Z | flag::V);
        if half > 0x0FFF {
            f |= flag::H;
        }
        if wide > 0xFFFF {
            f |= flag::C;
        }
        set_xy(&mut f, (result >> 8) as u8);
        self.f = f;
        self.latch_q();
        self.wz = lhs.wrapping_add(1);
        result
    }

    pub fn alu_adc16(&mut self, lhs: u16, rhs: u16) -> u16 {
        let carry = u32::from(self.flag(flag::C));
        let wide = u32::from(lhs) + u32::from(rhs) + carry;
        let result = wide as u16;
        let half = (lhs & 0x0FFF) + (rhs & 0x0FFF) + carry as u16;
        let overflow = (!(lhs ^ rhs) & (lhs ^ result) & 0x8000) != 0;

        let mut f = 0;
        f |= (result >> 8) as u8 & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if half > 0x0FFF {
            f |= flag::H;
        }
        if overflow {
            f |= flag::V;
        }
        if wide > 0xFFFF {
            f |= flag::C;
        }
        set_xy(&mut f, (result >> 8) as u8);
        self.f = f;
        self.latch_q();
        self.wz = lhs.wrapping_add(1);
        result
    }

    pub fn alu_sbc16(&mut self, lhs: u16, rhs: u16) -> u16 {
        let carry = i32::from(self.flag(flag::C));
        let wide = i32::from(lhs) - i32::from(rhs) - carry;
        let result = wide as u16;
        let half = i32::from(lhs & 0x0FFF) - i32::from(rhs & 0x0FFF) - carry;
        let overflow = ((lhs ^ rhs) & (lhs ^ result) & 0x8000) != 0;

        let mut f = flag::N;
        f |= (result >> 8) as u8 & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if half < 0 {
            f |= flag::H;
        }
        if overflow {
            f |= flag::V;
        }
        if wide < 0 {
            f |= flag::C;
        }
        set_xy(&mut f, (result >> 8) as u8);
        self.f = f;
        self.latch_q();
        self.wz = lhs.wrapping_add(1);
        result
    }

    // ---- Rotates and shifts ----------------------------------------------

    /// Flags shared by every CB-prefixed rotate and shift.
    fn shift_flags(&mut self, result: u8, carry: bool) {
        let mut f = 0;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if parity(result) {
            f |= flag::P;
        }
        if carry {
            f |= flag::C;
        }
        set_xy(&mut f, result);
        self.f = f;
        self.latch_q();
    }

    pub fn alu_rlc(&mut self, v: u8) -> u8 {
        let r = v.rotate_left(1);
        self.shift_flags(r, v & 0x80 != 0);
        r
    }

    pub fn alu_rrc(&mut self, v: u8) -> u8 {
        let r = v.rotate_right(1);
        self.shift_flags(r, v & 0x01 != 0);
        r
    }

    pub fn alu_rl(&mut self, v: u8) -> u8 {
        let r = (v << 1) | u8::from(self.flag(flag::C));
        self.shift_flags(r, v & 0x80 != 0);
        r
    }

    pub fn alu_rr(&mut self, v: u8) -> u8 {
        let r = (v >> 1) | (u8::from(self.flag(flag::C)) << 7);
        self.shift_flags(r, v & 0x01 != 0);
        r
    }

    pub fn alu_sla(&mut self, v: u8) -> u8 {
        let r = v << 1;
        self.shift_flags(r, v & 0x80 != 0);
        r
    }

    pub fn alu_sra(&mut self, v: u8) -> u8 {
        let r = (v >> 1) | (v & 0x80);
        self.shift_flags(r, v & 0x01 != 0);
        r
    }

    /// Undocumented `SLL`/`SL1`: shift left, feeding in a 1.
    pub fn alu_sll(&mut self, v: u8) -> u8 {
        let r = (v << 1) | 1;
        self.shift_flags(r, v & 0x80 != 0);
        r
    }

    pub fn alu_srl(&mut self, v: u8) -> u8 {
        let r = v >> 1;
        self.shift_flags(r, v & 0x01 != 0);
        r
    }

    /// The accumulator rotates (`RLCA`, `RRCA`, `RLA`, `RRA`) keep S, Z and P,
    /// and take X/Y from the result rather than computing parity.
    fn acc_rotate_flags(&mut self, result: u8, carry: bool) {
        let mut f = self.f & (flag::S | flag::Z | flag::P);
        if carry {
            f |= flag::C;
        }
        set_xy(&mut f, result);
        self.f = f;
        self.latch_q();
    }

    pub fn alu_rlca(&mut self) {
        let v = self.a;
        let r = v.rotate_left(1);
        self.a = r;
        self.acc_rotate_flags(r, v & 0x80 != 0);
    }

    pub fn alu_rrca(&mut self) {
        let v = self.a;
        let r = v.rotate_right(1);
        self.a = r;
        self.acc_rotate_flags(r, v & 0x01 != 0);
    }

    pub fn alu_rla(&mut self) {
        let v = self.a;
        let r = (v << 1) | u8::from(self.flag(flag::C));
        self.a = r;
        self.acc_rotate_flags(r, v & 0x80 != 0);
    }

    pub fn alu_rra(&mut self) {
        let v = self.a;
        let r = (v >> 1) | (u8::from(self.flag(flag::C)) << 7);
        self.a = r;
        self.acc_rotate_flags(r, v & 0x01 != 0);
    }

    // ---- Bit tests --------------------------------------------------------

    /// `BIT n,r`. X/Y come from the tested register.
    pub fn alu_bit(&mut self, bit: u8, value: u8) {
        let masked = value & (1 << bit);
        let mut f = (self.f & flag::C) | flag::H;
        if masked == 0 {
            f |= flag::Z | flag::P;
        }
        f |= masked & flag::S;
        set_xy(&mut f, value);
        self.f = f;
        self.latch_q();
    }

    /// `BIT n,(HL)` and the indexed forms have no register to take X/Y from,
    /// so they leak the high byte of the internal WZ pointer instead.
    pub fn alu_bit_indirect(&mut self, bit: u8, value: u8, wz_high: u8) {
        self.alu_bit(bit, value);
        let mut f = self.f;
        set_xy(&mut f, wz_high);
        self.f = f;
        self.latch_q();
    }

    // ---- Flags for the ED-prefixed block and I/O groups -------------------

    /// Shared flag behaviour for `RLD`/`RRD`, which act on `A` after the digit
    /// rotate.
    pub fn alu_rxd_flags(&mut self) {
        let r = self.a;
        let mut f = self.f & flag::C;
        f |= r & flag::S;
        if r == 0 {
            f |= flag::Z;
        }
        if parity(r) {
            f |= flag::P;
        }
        set_xy(&mut f, r);
        self.f = f;
        self.latch_q();
    }

    /// `IN r,(C)` sets flags from the byte read; the carry is preserved.
    pub fn alu_in_flags(&mut self, value: u8) {
        let mut f = self.f & flag::C;
        f |= value & flag::S;
        if value == 0 {
            f |= flag::Z;
        }
        if parity(value) {
            f |= flag::P;
        }
        set_xy(&mut f, value);
        self.f = f;
        self.latch_q();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Power-on defaults with the flags cleared, so each test starts from a
    /// known F rather than 0xFF.
    fn regs() -> Regs {
        Regs {
            f: 0,
            ..Default::default()
        }
    }

    #[test]
    fn add_sets_half_carry_and_overflow() {
        let mut r = regs();
        r.a = 0x0F;
        r.alu_add(0x01);
        assert_eq!(r.a, 0x10);
        assert!(r.flag(flag::H));
        assert!(!r.flag(flag::V));

        let mut r = regs();
        r.a = 0x7F;
        r.alu_add(0x01);
        assert_eq!(r.a, 0x80);
        assert!(r.flag(flag::V), "0x7F + 1 overflows into the sign bit");
        assert!(r.flag(flag::S));
    }

    #[test]
    fn cp_takes_undocumented_bits_from_the_operand() {
        let mut r = regs();
        r.a = 0x00;
        // Operand has bits 3 and 5 set; the result (0x00 - 0x28) does not.
        r.alu_cp(0x28);
        assert_eq!(r.f & flag::XY, 0x28, "CP copies X/Y from the operand");

        let mut r = regs();
        r.a = 0x00;
        r.alu_sub(0x28);
        assert_eq!(
            r.f & flag::XY,
            r.a & flag::XY,
            "SUB copies X/Y from the result"
        );
    }

    #[test]
    fn daa_corrects_bcd_addition() {
        let mut r = regs();
        r.a = 0x19;
        r.alu_add(0x08); // 0x21 raw, half-carry set
        r.alu_daa();
        assert_eq!(r.a, 0x27, "19 + 8 = 27 in BCD");
        assert!(!r.flag(flag::C));

        let mut r = regs();
        r.a = 0x90;
        r.alu_add(0x20);
        r.alu_daa();
        assert_eq!(r.a, 0x10);
        assert!(r.flag(flag::C), "90 + 20 = 110, carrying out of two digits");
    }

    #[test]
    fn scf_undocumented_bits_depend_on_the_previous_instruction() {
        // Previous instruction wrote flags: X/Y come from A alone.
        let mut r = regs();
        r.a = 0x00;
        r.f = 0xFF;
        r.q_prev = 0xFF;
        r.alu_scf();
        assert_eq!(r.f & flag::XY, 0x00);

        // Previous instruction did not write flags: X/Y are OR-ed in.
        let mut r = regs();
        r.a = 0x00;
        r.f = 0xFF;
        r.q_prev = 0x00;
        r.alu_scf();
        assert_eq!(r.f & flag::XY, flag::XY);
    }

    #[test]
    fn sbc16_borrow_and_overflow() {
        let mut r = regs();
        r.set_flag(flag::C, true);
        let out = r.alu_sbc16(0x0000, 0x0000);
        assert_eq!(out, 0xFFFF);
        assert!(r.flag(flag::C), "0 - 0 - 1 borrows");
        assert!(r.flag(flag::N));
        assert!(r.flag(flag::S));
    }
}
