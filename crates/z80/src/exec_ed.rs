//! The `ED` group: 16-bit loads and arithmetic, interrupt control, the block
//! move/search/IO instructions, and a large expanse of opcodes that do nothing.

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::registers::{InterruptMode, Reg8, Reg16, Regs, flag};

const ED_R_TABLE: [Option<Reg8>; 8] = [
    Some(Reg8::B),
    Some(Reg8::C),
    Some(Reg8::D),
    Some(Reg8::E),
    Some(Reg8::H),
    Some(Reg8::L),
    None,
    Some(Reg8::A),
];

const RP_TABLE: [Reg16; 4] = [Reg16::Bc, Reg16::De, Reg16::Hl, Reg16::Sp];

/// `IM` mode selected by the `y` field. Four of the eight encodings are
/// undocumented duplicates.
const IM_TABLE: [InterruptMode; 8] = [
    InterruptMode::Im0,
    InterruptMode::Im0,
    InterruptMode::Im1,
    InterruptMode::Im2,
    InterruptMode::Im0,
    InterruptMode::Im0,
    InterruptMode::Im1,
    InterruptMode::Im2,
];

impl Cpu {
    pub(crate) fn execute_ed(&mut self, bus: &mut impl Bus, opcode: u8) {
        let x = opcode >> 6;
        let y = (opcode >> 3) & 7;
        let z = opcode & 7;
        let p = y >> 1;
        let q = y & 1;

        match x {
            1 => self.execute_ed_x1(bus, y, z, p, q),
            2 if z <= 3 && y >= 4 => self.execute_block(bus, y - 4, z),
            // Everything else in the ED page is a two-byte NOP. On real
            // silicon these are "NONI" — they suppress interrupt sampling for
            // the next instruction — which matters only for exotic timing
            // tricks and is not modelled yet.
            _ => {}
        }
    }

    fn execute_ed_x1(&mut self, bus: &mut impl Bus, y: u8, z: u8, p: u8, q: u8) {
        match z {
            0 => {
                // IN r[y],(C). y == 6 is the undocumented IN (C): it sets
                // flags and throws the byte away.
                let port = self.regs.bc();
                let value = bus.input_cycle(port);
                self.regs.wz = port.wrapping_add(1);
                self.regs.alu_in_flags(value);
                if let Some(r) = ED_R_TABLE[y as usize] {
                    self.regs.set8(r, value);
                }
            }
            1 => {
                // OUT (C),r[y]. y == 6 outputs zero on NMOS parts (0xFF on
                // CMOS); the Spectrum's Z80 is NMOS.
                let port = self.regs.bc();
                let value = ED_R_TABLE[y as usize].map_or(0, |r| self.regs.get8(r));
                bus.output_cycle(port, value);
                self.regs.wz = port.wrapping_add(1);
            }
            2 => {
                bus.tick(7);
                let hl = self.regs.hl();
                let rhs = self.regs.get16(RP_TABLE[p as usize]);
                let result = if q == 0 {
                    self.regs.alu_sbc16(hl, rhs)
                } else {
                    self.regs.alu_adc16(hl, rhs)
                };
                self.regs.set_hl(result);
            }
            3 => {
                let addr = self.fetch_word(bus);
                let reg = RP_TABLE[p as usize];
                if q == 0 {
                    let [lo, hi] = self.regs.get16(reg).to_le_bytes();
                    bus.write_cycle(addr, lo);
                    bus.write_cycle(addr.wrapping_add(1), hi);
                } else {
                    let lo = bus.read_cycle(addr);
                    let hi = bus.read_cycle(addr.wrapping_add(1));
                    self.regs.set16(reg, u16::from_le_bytes([lo, hi]));
                }
                self.regs.wz = addr.wrapping_add(1);
            }
            4 => self.regs.alu_neg(),
            5 => {
                // RETN and RETI. Both restore IFF1 from IFF2; they differ only
                // in the bus signalling, which nothing here observes.
                self.regs.iff1 = self.regs.iff2;
                let target = self.pop(bus);
                self.regs.pc = target;
                self.regs.wz = target;
            }
            6 => self.regs.im = IM_TABLE[y as usize],
            _ => self.execute_ed_misc(bus, y),
        }
    }

