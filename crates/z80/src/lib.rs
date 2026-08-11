//! A Z80 CPU core.
//!
//! The core knows nothing about the Spectrum. It talks to a [`Bus`], which it
//! drives one machine cycle at a time so that a host with contended memory can
//! account for wait states without the instruction implementations changing.
//!
//! ```
//! use z80::{Bus, Cpu, FlatMemory};
//!
//! let mut mem = FlatMemory::new();
//! mem.load(0x8000, &[0x3E, 0x2A, 0x47]); // LD A,42 ; LD B,A
//!
//! let mut cpu = Cpu::new();
//! cpu.regs.pc = 0x8000;
//! cpu.step(&mut mem);
//! cpu.step(&mut mem);
//!
//! assert_eq!(cpu.regs.b, 42);
//! ```

mod alu;
mod bus;
mod cpu;
pub mod disasm;
mod exec_cb;
mod exec_ed;
mod registers;

pub use bus::{Bus, FlatMemory};
pub use cpu::{Cpu, Stop};
pub use disasm::{Flow, Instruction, Peek, disassemble, disassemble_range};
pub use registers::{Index, InterruptMode, Reg8, Reg16, Regs, flag};
