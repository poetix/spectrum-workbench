//! The ULA holding the CPU, and the byte it leaves on the bus — as the CPU
//! experiences them, which is the only place either is observable.
//!
//! The unit tests in `src/contention.rs` check the arithmetic against Fuse for
//! every T-state of the frame. These check that the arithmetic is *applied*: at
//! the right point of each machine cycle, to internal cycles as well as to
//! reads, and by the different rules I/O follows. The T-state walk-throughs in
//! the comments are the working; the numbers asserted are what a real machine
//! does.

use rkw_spectrum::contention::{self, FIRST_CONTENDED_T};
use rkw_spectrum::frame::{FIRST_DISPLAY_LINE, FIRST_DISPLAY_T, T_STATES_PER_LINE};
use rkw_spectrum::screen::{DISPLAY_BYTES, attr_addr};
use rkw_spectrum::{SCREEN_BASE, Spectrum};
use z80::{Bus, Cpu};

/// Somewhere in the top 32K, which the ULA never arbitrates for. Code put here
/// runs at its nominal speed whatever the beam is doing.
const FREE: u16 = 0x8000;

/// Somewhere in the bottom 16K, which it always does.
const HELD: u16 = 0x4800;

/// A machine with `code` at `addr`, `PC` there, and the clock parked `t`
/// T-states into the first frame.
fn board(t: u64, addr: u16, code: &[u8]) -> (Cpu, Spectrum) {
    let mut machine = Spectrum::new();
    machine.memory.load(addr, code);
    machine.tick(t as u32);

    let mut cpu = Cpu::new();
    cpu.regs.pc = addr;
    cpu.regs.sp = 0xFF00;
    (cpu, machine)
}

/// The wait [`contention::delay`] imposes, in the units the rest of this file
/// counts T-states in.
fn d(t: u64) -> u64 {
    u64::from(contention::delay(t))
}

/// What one instruction costs, in T-states.
fn cost(cpu: &mut Cpu, machine: &mut Spectrum) -> u64 {
    let before = machine.t_states();
    cpu.step(machine);
    machine.t_states() - before
}

/// A machine whose display file is all zeroes, so that a floating-bus read is
/// distinguishable from the `0xFF` an idle bus gives.
fn blank_screen(machine: &mut Spectrum) {
    for addr in SCREEN_BASE..SCREEN_BASE + DISPLAY_BYTES as u16 {
        machine.memory.poke(addr, 0x00);
    }
}

#[test]
fn an_access_to_the_contended_bank_waits_and_one_above_it_does_not() {
    // LD A,(HL) is an opcode fetch and a read. With the code above the bank
    // and HL inside it, only the read is held:
    //
    //   M1 at $8000, free            14335 -> 14339
    //   read $4800, waits 2          14339 -> 14344
    let code = [0x7E]; // ld a,(hl)
    let (mut cpu, mut machine) = board(FIRST_CONTENDED_T, FREE, &code);
    cpu.regs.set_hl(HELD);
    assert_eq!(d(FIRST_CONTENDED_T + 4), 2);
    assert_eq!(cost(&mut cpu, &mut machine), 9);

    // The same instruction reading from the free bank costs its nominal seven.
    let (mut cpu, mut machine) = board(FIRST_CONTENDED_T, FREE, &code);
    cpu.regs.set_hl(0xC000);
    assert_eq!(cost(&mut cpu, &mut machine), 7);
}

#[test]
fn nothing_is_held_while_the_beam_is_in_the_border() {
    // Code and operand both in the contended bank, and still nominal — as long
    // as no cycle of the instruction lands inside a fetch. LD A,(HL) spans
    // seven T-states, so "in the border" means seven clear of the next one.
    let code = [0x7E]; // ld a,(hl)
    for t in [0, 1000, FIRST_CONTENDED_T - 7, FIRST_DISPLAY_T + 150] {
        let (mut cpu, mut machine) = board(t, HELD, &code);
        cpu.regs.set_hl(HELD);
        assert_eq!(cost(&mut cpu, &mut machine), 7, "T {t}");
    }
}

