//! The machine as the CPU and the emulation thread see it: writes that the
//! map refuses, an `OUT` that reaches the ULA, and interrupts arriving at
//! 50 Hz.

use rkw_debug::Machine;
use rkw_spectrum::frame::{INTERRUPT_LENGTH, T_STATES_PER_FRAME};
use rkw_spectrum::{SCREEN_BASE, Spectrum};
use z80::{Cpu, Stop};

/// The slice loop of ADR-0007, in miniature: run to the earlier of the
/// caller's target and the machine's next scheduled event, and service the
/// event when the clock arrives at it. `rkw_debug::Emu` does exactly this
/// around a debugger; this is the same shape with nothing armed.
fn run_to(cpu: &mut Cpu, machine: &mut Spectrum, target: u64) {
    while machine.t_states() < target {
        let deadline = machine.next_event().unwrap().min(target);
        while machine.t_states() < deadline {
            cpu.step(machine);
        }
        if machine.t_states() >= machine.next_event().unwrap() {
            machine.service_event();
        }
    }
}

/// `IM 1`, `EI`, then a `HALT` looped back to, with a handler at `0x0038` that
/// counts its own calls at `0x8000`.
fn counting_interrupts() -> (Cpu, Spectrum) {
    let mut machine = Spectrum::new();
    #[rustfmt::skip]
    machine.memory.load(0x0038, &[
        0xF5,                    // push af
        0x3A, 0x00, 0x80,        // ld a,($8000)
        0x3C,                    // inc a
        0x32, 0x00, 0x80,        // ld ($8000),a
        0xF1,                    // pop af
        0xFB,                    // ei
        0xED, 0x4D,              // reti
    ]);
    #[rustfmt::skip]
    machine.memory.load(0x8010, &[
        0xED, 0x56,              // im 1
        0xFB,                    // ei
        0x76,                    // halt
        0x18, 0xFD,              // jr -3, back to the halt
    ]);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8010;
    cpu.regs.sp = 0xFF00;
    (cpu, machine)
}

#[test]
fn the_cpu_cannot_write_to_rom_and_a_poke_can() {
    let mut machine = Spectrum::new();
    machine.memory.poke(0x0000, 0xC9); // ret
    let mut cpu = Cpu::new();

    // ld a,$AA ; ld ($0000),a ; ld ($4000),a
    machine
        .memory
        .load(0x8000, &[0x3E, 0xAA, 0x32, 0x00, 0x00, 0x32, 0x00, 0x40]);
    cpu.regs.pc = 0x8000;
    for _ in 0..3 {
        cpu.step(&mut machine);
    }

    assert_eq!(machine.memory.read(0x0000), 0xC9, "ROM was written");
    assert_eq!(machine.memory.read(0x4000), 0xAA, "RAM was not");
}

#[test]
fn an_out_to_port_fe_sets_the_border() {
    let mut machine = Spectrum::new();
    let mut cpu = Cpu::new();
    // ld a,2 ; out ($FE),a
    machine.memory.load(0x8000, &[0x3E, 0x02, 0xD3, 0xFE]);
    cpu.regs.pc = 0x8000;
    cpu.step(&mut machine);
    cpu.step(&mut machine);

    assert_eq!(machine.ula.border(), 2);
    assert_eq!(machine.ula.port_fe(), 0x02);

    // Any port with A0 low is the ULA: the ROM writes $7FFE and the rest when
    // it reads the keyboard, and they all carry a border colour with them.
    machine
        .memory
        .load(0x8004, &[0x3E, 0x05, 0x01, 0xFE, 0x7F, 0xED, 0x79]);
    cpu.regs.pc = 0x8004;
    for _ in 0..3 {
        cpu.step(&mut machine);
    }
    assert_eq!(machine.ula.border(), 5);

    // An odd port is not.
    machine.memory.load(0x800B, &[0x3E, 0x03, 0xD3, 0xFF]);
    cpu.regs.pc = 0x800B;
    cpu.step(&mut machine);
    cpu.step(&mut machine);
    assert_eq!(machine.ula.border(), 5);
}

#[test]
fn one_interrupt_arrives_per_frame() {
    let (mut cpu, mut machine) = counting_interrupts();

    run_to(&mut cpu, &mut machine, T_STATES_PER_FRAME * 10 + 1000);

    // Eleven, not ten: this program reaches its `HALT` sixteen T-states after
    // the machine was made, which is inside the interrupt at the top of frame
    // zero, so it takes that one too. The property under test is the cadence —
    // one per frame — and a program that spent longer setting up would see ten.
    assert_eq!(machine.memory.read(0x8000), 11);
    assert_eq!(machine.ula.frames(), 10);
}

#[test]
fn an_interrupt_is_taken_at_the_top_of_the_frame() {
    let (mut cpu, mut machine) = counting_interrupts();
    let mut taken = Vec::new();

    while machine.t_states() < T_STATES_PER_FRAME * 4 {
        let next = machine.next_event().unwrap();
        if machine.t_states() >= next {
            machine.service_event();
        }
        let before = machine.t_states();
        if cpu.step(&mut machine) == Stop::Interrupt {
            taken.push(before);
        }
    }

    assert_eq!(taken.len(), 4);
    for (frame, t) in taken.iter().enumerate() {
        let frame = frame as u64;
        assert_eq!(t / T_STATES_PER_FRAME, frame, "interrupt at T={t}");
        // Accepted within the window, not merely somewhere in the frame: the
        // machine is halted, so the CPU is sampling the line every 4 T-states.
        assert!(
            t % T_STATES_PER_FRAME < INTERRUPT_LENGTH,
            "interrupt at T={t} is {} into its frame",
            t % T_STATES_PER_FRAME
        );
    }
}

#[test]
fn a_halted_machine_still_flashes_and_still_draws() {
    let (mut cpu, mut machine) = counting_interrupts();
    // Paper white, ink black, flashing, with a solid byte of pixels.
    machine
        .memory
        .poke(rkw_spectrum::screen::attr_addr(SCREEN_BASE, 0, 0), 0xB8);
    machine
        .memory
        .poke(rkw_spectrum::screen::pixel_addr(SCREEN_BASE, 0, 0), 0xFF);

    let top_left = |machine: &Spectrum| machine.frame().pixel(48, 48);

    assert_eq!(top_left(&machine), 0); // ink black
    run_to(&mut cpu, &mut machine, T_STATES_PER_FRAME * 16);
    assert_eq!(machine.ula.frames(), 16);
    assert_eq!(top_left(&machine), 7); // inverted: paper white
    run_to(&mut cpu, &mut machine, T_STATES_PER_FRAME * 32);
    assert_eq!(top_left(&machine), 0);
}

#[test]
fn rendering_part_way_through_a_frame_shows_the_border_of_the_last_complete_one() {
    let mut machine = Spectrum::new();
    let mut cpu = Cpu::new();
    // ld a,1 ; out ($FE),a
    machine.memory.load(0x8000, &[0x3E, 0x01, 0xD3, 0xFE]);
    cpu.regs.pc = 0x8000;
    cpu.step(&mut machine);
    cpu.step(&mut machine);

    // The write happened in frame zero, which is still in progress.
    assert_eq!(machine.frame().pixel(0, 0), 0);
    // And presenting it is the frame boundary's job. `service_event` is called
    // for a tape edge as well as for the interrupt (0016), so it does what is
    // due at the clock it is called at rather than assuming the frame ended.
    run_to(&mut cpu, &mut machine, T_STATES_PER_FRAME);
    assert_eq!(machine.frame().pixel(0, 0), 1);
}
