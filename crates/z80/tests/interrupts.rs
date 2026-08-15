//! Where the interrupt line is sampled, and the two instructions that behave
//! oddly when it is.
//!
//! `z80test` runs with interrupts disabled from its first instruction to its
//! last, so none of this is covered by it — nor by the Fuse suite, which runs
//! one instruction at a time with no interrupt controller. These are the
//! hand-written ones, and the reasoning behind each is in the comment above it.

use z80::{Bus, Cpu, FlatMemory, InterruptMode, Stop, flag};

/// A bus that asserts `INT` until told otherwise, so that every instruction
/// boundary is a chance to take one.
struct Interrupting {
    mem: FlatMemory,
    int: bool,
    nmi: bool,
}

impl Interrupting {
    fn new() -> Interrupting {
        Interrupting {
            mem: FlatMemory::new(),
            int: false,
            nmi: false,
        }
    }

    fn load(&mut self, addr: u16, bytes: &[u8]) {
        self.mem.load(addr, bytes);
    }
}

impl Bus for Interrupting {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem.read(addr)
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.mem.write(addr, value);
    }
    fn input(&mut self, port: u16) -> u8 {
        self.mem.input(port)
    }
    fn output(&mut self, port: u16, value: u8) {
        self.mem.output(port, value);
    }
    fn tick(&mut self, t: u32) {
        self.mem.tick(t);
    }
    fn tick_at(&mut self, addr: u16, t: u32) {
        self.mem.tick_at(addr, t);
    }
    fn interrupt_pending(&self) -> bool {
        self.int
    }
    fn nmi_pending(&mut self) -> bool {
        core::mem::take(&mut self.nmi)
    }
}

/// A CPU with interrupts enabled in mode 1 and a stack, running `code` at
/// `0x8000`.
fn ready(code: &[u8]) -> (Cpu, Interrupting) {
    let mut bus = Interrupting::new();
    bus.load(0x8000, code);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    cpu.regs.iff1 = true;
    cpu.regs.iff2 = true;
    cpu.regs.im = InterruptMode::Im1;
    (cpu, bus)
}

#[test]
fn an_interrupt_is_not_taken_between_a_prefix_and_the_byte_it_prefixes() {
    // The rule the decoding tables call NONI. A `DD` sets "use IX instead of
    // HL" in a place the CPU cannot push, so an interrupt between the prefix
    // and its opcode would lose it — and the Z80 does not offer one. Here the
    // line is held down for the whole run and the four-byte `DD CB d op` still
    // executes as one indivisible thing.
    let (mut cpu, mut bus) = ready(&[0xDD, 0xCB, 0x02, 0x06]); // rlc (ix+2)
    cpu.regs.ix = 0x9000;
    bus.load(0x9002, &[0x81]);
    bus.int = true;
    cpu.regs.iff1 = false; // so the instruction gets to start at all

    // All four bytes and all 23 T-states in one step, with the line down
    // throughout: there is no boundary inside it to take an interrupt at.
    let before = bus.mem.t;
    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert_eq!(cpu.regs.pc, 0x8004);
    assert_eq!(bus.mem.t - before, 23);
    assert_eq!(bus.read(0x9002), 0x03); // rotated

    // The boundary after it is a sampling point like any other.
    cpu.regs.iff1 = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
}

#[test]
fn an_interrupt_is_not_taken_inside_a_two_byte_ed_nop() {
    // `ED 00` and its many siblings are "NONI followed by NOP": nothing
    // happens, in eight T-states, and no interrupt is taken between the two
    // bytes. One step, both bytes, PC past the pair.
    let (mut cpu, mut bus) = ready(&[0xED, 0x00, 0x00]);
    bus.int = true;
    cpu.regs.iff1 = false; // let the ED run first

    let before = bus.mem.t;
    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert_eq!(cpu.regs.pc, 0x8002);
    assert_eq!(bus.mem.t - before, 8);

    // And the pair is not itself a shield: an interrupt is taken at the
    // boundary after it, exactly as after any other instruction.
    cpu.regs.iff1 = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
}

#[test]
fn a_repeating_block_instruction_is_interruptible_between_iterations() {
    // The other half of the sampling rule. LDIR is one step per iteration, so
    // an interrupt lands between two of them — and PC is back on the `ED`, so
    // the return from the handler resumes the copy rather than skipping it.
    let (mut cpu, mut bus) = ready(&[0xED, 0xB0]); // ldir
    bus.load(0x9000, &[1, 2, 3, 4]);
    cpu.regs.set_hl(0x9000);
    cpu.regs.set_de(0xA000);
    cpu.regs.set_bc(4);

    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert_eq!(cpu.regs.pc, 0x8000, "PC goes back to the ED to repeat");
    assert_eq!(cpu.regs.bc(), 3);

    bus.int = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
    // The handler's return address is the instruction itself.
    assert_eq!(bus.mem.word(cpu.regs.sp), 0x8000);

    bus.int = false;
    cpu.regs.pc = 0x8000;
    cpu.regs.iff1 = true;
    while cpu.regs.bc() != 0 {
        cpu.step(&mut bus);
    }
    // Every byte arrived, interruption and all.
    for i in 0..4u16 {
        assert_eq!(bus.read(0xA000 + i), (i + 1) as u8);
    }
}