    fn execute_ed_misc(&mut self, bus: &mut impl Bus, y: u8) {
        match y {
            0 => {
                bus.tick(1);
                self.regs.i = self.regs.a;
            }
            1 => {
                bus.tick(1);
                self.regs.r = self.regs.a;
            }
            2 | 3 => {
                // LD A,I / LD A,R. P/V reflects IFF2, which is how interrupt
                // state is read back.
                bus.tick(1);
                let value = if y == 2 { self.regs.i } else { self.regs.r };
                self.regs.a = value;
                let mut f = self.regs.f & flag::C;
                f |= value & flag::S;
                if value == 0 {
                    f |= flag::Z;
                }
                if self.regs.iff2 {
                    f |= flag::P;
                }
                f |= value & flag::XY;
                self.regs.f = f;
                self.regs.q = f;
            }
            4 | 5 => {
                // RRD / RLD: rotate a BCD digit between A and (HL).
                let addr = self.regs.hl();
                let mem = bus.read_cycle(addr);
                bus.tick(4);
                let (new_a, new_mem) = if y == 4 {
                    // RRD
                    (
                        (self.regs.a & 0xF0) | (mem & 0x0F),
                        (mem >> 4) | (self.regs.a << 4),
                    )
                } else {
                    // RLD
                    (
                        (self.regs.a & 0xF0) | (mem >> 4),
                        (mem << 4) | (self.regs.a & 0x0F),
                    )
                };
                bus.write_cycle(addr, new_mem);
                self.regs.a = new_a;
                self.regs.wz = addr.wrapping_add(1);
                self.regs.alu_rxd_flags();
            }
            _ => {} // ED 77 / ED 7F are NOPs
        }
    }

    /// The block instructions. `op` selects LDx/CPx/INx/OUTx and `kind`
    /// selects increment/decrement/repeat:
    ///
    /// ```text
    ///   kind 0 = increment once      kind 2 = increment, repeat
    ///   kind 1 = decrement once      kind 3 = decrement, repeat
    /// ```
    fn execute_block(&mut self, bus: &mut impl Bus, kind: u8, op: u8) {
        let decrement = kind & 1 != 0;
        let repeating = kind & 2 != 0;
        let delta: u16 = if decrement { 0xFFFF } else { 0x0001 };

        match op {
            0 => self.block_load(bus, delta, repeating),
            1 => self.block_compare(bus, delta, repeating),
            2 => self.block_in(bus, delta, repeating),
            _ => self.block_out(bus, delta, repeating),
        }
    }

    /// `LDI`/`LDD`/`LDIR`/`LDDR`.
    fn block_load(&mut self, bus: &mut impl Bus, delta: u16, repeating: bool) {
        let hl = self.regs.hl();
        let de = self.regs.de();
        let value = bus.read_cycle(hl);
        bus.write_cycle(de, value);
        bus.tick(2);

        self.regs.set_hl(hl.wrapping_add(delta));
        self.regs.set_de(de.wrapping_add(delta));
        let bc = self.regs.bc().wrapping_sub(1);
        self.regs.set_bc(bc);

        // The undocumented bits come from A plus the byte just moved: bit 1 of
        // the sum lands in Y and bit 3 in X.
        let n = self.regs.a.wrapping_add(value);
        let mut f = self.regs.f & (flag::S | flag::Z | flag::C);
        if bc != 0 {
            f |= flag::V;
        }
        if n & 0x02 != 0 {
            f |= flag::Y;
        }
        if n & 0x08 != 0 {
            f |= flag::X;
        }
        self.regs.f = f;
        self.regs.q = f;

        if repeating && bc != 0 {
            bus.tick(5);
            self.regs.pc = self.regs.pc.wrapping_sub(2);
            self.regs.wz = self.regs.pc.wrapping_add(1);
        }
    }

