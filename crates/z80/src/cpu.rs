//! The CPU core: fetch, decode, execute.
//!
//! Decoding follows the octal decomposition of the opcode byte rather than a
//! flat 256-arm match. Every Z80 opcode splits into
//!
//! ```text
//!   7 6   5 4 3   2 1 0
//!  +-----+-------+-------+
//!  |  x  |   y   |   z   |      p = y >> 1,  q = y & 1
//!  +-----+-------+-------+
//! ```
//!
//! and the instruction set is regular in those fields. This keeps the whole
//! unprefixed set in about a hundred lines and makes the undocumented opcodes
//! fall out for free instead of needing to be enumerated.

use crate::bus::Bus;
use crate::registers::{Index, InterruptMode, Reg8, Reg16, Regs, flag};

/// Register slots for the `r[]` field. Slot 6 is `(HL)` and is `None`.
const R_TABLE: [Option<Reg8>; 8] = [
    Some(Reg8::B),
    Some(Reg8::C),
    Some(Reg8::D),
    Some(Reg8::E),
    Some(Reg8::H),
    Some(Reg8::L),
    None,
    Some(Reg8::A),
];

/// Register pairs for the `rp[]` field, where slot 3 is `SP`.
const RP_TABLE: [Reg16; 4] = [Reg16::Bc, Reg16::De, Reg16::Hl, Reg16::Sp];

/// Register pairs for the `rp2[]` field, used by `PUSH`/`POP`, where slot 3 is
/// `AF`.
const RP2_TABLE: [Reg16; 4] = [Reg16::Bc, Reg16::De, Reg16::Hl, Reg16::Af];

