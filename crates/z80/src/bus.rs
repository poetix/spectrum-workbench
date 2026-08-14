//! The CPU's view of the outside world.
//!
//! The core never adds up T-states itself. Every access is issued as a discrete
//! machine cycle with its real duration, and purely internal cycles are issued
//! as explicit [`Bus::tick_at`] calls carrying the address the CPU is holding
//! while they run. That is what makes it possible to bolt the Spectrum's ULA
//! contention onto this core: contention is a function of *when* an access
//! happens and *what address* it is to, so the timing has to be right before
//! the quirks can be.
//!
//! # Why an internal cycle has an address
//!
//! ADR-0002 gave internal cycles a bare `tick(n)`, on the reasoning that the
//! CPU is not driving the bus in a way anything can see. That is not true of
//! this machine. The Z80 leaves the last address it used on the address bus for
//! the whole of an internal cycle, and the ULA arbitrates on the address lines
//! alone: it cannot tell an internal cycle from a read, so it stalls the CPU on
//! each of those T-states individually if the address happens to be in the
//! contended bank. `INC (HL)` with `HL` in the lower 16K is contended four
//! times, not three, and `LDIR` copying within it is contended seven times per
//! iteration.
//!
//! Which address is held is instruction-local knowledge — `IR` after an opcode
//! fetch, the operand address after a memory cycle, `PC` after a displacement
//! byte — so the bus cannot recover it and the core has to state it. See
//! ADR-0023.

/// Everything the CPU can do to the rest of the machine.
///
/// Implementors provide the four raw accessors plus [`Bus::tick`]; the machine
/// cycle wrappers have default bodies that spend the standard number of
/// T-states. A contended implementation overrides the wrappers instead, keeping
/// the timing decisions in one place.
///
/// A *decorator* around another bus must forward every wrapper, not just the
/// raw accessors. Overriding `read` alone routes around a contended
/// implementation's `read_cycle` and silently takes the contention with it.
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
    fn input(&mut self, port: u16) -> u8;
    fn output(&mut self, port: u16, value: u8);

    /// Burn `t` T-states during which the CPU is driving nothing the rest of
    /// the machine can arbitrate on: the wait states of an interrupt
    /// acknowledge, and nothing else.
    ///
    /// Internal cycles that happen mid-instruction are [`Bus::tick_at`]; they
    /// do hold an address, and on a contended machine that address is what
    /// decides whether they are stalled.
    fn tick(&mut self, t: u32);

    /// Burn `t` T-states of internal CPU activity with `addr` held on the
    /// address bus.
    ///
    /// This is `t` separate one-T-state cycles rather than one cycle of `t`
    /// T-states, because that is what the ULA sees and each of them is stalled
    /// on its own account.
    fn tick_at(&mut self, addr: u16, t: u32) {
        let _ = addr;
        self.tick(t);
    }

    /// M1: opcode fetch. Four T-states — three for the read, one for the
    /// refresh cycle that follows it.
    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        let v = self.read(addr);
        self.tick(4);
        v
    }

    /// A normal memory read machine cycle: three T-states.
    fn read_cycle(&mut self, addr: u16) -> u8 {
        let v = self.read(addr);
        self.tick(3);
        v
    }

    /// A normal memory write machine cycle: three T-states.
    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.write(addr, value);
        self.tick(3);
    }

    /// An I/O read: four T-states in total, but `IORQ` does not assert until
    /// one T-state in, and the remaining three include the automatic wait
    /// state. The split matters because the ULA samples the bus at that point,
    /// so a device sees the access at T+1 rather than T+4.
    fn input_cycle(&mut self, port: u16) -> u8 {
        self.tick(1);
        let v = self.input(port);
        self.tick(3);
        v
    }

    /// An I/O write: four T-states, asserting one T-state in as reads do.
    fn output_cycle(&mut self, port: u16, value: u8) {
        self.tick(1);
        self.output(port, value);
        self.tick(3);
    }

    /// The byte the interrupting device places on the data bus during an
    /// interrupt acknowledge cycle. On the Spectrum nothing drives the bus, so
    /// it floats to 0xFF; other hardware supplies a vector here.
    fn interrupt_data(&mut self) -> u8 {
        0xFF
    }

    /// True while the maskable interrupt line is asserted. The core samples
    /// this at instruction boundaries.
    fn interrupt_pending(&self) -> bool {
        false
    }

    /// True while a non-maskable interrupt is pending. The implementor is
    /// responsible for clearing it once taken (NMI is edge-triggered).
    fn nmi_pending(&mut self) -> bool {
        false
    }
}

/// A flat 64K address space with no I/O, for tests and for running assembler
/// output before there is any Spectrum hardware to run it in.
#[derive(Clone)]
pub struct FlatMemory {
    pub ram: Box<[u8; 0x1_0000]>,
    /// Total T-states elapsed.
    pub t: u64,
    /// Bytes read back from every input port.
    pub port_value: u8,
    /// Log of `(port, value)` writes, so tests can assert on OUT.
    pub port_writes: Vec<(u16, u8)>,
}

impl Default for FlatMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl FlatMemory {
    pub fn new() -> Self {
        Self {
            ram: Box::new([0; 0x1_0000]),
            t: 0,
            port_value: 0xFF,
            port_writes: Vec::new(),
        }
    }

    /// Copy `bytes` into memory at `addr`, wrapping at the top of the address
    /// space.
    pub fn load(&mut self, addr: u16, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.ram[addr.wrapping_add(i as u16) as usize] = *b;
        }
    }

    pub fn word(&self, addr: u16) -> u16 {
        u16::from_le_bytes([
            self.ram[addr as usize],
            self.ram[addr.wrapping_add(1) as usize],
        ])
    }
}

impl Bus for FlatMemory {
    fn read(&mut self, addr: u16) -> u8 {
        self.ram[addr as usize]
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.ram[addr as usize] = value;
    }

    fn input(&mut self, _port: u16) -> u8 {
        self.port_value
    }

    fn output(&mut self, port: u16, value: u8) {
        self.port_writes.push((port, value));
    }

    fn tick(&mut self, t: u32) {
        self.t += u64::from(t);
    }
}