#[test]
fn an_internal_cycle_is_held_too_because_the_cpu_is_still_holding_the_address() {
    // INC (HL) is fetch, read, one internal T-state, write. The internal one
    // has HL on the address bus, and the ULA cannot tell it from a read:
    //
    //   M1 at $8000, free            14331 -> 14335
    //   read $4800, waits 6          14335 -> 14344
    //   internal at $4800, waits 5   14344 -> 14350
    //   write $4800, waits 0         14350 -> 14353
    //
    // 22 T-states against a nominal 11. An internal cycle that carried no
    // address would not wait on the third line, would then meet the write in a
    // slot costing 4 rather than 0, and would come to 21 — which is the whole
    // of ADR-0023 in one number.
    let (mut cpu, mut machine) = board(FIRST_CONTENDED_T - 4, FREE, &[0x34]); // inc (hl)
    cpu.regs.set_hl(HELD);
    assert_eq!(cost(&mut cpu, &mut machine), 22);
}

#[test]
fn the_seven_internal_t_states_of_add_hl_are_held_against_the_refresh_address() {
    // ADD HL,DE is an opcode fetch and seven internal T-states, and what is on
    // the bus for those seven is I:R — the address the refresh half of the M1
    // left there, not HL and not PC. With I in the free bank nothing waits;
    // with I pointing into the contended one, all seven do.
    let code = [0x19]; // add hl,de
    let t = FIRST_CONTENDED_T;

    let (mut cpu, mut machine) = board(t, FREE, &code);
    cpu.regs.i = 0x00;
    assert_eq!(cost(&mut cpu, &mut machine), 11);

    let (mut cpu, mut machine) = board(t, FREE, &code);
    cpu.regs.i = 0x40;
    let held = cost(&mut cpu, &mut machine);

    // Seven one-T-state cycles from 14339, each waiting whatever the pattern
    // says at the T-state it *starts* on — which the wait before it has
    // already moved. Waiting shifts the phase, so the sequence is 2,0,6,0,6,0,6
    // and not the 2,1,0,0,... reading straight down the pattern suggests.
    let mut clock = t + 4;
    let mut waits = Vec::new();
    for _ in 0..7 {
        waits.push(d(clock));
        clock += d(clock) + 1;
    }
    assert_eq!(waits, vec![2, 0, 6, 0, 6, 0, 6]);
    assert_eq!(held, 4 + (clock - (t + 4)));
    assert_eq!(held, 31);
}

#[test]
fn a_repeating_block_instruction_is_held_on_every_cycle_of_every_iteration() {
    // LDIR moving 64 bytes from the free bank into the contended one, started
    // in the border and running well into the display. What matters is not the
    // exact total but that it is a long way above the uncontended one and that
    // it is reproducible: a demo that budgets for the nominal figure runs off
    // the end of its scanline.
    let code = [0xED, 0xB0]; // ldir
    let start = FIRST_CONTENDED_T - 100;

    let mut totals = Vec::new();
    for (src, dst) in [(0xC000u16, 0xD000u16), (0xC000, HELD)] {
        let (mut cpu, mut machine) = board(start, FREE, &code);
        cpu.regs.set_hl(src);
        cpu.regs.set_de(dst);
        cpu.regs.set_bc(64);
        let before = machine.t_states();
        for _ in 0..64 {
            cpu.step(&mut machine);
        }
        assert_eq!(cpu.regs.bc(), 0);
        totals.push(machine.t_states() - before);
    }

    // 21 T-states an iteration for the 63 that repeat, 16 for the last.
    assert_eq!(totals[0], 63 * 21 + 16);
    assert!(
        totals[1] > totals[0] + 200,
        "contended {} against free {}",
        totals[1],
        totals[0]
    );

    // Same run, same answer.
    let (mut cpu, mut machine) = board(start, FREE, &code);
    cpu.regs.set_hl(0xC000);
    cpu.regs.set_de(HELD);
    cpu.regs.set_bc(64);
    let before = machine.t_states();
    for _ in 0..64 {
        cpu.step(&mut machine);
    }
    assert_eq!(machine.t_states() - before, totals[1]);
}