/// Why a `step` returned, for the benefit of a debugger driving the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// One instruction executed normally.
    Instruction,
    /// The instruction was a `HALT` and the CPU is now idling.
    Halt,
    /// An interrupt was accepted before the instruction ran.
    Interrupt,
    /// A non-maskable interrupt was accepted.
    Nmi,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Cpu {
    pub regs: Regs,
    /// Set by `EI`, cleared after the following instruction. Interrupts are not
    /// sampled while this is true, which is what makes the `EI` / `HALT` idiom
    /// work.
    pub ei_pending: bool,
    /// Instructions retired since reset. Useful for debugger stepping and for
    /// bisecting conformance failures.
    pub instructions: u64,
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.ei_pending = false;
    }

    /// Run one instruction, taking any pending interrupt first.
    pub fn step(&mut self, bus: &mut impl Bus) -> Stop {
        if bus.nmi_pending() {
            self.take_nmi(bus);
            return Stop::Nmi;
        }
        if self.regs.iff1 && !self.ei_pending && bus.interrupt_pending() {
            self.take_interrupt(bus);
            return Stop::Interrupt;
        }

        let was_ei_pending = self.ei_pending;

        if self.regs.halted {
            // A halted CPU still fetches, still refreshes, still burns 4T.
            bus.fetch_opcode(self.regs.pc);
            self.regs.bump_r();
            if was_ei_pending {
                self.ei_pending = false;
            }
            return Stop::Halt;
        }

        // Roll the Q latch over: whatever the last instruction wrote to F is
        // now the "previous" value that SCF and CCF may consult, and this
        // instruction starts with nothing recorded. Any ALU helper it calls
        // will set `q` again.
        self.regs.q_prev = self.regs.q;
        self.regs.q = 0;

        let opcode = self.fetch_opcode(bus);
        self.execute(bus, opcode, Index::Hl);
        self.instructions += 1;

        if was_ei_pending {
            self.ei_pending = false;
        }
        if self.regs.halted {
            Stop::Halt
        } else {
            Stop::Instruction
        }
    }

    // ---- Fetch helpers ----------------------------------------------------

    fn fetch_opcode(&mut self, bus: &mut impl Bus) -> u8 {
        let op = bus.fetch_opcode(self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        self.regs.bump_r();
        op
    }

    fn fetch_byte(&mut self, bus: &mut impl Bus) -> u8 {
        let v = bus.read_cycle(self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        v
    }

    /// The address of the operand byte most recently fetched — `PC` has already
    /// moved past it — which is what stays on the address bus through any
    /// internal cycles that follow the fetch.
    fn operand_addr(&self) -> u16 {
        self.regs.pc.wrapping_sub(1)
    }

    pub(crate) fn fetch_word(&mut self, bus: &mut impl Bus) -> u16 {
        let lo = self.fetch_byte(bus);
        let hi = self.fetch_byte(bus);
        u16::from_le_bytes([lo, hi])
    }

    fn push(&mut self, bus: &mut impl Bus, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write_cycle(self.regs.sp, hi);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write_cycle(self.regs.sp, lo);
    }

    pub(crate) fn pop(&mut self, bus: &mut impl Bus) -> u16 {
        let lo = bus.read_cycle(self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        let hi = bus.read_cycle(self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }

    // ---- Interrupts -------------------------------------------------------
    //
    // The wait states of an acknowledge are the one place `tick` is still the
    // right call rather than `tick_at`. The cycle asserts `M1` and `IORQ`
    // together, which is a combination no memory cycle produces and which the
    // ULA does not arbitrate on, so nothing stalls the CPU here however the
    // address bus happens to be sitting. The pushes that follow are ordinary
    // write cycles and are contended like any other.

    fn leave_halt(&mut self) {
        if self.regs.halted {
            self.regs.halted = false;
            self.regs.pc = self.regs.pc.wrapping_add(1);
        }
    }

    fn take_nmi(&mut self, bus: &mut impl Bus) {
        self.leave_halt();
        self.regs.bump_r();
        self.regs.iff1 = false; // iff2 is preserved, and RETN restores from it
        bus.tick(5);
        let pc = self.regs.pc;
        self.push(bus, pc);
        self.regs.pc = 0x0066;
        self.regs.wz = 0x0066;
    }

    fn take_interrupt(&mut self, bus: &mut impl Bus) {
        self.leave_halt();
        self.regs.bump_r();
        self.regs.iff1 = false;
        self.regs.iff2 = false;

        let data = bus.interrupt_data();

        match self.regs.im {
            InterruptMode::Im0 => {
                // The device supplies an instruction. In practice that is
                // always an RST on real hardware, and on the Spectrum the bus
                // floats to 0xFF, which is RST 38h.
                bus.tick(6);
                let pc = self.regs.pc;
                self.push(bus, pc);
                let target = u16::from(data & 0x38);
                self.regs.pc = target;
                self.regs.wz = target;
            }
            InterruptMode::Im1 => {
                bus.tick(7);
                let pc = self.regs.pc;
                self.push(bus, pc);
                self.regs.pc = 0x0038;
                self.regs.wz = 0x0038;
            }
            InterruptMode::Im2 => {
                bus.tick(7);
                let pc = self.regs.pc;
                self.push(bus, pc);
                let vector = (u16::from(self.regs.i) << 8) | u16::from(data);
                let lo = bus.read_cycle(vector);
                let hi = bus.read_cycle(vector.wrapping_add(1));
                let target = u16::from_le_bytes([lo, hi]);
                self.regs.pc = target;
                self.regs.wz = target;
            }
        }
    }

    // ---- Operand access ---------------------------------------------------

    /// Resolve the `(HL)` operand slot, applying an index prefix if one is
    /// active. Returns the effective address and consumes the displacement
    /// byte plus its five internal T-states when indexed.
    fn indexed_addr(&mut self, bus: &mut impl Bus, index: Index) -> u16 {
        match index {
            Index::Hl => self.regs.hl(),
            Index::Ix | Index::Iy => {
                let d = self.fetch_byte(bus) as i8;
                bus.tick_at(self.operand_addr(), 5);
                let base = self.regs.get16(index.reg16());
                let addr = base.wrapping_add(d as u16);
                self.regs.wz = addr;
                addr
            }
        }
    }

    /// Read register slot `slot`, going to memory for slot 6.
    ///
    /// `addr` must already have been resolved by the caller when the slot may
    /// be 6, because the displacement byte has to be fetched at a specific
    /// point in the instruction's byte order.
    fn read_slot(&mut self, bus: &mut impl Bus, slot: u8, index: Index, addr: u16) -> u8 {
        match R_TABLE[slot as usize] {
            Some(r) => self.regs.get8(index.map(r)),
            None => bus.read_cycle(addr),
        }
    }

    fn write_slot(&mut self, bus: &mut impl Bus, slot: u8, index: Index, addr: u16, value: u8) {
        match R_TABLE[slot as usize] {
            Some(r) => self.regs.set8(index.map(r), value),
            None => bus.write_cycle(addr, value),
        }
    }

    fn condition(&self, y: u8) -> bool {
        match y {
            0 => !self.regs.flag(flag::Z),
            1 => self.regs.flag(flag::Z),
            2 => !self.regs.flag(flag::C),
            3 => self.regs.flag(flag::C),
            4 => !self.regs.flag(flag::P),
            5 => self.regs.flag(flag::P),
            6 => !self.regs.flag(flag::S),
            _ => self.regs.flag(flag::S),
        }
    }

    fn apply_alu(&mut self, op: u8, value: u8) {
        match op {
            0 => self.regs.alu_add(value),
            1 => self.regs.alu_adc(value),
            2 => self.regs.alu_sub(value),
            3 => self.regs.alu_sbc(value),
            4 => self.regs.alu_and(value),
            5 => self.regs.alu_xor(value),
            6 => self.regs.alu_or(value),
            _ => self.regs.alu_cp(value),
        }
    }

    fn jump_relative(&mut self, bus: &mut impl Bus, offset: i8) {
        bus.tick_at(self.operand_addr(), 5);
        let target = self.regs.pc.wrapping_add(offset as u16);
        self.regs.pc = target;
        self.regs.wz = target;
    }

    // ---- Execute ----------------------------------------------------------

    fn execute(&mut self, bus: &mut impl Bus, opcode: u8, index: Index) {
        let x = opcode >> 6;
        let y = (opcode >> 3) & 7;
        let z = opcode & 7;
        let p = y >> 1;
        let q = y & 1;

        // Q is cleared once per instruction in `step`, not here: this function
        // re-enters itself for the DD and FD prefixes, and a reset at this
        // point would wipe the latch partway through.
        match x {
            0 => self.execute_x0(bus, index, y, z, p, q),
            1 => self.execute_load(bus, index, y, z),
            2 => {
                let addr = if z == 6 {
                    self.indexed_addr(bus, index)
                } else {
                    0
                };
                let value = self.read_slot(bus, z, index, addr);
                self.apply_alu(y, value);
            }
            _ => self.execute_x3(bus, index, y, z, p, q),
        }
    }

    fn execute_x0(&mut self, bus: &mut impl Bus, index: Index, y: u8, z: u8, p: u8, q: u8) {
        match z {
            0 => match y {
                0 => {} // NOP
                1 => self.regs.ex_af(),
                2 => {
                    // DJNZ d: the decrement costs one extra T-state on the
                    // opcode fetch, before the displacement is read.
                    bus.tick_at(self.regs.ir(), 1);
                    let d = self.fetch_byte(bus) as i8;
                    self.regs.b = self.regs.b.wrapping_sub(1);
                    if self.regs.b != 0 {
                        self.jump_relative(bus, d);
                    }
                }
                3 => {
                    let d = self.fetch_byte(bus) as i8;
                    self.jump_relative(bus, d);
                }
                _ => {
                    let d = self.fetch_byte(bus) as i8;
                    if self.condition(y - 4) {
                        self.jump_relative(bus, d);
                    }
                }
            },
            1 => {
                if q == 0 {
                    // LD rp[p],nn
                    let value = self.fetch_word(bus);
                    self.regs.set16(self.rp(p, index), value);
                } else {
                    // ADD HL,rp[p]
                    bus.tick_at(self.regs.ir(), 7);
                    let target = index.reg16();
                    let lhs = self.regs.get16(target);
                    let rhs = self.regs.get16(self.rp(p, index));
                    let result = self.regs.alu_add16(lhs, rhs);
                    self.regs.set16(target, result);
                }
            }
            2 => self.execute_x0_z2(bus, index, p, q),
            3 => {
                // INC/DEC rp[p]. No flags, two internal T-states.
                bus.tick_at(self.regs.ir(), 2);
                let reg = self.rp(p, index);
                let value = self.regs.get16(reg);
                let result = if q == 0 {
                    value.wrapping_add(1)
                } else {
                    value.wrapping_sub(1)
                };
                self.regs.set16(reg, result);
            }
            4 | 5 => {
                // INC/DEC r[y]. The memory form re-reads, pauses, then writes.
                let addr = if y == 6 {
                    self.indexed_addr(bus, index)
                } else {
                    0
                };
                let value = self.read_slot(bus, y, index, addr);
                if y == 6 {
                    bus.tick_at(addr, 1);
                }
                let result = if z == 4 {
                    self.regs.alu_inc(value)
                } else {
                    self.regs.alu_dec(value)
                };
                self.write_slot(bus, y, index, addr, result);
            }
            6 => {
                // LD r[y],n. For (IX+d) the displacement precedes the operand,
                // and the five internal T-states are folded into the operand
                // fetch rather than paid separately.
                if y == 6 && index.is_indexed() {
                    let d = self.fetch_byte(bus) as i8;
                    let n = self.fetch_byte(bus);
                    bus.tick_at(self.operand_addr(), 2);
                    let addr = self.regs.get16(index.reg16()).wrapping_add(d as u16);
                    self.regs.wz = addr;
                    bus.write_cycle(addr, n);
                } else {
                    let n = self.fetch_byte(bus);
                    let addr = if y == 6 { self.regs.hl() } else { 0 };
                    self.write_slot(bus, y, index, addr, n);
                }
            }
            _ => match y {
                0 => self.regs.alu_rlca(),
                1 => self.regs.alu_rrca(),
                2 => self.regs.alu_rla(),
                3 => self.regs.alu_rra(),
                4 => self.regs.alu_daa(),
                5 => self.regs.alu_cpl(),
                6 => self.regs.alu_scf(),
                _ => self.regs.alu_ccf(),
            },
        }
    }

    fn execute_x0_z2(&mut self, bus: &mut impl Bus, index: Index, p: u8, q: u8) {
        match (q, p) {
            (0, 0) => {
                // LD (BC),A. WZ gets BC+1 in its low byte and A in its high
                // byte, one of the stranger MEMPTR rules.
                let addr = self.regs.bc();
                bus.write_cycle(addr, self.regs.a);
                self.regs.wz = (u16::from(self.regs.a) << 8) | (addr.wrapping_add(1) & 0xFF);
            }
            (0, 1) => {
                let addr = self.regs.de();
                bus.write_cycle(addr, self.regs.a);
                self.regs.wz = (u16::from(self.regs.a) << 8) | (addr.wrapping_add(1) & 0xFF);
            }
            (0, 2) => {
                // LD (nn),HL
                let addr = self.fetch_word(bus);
                let value = self.regs.get16(index.reg16());
                let [lo, hi] = value.to_le_bytes();
                bus.write_cycle(addr, lo);
                bus.write_cycle(addr.wrapping_add(1), hi);
                self.regs.wz = addr.wrapping_add(1);
            }
            (0, 3) => {
                // LD (nn),A
                let addr = self.fetch_word(bus);
                bus.write_cycle(addr, self.regs.a);
                self.regs.wz = (u16::from(self.regs.a) << 8) | (addr.wrapping_add(1) & 0xFF);
            }
            (1, 0) => {
                let addr = self.regs.bc();
                self.regs.a = bus.read_cycle(addr);
                self.regs.wz = addr.wrapping_add(1);
            }
            (1, 1) => {
                let addr = self.regs.de();
                self.regs.a = bus.read_cycle(addr);
                self.regs.wz = addr.wrapping_add(1);
            }
            (1, 2) => {
                // LD HL,(nn)
                let addr = self.fetch_word(bus);
                let lo = bus.read_cycle(addr);
                let hi = bus.read_cycle(addr.wrapping_add(1));
                self.regs.set16(index.reg16(), u16::from_le_bytes([lo, hi]));
                self.regs.wz = addr.wrapping_add(1);
            }
            _ => {
                // LD A,(nn)
                let addr = self.fetch_word(bus);
                self.regs.a = bus.read_cycle(addr);
                self.regs.wz = addr.wrapping_add(1);
            }
        }
    }

    fn execute_load(&mut self, bus: &mut impl Bus, index: Index, y: u8, z: u8) {
        if y == 6 && z == 6 {
            // HALT. PC stays on the HALT instruction; the core re-fetches it
            // every 4T until an interrupt arrives.
            self.regs.halted = true;
            self.regs.pc = self.regs.pc.wrapping_sub(1);
            return;
        }

        // When one operand is (index+d), the other keeps its unindexed
        // meaning: DD 74 is LD (IX+d),H, not LD (IX+d),IXH.
        let memory_operand = y == 6 || z == 6;
        let addr = if memory_operand {
            self.indexed_addr(bus, index)
        } else {
            0
        };
        let src_index = if memory_operand { Index::Hl } else { index };

        let value = self.read_slot(bus, z, src_index, addr);
        self.write_slot(bus, y, src_index, addr, value);
    }

    fn execute_x3(&mut self, bus: &mut impl Bus, index: Index, y: u8, z: u8, p: u8, q: u8) {
        match z {
            0 => {
                // RET cc
                bus.tick_at(self.regs.ir(), 1);
                if self.condition(y) {
                    let target = self.pop(bus);
                    self.regs.pc = target;
                    self.regs.wz = target;
                }
            }
            1 => {
                if q == 0 {
                    let value = self.pop(bus);
                    self.regs.set16(self.rp2(p, index), value);
                } else {
                    match p {
                        0 => {
                            let target = self.pop(bus);
                            self.regs.pc = target;
                            self.regs.wz = target;
                        }
                        1 => self.regs.exx(),
                        2 => self.regs.pc = self.regs.get16(index.reg16()), // JP (HL)
                        _ => {
                            // LD SP,HL
                            bus.tick_at(self.regs.ir(), 2);
                            self.regs.sp = self.regs.get16(index.reg16());
                        }
                    }
                }
            }
            2 => {
                // JP cc,nn. WZ takes the operand whether or not the jump is
                // taken.
                let target = self.fetch_word(bus);
                self.regs.wz = target;
                if self.condition(y) {
                    self.regs.pc = target;
                }
            }
            3 => match y {
                0 => {
                    let target = self.fetch_word(bus);
                    self.regs.pc = target;
                    self.regs.wz = target;
                }
                1 => {
                    let op = self.fetch_cb_opcode(bus, index);
                    self.execute_cb(bus, op, index);
                }
                2 => {
                    // OUT (n),A. The port's high byte is A.
                    let n = self.fetch_byte(bus);
                    let port = (u16::from(self.regs.a) << 8) | u16::from(n);
                    bus.output_cycle(port, self.regs.a);
                    self.regs.wz =
                        (u16::from(self.regs.a) << 8) | (u16::from(n).wrapping_add(1) & 0xFF);
                }
                3 => {
                    let n = self.fetch_byte(bus);
                    let port = (u16::from(self.regs.a) << 8) | u16::from(n);
                    self.regs.a = bus.input_cycle(port);
                    self.regs.wz = port.wrapping_add(1);
                }
                4 => {
                    // EX (SP),HL
                    let sp = self.regs.sp;
                    let lo = bus.read_cycle(sp);
                    let hi = bus.read_cycle(sp.wrapping_add(1));
                    bus.tick_at(sp.wrapping_add(1), 1);
                    let old = self.regs.get16(index.reg16());
                    let [old_lo, old_hi] = old.to_le_bytes();
                    bus.write_cycle(sp.wrapping_add(1), old_hi);
                    bus.write_cycle(sp, old_lo);
                    bus.tick_at(sp, 2);
                    let value = u16::from_le_bytes([lo, hi]);
                    self.regs.set16(index.reg16(), value);
                    self.regs.wz = value;
                }
                5 => {
                    // EX DE,HL is never affected by a DD/FD prefix.
                    let de = self.regs.de();
                    let hl = self.regs.hl();
                    self.regs.set_de(hl);
                    self.regs.set_hl(de);
                }
                6 => {
                    self.regs.iff1 = false;
                    self.regs.iff2 = false;
                }
                _ => {
                    self.regs.iff1 = true;
                    self.regs.iff2 = true;
                    self.ei_pending = true;
                }
            },
            4 => {
                // CALL cc,nn
                let target = self.fetch_word(bus);
                self.regs.wz = target;
                if self.condition(y) {
                    bus.tick_at(self.operand_addr(), 1);
                    let pc = self.regs.pc;
                    self.push(bus, pc);
                    self.regs.pc = target;
                }
            }
            5 => {
                if q == 0 {
                    // PUSH rp2[p]
                    bus.tick_at(self.regs.ir(), 1);
                    let value = self.regs.get16(self.rp2(p, index));
                    self.push(bus, value);
                } else {
                    match p {
                        0 => {
                            let target = self.fetch_word(bus);
                            bus.tick_at(self.operand_addr(), 1);
                            let pc = self.regs.pc;
                            self.push(bus, pc);
                            self.regs.pc = target;
                            self.regs.wz = target;
                        }
                        1 => {
                            // DD prefix. A prefix is a whole instruction as far
                            // as R and interrupts are concerned.
                            let op = self.fetch_opcode(bus);
                            self.execute(bus, op, Index::Ix);
                        }
                        2 => {
                            let op = self.fetch_opcode(bus);
                            self.execute_ed(bus, op);
                        }
                        _ => {
                            let op = self.fetch_opcode(bus);
                            self.execute(bus, op, Index::Iy);
                        }
                    }
                }
            }
            6 => {
                let n = self.fetch_byte(bus);
                self.apply_alu(y, n);
            }
            _ => {
                // RST y*8
                bus.tick_at(self.regs.ir(), 1);
                let pc = self.regs.pc;
                self.push(bus, pc);
                let target = u16::from(y) * 8;
                self.regs.pc = target;
                self.regs.wz = target;
            }
        }
    }

    /// `rp[]` with the index prefix applied: `DD 21` is `LD IX,nn`.
    fn rp(&self, p: u8, index: Index) -> Reg16 {
        let base = RP_TABLE[p as usize];
        if base == Reg16::Hl {
            index.reg16()
        } else {
            base
        }
    }

    fn rp2(&self, p: u8, index: Index) -> Reg16 {
        let base = RP2_TABLE[p as usize];
        if base == Reg16::Hl {
            index.reg16()
        } else {
            base
        }
    }

    // Prefixed groups live in their own modules; these are the entry points.

    fn fetch_cb_opcode(&mut self, bus: &mut impl Bus, index: Index) -> u8 {
        if index.is_indexed() {
            // DD CB d op: the displacement comes before the opcode, and the
            // opcode byte is *not* an M1 cycle, so R does not increment.
            let d = self.fetch_byte(bus) as i8;
            let base = self.regs.get16(index.reg16());
            self.regs.wz = base.wrapping_add(d as u16);
            let op = self.fetch_byte(bus);
            bus.tick_at(self.operand_addr(), 2);
            op
        } else {
            self.fetch_opcode(bus)
        }
    }
}
