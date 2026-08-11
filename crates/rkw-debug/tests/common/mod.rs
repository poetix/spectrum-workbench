#![allow(dead_code)]
//! Shared test helpers: a machine with a program in it.

use z80::{Cpu, FlatMemory};

/// Where test programs are assembled to, chosen to be well away from the
/// restart vectors so a stray `RST` is obvious.
pub const ORG: u16 = 0x8000;

/// Where the stack starts. Every step-out and step-over test is really a test
/// about `SP`, so it is worth having one place that says what it began as.
pub const STACK: u16 = 0xFF00;

/// A CPU sitting at [`ORG`] with `bytes` loaded there.
pub fn machine(bytes: &[u8]) -> (Cpu, FlatMemory) {
    let mut mem = FlatMemory::new();
    mem.load(ORG, bytes);
    let mut cpu = Cpu::new();
    cpu.regs.pc = ORG;
    cpu.regs.sp = STACK;
    (cpu, mem)
}

/// Enough instructions for any test program here to finish, and few enough
/// that a program that fails to finish says so instead of hanging.
pub const BUDGET: u64 = 10_000;