#[test]
fn the_four_rows_of_the_io_contention_table() {
    // I/O follows different rules to memory, and which of the four applies
    // depends on two independent things: whether the port's high byte falls in
    // the contended bank — the ULA has only the address lines and cannot tell
    // a port from an address — and whether A0 is low, which is what makes it
    // the ULA's own port.
    //
    //   high byte contended, A0 set     C:1 C:1 C:1 C:1
    //   high byte contended, A0 clear   C:1 C:3
    //   high byte free,      A0 set     N:1 N:3
    //   high byte free,      A0 clear   N:1 C:3
    //
    // IN A,(n) puts A in the high byte, so A chooses the column and n the row.
    // Starting one T-state into the display, so that the port cycle begins at
    // 14343 — six T-states from the ULA's turn, the worst of the eight.
    let t = FIRST_CONTENDED_T + 1;
    let nominal = 11; // 4 opcode fetch, 3 operand fetch, 4 the port cycle
    let start = t + 7;
    assert_eq!(start, 14_343);
    assert_eq!(d(start), 6);

    let run = |a: u8, n: u8| {
        let (mut cpu, mut machine) = board(t, FREE, &[0xDB, n]); // in a,(n)
        cpu.regs.a = a;
        cost(&mut cpu, &mut machine)
    };

    // Free high byte, odd port: nothing is contended at all, wherever it falls.
    assert_eq!(run(0x00, 0xFF), nominal);

    // Free high byte, even port: the first T-state passes, then the ULA has to
    // finish what it is doing before it can answer, and the remaining three are
    // held once between them.
    //   14343 +1 -> 14344, wait 5 -> 14349, +3 -> 14352
    assert_eq!(run(0x00, 0xFE), nominal + 5);

    // Contended high byte, odd port: the address alone is enough, and there is
    // no single access for the ULA to hold, so each of the four T-states is
    // arbitrated on its own.
    //   wait 6 -> 14349, +1 -> 14350, wait 0, +1 -> 14351, wait 6 -> 14357,
    //   +1 -> 14358, wait 0, +1 -> 14359
    assert_eq!(run(0x40, 0xFF), nominal + 6 + 0 + 6 + 0);

    // Contended high byte, even port: held once for the address and then once
    // for the ULA, which by then has nothing left to finish.
    //   wait 6 -> 14349, +1 -> 14350, wait 0, +3 -> 14353
    assert_eq!(run(0x40, 0xFE), nominal + 6);

    // All four rows are genuinely different, which is what makes the table a
    // table rather than a special case.
    let costs = [
        run(0x00, 0xFF),
        run(0x00, 0xFE),
        run(0x40, 0xFF),
        run(0x40, 0xFE),
    ];
    assert_eq!(costs, [11, 16, 23, 17]);
}

#[test]
fn an_unattached_port_reads_the_byte_the_ula_is_fetching() {
    // The screen is zeroed, so an idle bus (0xFF) and a fetched byte (0x00)
    // are distinguishable. IN A,(C) with BC = $00FF: an odd port nothing
    // answers, with a high byte outside the contended bank so that the cycle
    // costs its nominal four T-states wherever it falls.
    //
    //   M1 $8000, 4;  M1 $8001, 4;  port cycle 4, sampled 3 T-states in.
    let code = [0xED, 0x78]; // in a,(c)

    // Sampled at 14336 + 2, which is the first bitmap byte of the top line.
    let (mut cpu, mut machine) = board(FIRST_DISPLAY_T + 2 - 11, FREE, &code);
    blank_screen(&mut machine);
    cpu.regs.set_bc(0x00FF);
    cpu.step(&mut machine);
    assert_eq!(machine.t_states(), FIRST_DISPLAY_T + 3);
    assert_eq!(cpu.regs.a, 0x00);

    // Two T-states earlier the ULA has not started and nothing drives the bus.
    let (mut cpu, mut machine) = board(FIRST_DISPLAY_T - 11, FREE, &code);
    blank_screen(&mut machine);
    cpu.regs.set_bc(0x00FF);
    cpu.step(&mut machine);
    assert_eq!(cpu.regs.a, 0xFF);

    // And an attribute byte, which a program can tell from a bitmap byte by
    // putting something in one and not the other.
    let (mut cpu, mut machine) = board(FIRST_DISPLAY_T + 3 - 11, FREE, &code);
    blank_screen(&mut machine);
    machine.memory.poke(attr_addr(SCREEN_BASE, 0, 0), 0x2A);
    cpu.regs.set_bc(0x00FF);
    cpu.step(&mut machine);
    assert_eq!(cpu.regs.a, 0x2A);
}