    /// `CPI`/`CPD`/`CPIR`/`CPDR`.
    fn block_compare(&mut self, bus: &mut impl Bus, delta: u16, repeating: bool) {
        let hl = self.regs.hl();
        let value = bus.read_cycle(hl);
        bus.tick(5);

        self.regs.set_hl(hl.wrapping_add(delta));
        let bc = self.regs.bc().wrapping_sub(1);
        self.regs.set_bc(bc);

        let a = self.regs.a;
        let result = a.wrapping_sub(value);
        let half_borrow = (a & 0x0F) < (value & 0x0F);

        let mut f = (self.regs.f & flag::C) | flag::N;
        f |= result & flag::S;
        if result == 0 {
            f |= flag::Z;
        }
        if half_borrow {
            f |= flag::H;
        }
        if bc != 0 {
            f |= flag::V;
        }
        // X/Y come from the result with the half-borrow subtracted out.
        let n = result.wrapping_sub(u8::from(half_borrow));
        if n & 0x02 != 0 {
            f |= flag::Y;
        }
        if n & 0x08 != 0 {
            f |= flag::X;
        }
        self.regs.f = f;
        self.regs.q = f;

        if repeating && bc != 0 && result != 0 {
            bus.tick(5);
            self.regs.pc = self.regs.pc.wrapping_sub(2);
            self.regs.wz = self.regs.pc.wrapping_add(1);
        } else {
            self.regs.wz = self.regs.wz.wrapping_add(delta);
        }
    }

    /// `INI`/`IND`/`INIR`/`INDR`.
    fn block_in(&mut self, bus: &mut impl Bus, delta: u16, repeating: bool) {
        bus.tick(1);
        let port = self.regs.bc();
        let value = bus.input_cycle(port);
        self.regs.wz = port.wrapping_add(delta);

        let hl = self.regs.hl();
        bus.write_cycle(hl, value);
        self.regs.set_hl(hl.wrapping_add(delta));

        let b = self.regs.b.wrapping_sub(1);
        self.regs.b = b;

        self.block_io_flags(value, self.regs.c.wrapping_add(delta as u8), b);

        if repeating && b != 0 {
            bus.tick(5);
            self.regs.pc = self.regs.pc.wrapping_sub(2);
        }
    }

    /// `OUTI`/`OUTD`/`OTIR`/`OTDR`.
    fn block_out(&mut self, bus: &mut impl Bus, delta: u16, repeating: bool) {
        bus.tick(1);
        let hl = self.regs.hl();
        let value = bus.read_cycle(hl);

        // B is decremented before the port is driven, so the port's high byte
        // is the *new* B.
        let b = self.regs.b.wrapping_sub(1);
        self.regs.b = b;

        let port = self.regs.bc();
        bus.output_cycle(port, value);
        self.regs.set_hl(hl.wrapping_add(delta));
        self.regs.wz = port.wrapping_add(delta);

        self.block_io_flags(value, self.regs.l, b);

        if repeating && b != 0 {
            bus.tick(5);
            self.regs.pc = self.regs.pc.wrapping_sub(2);
        }
    }

    /// The shared — and famously strange — flag rules for the block I/O group.
    ///
    /// `k` is the byte transferred plus a second byte that differs between the
    /// IN and OUT forms (`C±1` for IN, `L` after adjustment for OUT). It sets
    /// both H and C on overflow, and its low three bits are XOR-ed with `B` to
    /// produce the parity flag.
    fn block_io_flags(&mut self, value: u8, addend: u8, b: u8) {
        let k = u16::from(value) + u16::from(addend);

        let mut f = 0u8;
        f |= b & flag::S;
        if b == 0 {
            f |= flag::Z;
        }
        f |= b & flag::XY;
        if value & 0x80 != 0 {
            f |= flag::N;
        }
        if k > 0xFF {
            f |= flag::H | flag::C;
        }
        if Regs::parity_of(((k & 0x07) as u8) ^ b) {
            f |= flag::P;
        }
        self.regs.f = f;
        self.regs.q = f;
    }
}
