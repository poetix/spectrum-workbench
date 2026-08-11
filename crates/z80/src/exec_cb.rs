//! The `CB` group: rotates, shifts, and bit operations.
//!
//! The indexed forms (`DD CB d op`) have an undocumented twist: the operation
//! is performed on `(IX+d)` and the result is written back there *and* copied
//! into the register named by the `z` field. Real code — including some
//! Spectrum demos — uses this deliberately, so it is modelled rather than
//! treated as an error.

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::registers::{Index, Reg8};

const CB_R_TABLE: [Option<Reg8>; 8] = [
    Some(Reg8::B),
    Some(Reg8::C),
    Some(Reg8::D),
    Some(Reg8::E),
    Some(Reg8::H),
    Some(Reg8::L),
    None,
    Some(Reg8::A),
];

impl Cpu {
    pub(crate) fn execute_cb(&mut self, bus: &mut impl Bus, opcode: u8, index: Index) {
        let x = opcode >> 6;
        let y = (opcode >> 3) & 7;
        let z = opcode & 7;

        // For the indexed forms WZ already holds the effective address; it was
        // computed when the displacement was fetched.
        let indexed = index.is_indexed();
        let addr = if indexed {
            self.regs.wz
        } else {
            self.regs.hl()
        };
        let use_memory = indexed || z == 6;

        let value = if use_memory {
            bus.read_cycle(addr)
        } else {
            self.regs.get8(CB_R_TABLE[z as usize].unwrap())
        };

        if x == 1 {
            // BIT. No write-back, and the indexed and (HL) forms take their
            // undocumented flag bits from WZ rather than from a register.
            if use_memory {
                bus.tick(1);
                self.regs.alu_bit_indirect(y, value, (self.regs.wz >> 8) as u8);
            } else {
                self.regs.alu_bit(y, value);
            }
            return;
        }

        let result = match x {
            0 => match y {
                0 => self.regs.alu_rlc(value),
                1 => self.regs.alu_rrc(value),
                2 => self.regs.alu_rl(value),
                3 => self.regs.alu_rr(value),
                4 => self.regs.alu_sla(value),
                5 => self.regs.alu_sra(value),
                6 => self.regs.alu_sll(value),
                _ => self.regs.alu_srl(value),
            },
            2 => value & !(1 << y), // RES
            _ => value | (1 << y),  // SET
        };

        if use_memory {
            bus.tick(1);
            bus.write_cycle(addr, result);
            // The undocumented copy: DD CB d 06 (z == 6) is the documented
            // form and writes nowhere else.
            if indexed && z != 6 {
                self.regs.set8(CB_R_TABLE[z as usize].unwrap(), result);
            }
        } else {
            self.regs.set8(CB_R_TABLE[z as usize].unwrap(), result);
        }
    }
}