#[test]
fn the_floating_bus_synchronises_a_program_to_the_beam() {
    // The technique, entire: spin reading a port nothing answers until the
    // answer stops being 0xFF, and you are at the top-left of the display with
    // no interrupt and no counting. Demos use it to start an effect on a known
    // scanline; a game uses it to draw without tearing.
    //
    //   ld bc,$00ff
    //   sync: in a,(c)   ; 12
    //         inc a      ;  4   -- 0xFF wraps to zero and sets Z
    //         jr z,sync  ; 12 taken, 7 not
    //         ld a,2
    //         out ($fe),a
    #[rustfmt::skip]
    let code = [
        0x01, 0xFF, 0x00,  // ld bc,$00ff
        0xED, 0x78,        // in a,(c)
        0x3C,              // inc a
        0x28, 0xFB,        // jr z,-5
        0x3E, 0x02,        // ld a,2
        0xD3, 0xFE,        // out ($fe),a
    ];
    let (mut cpu, mut machine) = board(0, FREE, &code);
    blank_screen(&mut machine);

    // The spin is 28 T-states, so it walks through the eight-T-state fetch
    // group rather than sitting in one phase, and cannot miss the display.
    while cpu.regs.pc != FREE + 8 {
        cpu.step(&mut machine);
        assert!(
            machine.t_states() < 100_000,
            "the sync loop never broke out"
        );
    }

    // It broke out on the first pass whose sample landed inside the fetch:
    // the loop starts at T=10 and samples 11 T-states into each pass, so the
    // samples are 21, 49, 77 ... and the first at or past 14336 is 14357.
    assert_eq!(machine.t_states(), FIRST_DISPLAY_T + 33);
    let into_line = machine.t_states() - FIRST_DISPLAY_T;
    assert!(into_line < contention::FETCH_T_STATES);

    // And the border write that follows lands on the first display line, which
    // is the point of the exercise.
    cpu.step(&mut machine); // ld a,2
    cpu.step(&mut machine); // out ($fe),a
    machine.ula.end_frame();
    let border = machine.ula.border_lines();
    assert_eq!(border[FIRST_DISPLAY_LINE - 1], 0);
    assert_eq!(border[FIRST_DISPLAY_LINE], 2);
}

#[test]
fn a_routine_costs_a_different_amount_at_every_point_in_a_scanline() {
    // The property a timing-sensitive effect is built on and broken by: the
    // same instructions cost different numbers of T-states depending only on
    // where in the line they start. Across one display line the profile is
    // flat through the border and a period-eight sawtooth through the fetch.
    let code = [0x34]; // inc (hl)
    let line = FIRST_CONTENDED_T + 100 * T_STATES_PER_LINE;

    let profile: Vec<u64> = (0..T_STATES_PER_LINE)
        .map(|i| {
            let (mut cpu, mut machine) = board(line + i, FREE, &code);
            cpu.regs.set_hl(HELD);
            cost(&mut cpu, &mut machine)
        })
        .collect();

    // Nominal everywhere no cycle of the instruction meets a fetch. INC (HL)
    // spans eleven T-states, so the flat stretch stops eleven short of the
    // next line rather than at the end of this one.
    for i in contention::FETCH_T_STATES..T_STATES_PER_LINE - 11 {
        assert_eq!(profile[i as usize], 11, "{i} into the line");
    }
    // Through the fetch it is never nominal, and it repeats with the ULA's
    // period of eight. The last two groups are left out: an instruction
    // starting there finishes in the border, so its profile is the tail of the
    // sawtooth rather than the whole of it.
    let inside = &profile[..(contention::FETCH_T_STATES as usize - 16)];
    assert!(inside.iter().all(|&c| c > 11), "{inside:?}");
    assert_eq!(inside[..8], [18, 17, 16, 15, 22, 21, 20, 19]);
    for i in 8..inside.len() {
        assert_eq!(inside[i], inside[i - 8], "{i} into the line");
    }
}

#[test]
fn code_in_the_contended_bank_is_slower_than_the_same_code_above_it() {
    // Why every demo that cares puts its timing loop above 0x8000: the opcode
    // fetches alone are enough, without a single memory operand.
    let code = [0x00, 0x00, 0x00, 0x00]; // four nops
    let t = FIRST_CONTENDED_T;

    let run = |addr: u16| {
        let (mut cpu, mut machine) = board(t, addr, &code);
        let before = machine.t_states();
        for _ in 0..4 {
            cpu.step(&mut machine);
        }
        machine.t_states() - before
    };

    assert_eq!(run(FREE), 16);

    // Four M1 cycles, each waiting whatever the pattern says at the T-state it
    // starts on, which the wait before it has already moved:
    //   14335 wait 6 -> 14341 +4 -> 14345 wait 2 -> 14347 +4 -> 14351
    //   14351 wait 6 -> 14357 +4 -> 14361 wait 4 -> 14365 +4 -> 14369
    assert_eq!(run(HELD), 16 + 6 + 2 + 6 + 4);
    assert_eq!(run(HELD), 34);
}