#[test]
fn an_interrupt_is_not_taken_after_ei_but_is_after_the_instruction_following() {
    // What makes `EI` / `RET` safe: the boundary immediately after `EI` is not
    // a sampling point, so a handler can re-enable and return without being
    // re-entered before the return has happened.
    let (mut cpu, mut bus) = ready(&[0xFB, 0x00, 0x00]); // ei ; nop ; nop
    cpu.regs.iff1 = false;
    cpu.regs.iff2 = false;
    bus.int = true;

    assert_eq!(cpu.step(&mut bus), Stop::Instruction); // ei
    assert!(cpu.regs.iff1);
    assert_eq!(
        cpu.step(&mut bus),
        Stop::Instruction,
        "the nop after EI runs"
    );
    assert_eq!(cpu.regs.pc, 0x8002);
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
}

#[test]
fn ld_a_i_loses_its_parity_flag_when_an_interrupt_arrives_on_top_of_it() {
    // The NMOS bug, from the Zilog data book and Roshchin's 1998 note. `LD A,I`
    // copies IFF2 into P/V, which is the only way to read the interrupt state
    // back — and if the interrupt is accepted at the end of that very
    // instruction, IFF2 has been cleared before the copy settles, so the flag
    // reads 0. Software that tests P/V here to decide whether to re-enable
    // interrupts will get it wrong on real hardware, and that is the point.
    let (mut cpu, mut bus) = ready(&[0xED, 0x57]); // ld a,i
    cpu.regs.i = 0x7F;

    // With no interrupt, P/V is IFF2 and IFF2 is set.
    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert!(cpu.regs.flag(flag::P));
    assert_eq!(cpu.regs.a, 0x7F);

    // With one arriving on the boundary, it is not.
    let (mut cpu, mut bus) = ready(&[0xED, 0x57]);
    cpu.regs.i = 0x7F;
    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert!(cpu.regs.flag(flag::P));
    bus.int = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
    assert!(!cpu.regs.flag(flag::P), "P/V is cleared by the acceptance");
    // And the Q latch follows the flag it belongs to.
    assert_eq!(cpu.regs.q, cpu.regs.f);
}

#[test]
fn ld_a_r_has_the_same_bug_and_an_ordinary_instruction_does_not() {
    let (mut cpu, mut bus) = ready(&[0xED, 0x5F]); // ld a,r
    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert!(cpu.regs.flag(flag::P));
    bus.int = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
    assert!(!cpu.regs.flag(flag::P));

    // An instruction that merely leaves P/V set is untouched: the bug is about
    // where the flag came from, not what is in it.
    let (mut cpu, mut bus) = ready(&[0xED, 0x57, 0x00]); // ld a,i ; nop
    cpu.regs.i = 0x7F;
    cpu.step(&mut bus); // ld a,i sets P/V
    cpu.step(&mut bus); // nop leaves it alone
    assert!(cpu.regs.flag(flag::P));
    bus.int = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
    assert!(
        cpu.regs.flag(flag::P),
        "only LD A,I and LD A,R are affected"
    );
}

#[test]
fn an_interrupt_is_not_taken_after_a_retn_that_restored_a_different_iff1() {
    // Weissflog, 2021. An NMI leaves IFF1 clear and IFF2 set; the `RETN` that
    // ends the handler puts IFF1 back, and the boundary after it is not a
    // sampling point. Without that, a maskable interrupt pending throughout
    // the NMI would be taken before the interrupted instruction got to run.
    let (mut cpu, mut bus) = ready(&[0xED, 0x45]); // retn
    bus.mem.load(0xFEFE, &[0x00, 0x90]); // return address 0x9000
    cpu.regs.sp = 0xFEFE;
    cpu.regs.iff1 = false; // as an NMI left it
    cpu.regs.iff2 = true;
    bus.int = true;

    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert_eq!(cpu.regs.pc, 0x9000);
    assert!(cpu.regs.iff1, "IFF1 restored from IFF2");

    // The line is still down, and still not taken.
    bus.load(0x9000, &[0x00, 0x00]);
    assert_eq!(
        cpu.step(&mut bus),
        Stop::Instruction,
        "the first instruction back runs"
    );
    assert_eq!(cpu.regs.pc, 0x9001);
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);

    // A return that found the two agreeing — an ordinary RETI from a maskable
    // handler that never saw an NMI — suppresses nothing. The line goes down
    // after it has run, because with IFF1 already set a line held down would
    // have pre-empted the RETI itself.
    let (mut cpu, mut bus) = ready(&[0xED, 0x4D]); // reti
    bus.mem.load(0xFEFE, &[0x00, 0x90]);
    cpu.regs.sp = 0xFEFE;
    cpu.regs.iff1 = true;
    cpu.regs.iff2 = true;
    assert_eq!(cpu.step(&mut bus), Stop::Instruction);
    assert_eq!(cpu.regs.pc, 0x9000);
    bus.int = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
}

#[test]
fn a_halted_cpu_samples_the_line_every_four_t_states_and_steps_past_the_halt() {
    let (mut cpu, mut bus) = ready(&[0x76, 0x00]); // halt ; nop
    assert_eq!(cpu.step(&mut bus), Stop::Halt);
    assert_eq!(cpu.regs.pc, 0x8000, "PC stays on the HALT (ADR-0004)");

    let before = bus.mem.t;
    assert_eq!(cpu.step(&mut bus), Stop::Halt);
    assert_eq!(bus.mem.t - before, 4);

    bus.int = true;
    assert_eq!(cpu.step(&mut bus), Stop::Interrupt);
    // The return address is the instruction *after* the HALT, so the handler
    // returns to the nop rather than back into the halt.
    assert_eq!(bus.mem.word(cpu.regs.sp), 0x8001);
    assert!(!cpu.regs.halted);
}
