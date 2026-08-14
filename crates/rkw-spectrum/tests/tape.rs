//! Tape: the waveform against the machine, and the ROM against the waveform.
//!
//! The tests that need a ROM skip without one, as everything else here does —
//! see `scripts/fetch-rom.sh`. They are the ones that matter most: what a tape
//! has to satisfy is not this crate's idea of a pulse but sixteen kilobytes of
//! 1982 machine code that measures every edge and has an opinion about all of
//! them.

mod common;

use std::sync::Arc;

use rkw_debug::machine::{Clock, Machine};
use rkw_spectrum::frame::{CLOCK_HZ, T_STATES_PER_FRAME};
use rkw_spectrum::{Loaded, Saving, Spectrum, ld_bytes};
use rkw_tape::tap::{DATA_FLAG, HEADER_FLAG};
use rkw_tape::{Player, Tap, Timing};
use z80::{Bus, Cpu, flag};

/// Where a test's own code and data go: clear of the screen, the system
/// variables and anything the ROM touches.
const SCRATCH: u16 = 0x9000;

/// The sentinel a called ROM routine returns to. `HALT` rather than a
/// breakpoint, so that a test which overruns stops rather than running away.
const RETURN_TO: u16 = 0x8000;

/// The ROM's `SA-BYTES`, which is `SAVE`'s half of the tape.
const SA_BYTES: u16 = 0x04C2;

fn tap_of(data: &[u8]) -> Tap {
    Tap::builder().block(DATA_FLAG, data).build()
}

/// Run the machine's schedule without a CPU in it: tick to the next event and
/// service it. What a tape does to the `EAR` line is a property of the machine
/// alone, and a test that ran instructions as well would be asserting on two
/// things at once.
fn tick_to(machine: &mut Spectrum, target: u64) {
    while machine.t_states() < target {
        let event = machine.next_event().expect("the ULA schedules frames");
        let step = event.min(target).saturating_sub(machine.t_states());
        machine.tick(step as u32);
        if machine.t_states() >= event {
            machine.service_event();
        }
    }
}

/// What a read of port `0xFE` says the `EAR` line is doing.
fn ear(machine: &mut Spectrum) -> bool {
    machine.input(0xFEFE) & 0x40 != 0
}

#[test]
fn a_tape_that_is_not_playing_schedules_only_the_frame_interrupt() {
    let mut machine = Spectrum::new();
    assert_eq!(machine.next_event(), Some(T_STATES_PER_FRAME));

    machine.mount_tape(Arc::new(tap_of(&[0x00])));
    assert_eq!(machine.next_event(), Some(T_STATES_PER_FRAME));
    assert!(ear(&mut machine), "the line idles high");
}

#[test]
fn a_playing_tape_schedules_the_earlier_of_its_pulse_and_the_frame() {
    let mut machine = Spectrum::new();
    machine.mount_tape(Arc::new(tap_of(&[0x00])));
    machine.play_tape();

    // The first pulse is due immediately, and every one after it is a pilot
    // pulse away — all of them well inside the frame.
    assert_eq!(machine.next_event(), Some(0));
    tick_to(&mut machine, 1);
    assert_eq!(machine.next_event(), Some(2168));
    tick_to(&mut machine, 2168);
    assert_eq!(machine.next_event(), Some(2 * 2168));
}

#[test]
fn the_ear_line_follows_the_pulses_through_the_machine() {
    let mut machine = Spectrum::new();
    machine.mount_tape(Arc::new(tap_of(&[0x00])));
    machine.play_tape();

    // The pilot: high for 2168 T-states, then low for 2168, and so on. The
    // level itself means nothing to a loader; that it changes when the
    // waveform says it does is the whole of what a loader reads.
    let mut expected = true;
    for pulse in 0..8u64 {
        tick_to(&mut machine, pulse * 2168 + 1);
        assert_eq!(ear(&mut machine), expected, "pilot pulse {pulse}");
        tick_to(&mut machine, (pulse + 1) * 2168 - 1);
        assert_eq!(ear(&mut machine), expected, "still in pulse {pulse}");
        expected = !expected;
    }
}

#[test]
fn the_frame_interrupt_still_arrives_while_a_tape_is_running() {
    // The two schedules interleave, and a tape that swallowed the interrupt
    // would stop every program that loads one thing and then another.
    let mut machine = Spectrum::new();
    machine.mount_tape(Arc::new(tap_of(&[0xFF; 16])));
    machine.play_tape();

    tick_to(&mut machine, 3 * T_STATES_PER_FRAME + 100);
    assert_eq!(machine.ula.frames(), 3);
    assert!(machine.tape.is_playing());
    assert!(machine.tape.block() == 0);
}

#[test]
fn stopping_the_tape_puts_the_line_back_where_it_idles() {
    let mut machine = Spectrum::new();
    machine.mount_tape(Arc::new(tap_of(&[0x00])));
    machine.play_tape();
    tick_to(&mut machine, 3 * 2168 + 10);
    assert!(!ear(&mut machine), "mid-pulse, and this one is low");

    machine.stop_tape();
    assert!(ear(&mut machine));
    assert_eq!(machine.next_event(), Some(T_STATES_PER_FRAME));

    // And picking up where it left off is picking up where it left off.
    machine.play_tape();
    let at = machine.t_states();
    tick_to(&mut machine, at + 1);
    assert_eq!(machine.next_event(), Some(at + 2168));
}

#[test]
fn the_tape_runs_out_and_the_machine_carries_on() {
    let tap = tap_of(&[0x00]);
    let timing = Timing::rom(CLOCK_HZ).with_pause(0);
    let mut machine = Spectrum::new();
    machine.mount_tape(Arc::new(tap.clone()));
    machine.tape.set_timing(timing);
    machine.play_tape();

    tick_to(&mut machine, Player::duration(&tap, &timing) + 1000);
    assert!(!machine.tape.is_playing());
    assert!(machine.tape.finished());
    assert!(ear(&mut machine));
    // Back to the frame interrupt on its own, several frames in.
    assert_eq!(machine.next_event(), Some(machine.ula.next_interrupt()));
    assert!(machine.ula.frames() > 0);
}

// ---------------------------------------------------------------------------
// The ROM's loader, trapped rather than run.
// ---------------------------------------------------------------------------

/// A machine with a tape in it and a CPU set up as the ROM's `LD-BYTES`
/// expects: `IX` where the bytes go, `DE` how many, `A` the flag byte, carry
/// set to load.
fn ready_to_load(tap: Tap, flag_byte: u8, length: u16) -> (Cpu, Spectrum) {
    let mut machine = Spectrum::new();
    machine.mount_tape(Arc::new(tap));
    let mut cpu = Cpu::new();
    cpu.regs.ix = SCRATCH;
    cpu.regs.set_de(length);
    cpu.regs.a = flag_byte;
    cpu.regs.set_flag(flag::C, true);
    // The return address the trap's `RET` will find.
    cpu.regs.sp = 0xFF00;
    machine.memory.write(0xFF00, RETURN_TO as u8);
    machine.memory.write(0xFF01, (RETURN_TO >> 8) as u8);
    (cpu, machine)
}

#[test]
fn the_trap_loads_a_block_and_returns_with_carry_set() {
    let data = [0xDE, 0xAD, 0xBE, 0xEF];
    let (mut cpu, mut machine) = ready_to_load(tap_of(&data), DATA_FLAG, data.len() as u16);

    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Ok { block: 0, bytes: 4 }
    );
    assert!(cpu.regs.flag(flag::C));
    assert_eq!(cpu.regs.pc, RETURN_TO);
    assert_eq!(cpu.regs.sp, 0xFF02);
    assert_eq!(cpu.regs.ix, SCRATCH + 4);
    assert_eq!(cpu.regs.de(), 0);
    for (i, &byte) in data.iter().enumerate() {
        assert_eq!(machine.memory.read(SCRATCH + i as u16), byte);
    }
    // The head has moved on, which is what makes a search for a header end.
    assert_eq!(machine.tape.block(), 1);
}

#[test]
fn the_trap_refuses_a_block_of_the_wrong_kind_without_touching_memory() {
    // What `LOAD ""` does with a tape whose first block is somebody else's
    // header: reject it, and go round again for the next one.
    let data = [0x11, 0x22];
    let tap = Tap::builder()
        .block(HEADER_FLAG, &[0; 17])
        .block(DATA_FLAG, &data)
        .build();
    let (mut cpu, mut machine) = ready_to_load(tap, DATA_FLAG, 2);

    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Rejected { block: 0 }
    );
    assert!(!cpu.regs.flag(flag::C));
    assert_eq!(machine.memory.read(SCRATCH), 0);

    cpu.regs.pc = 0;
    cpu.regs.sp = 0xFF00;
    cpu.regs.set_flag(flag::C, true);
    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Ok { block: 1, bytes: 2 }
    );
    assert_eq!(machine.memory.read(SCRATCH), 0x11);
}

#[test]
fn the_trap_fails_a_block_whose_checksum_is_wrong() {
    let mut body = vec![DATA_FLAG, 0x01, 0x02];
    body.push(rkw_tape::checksum(&body) ^ 0xFF);
    let tap = Tap::builder().body(&body).build();
    let (mut cpu, mut machine) = ready_to_load(tap, DATA_FLAG, 2);

    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Rejected { block: 0 }
    );
    assert!(!cpu.regs.flag(flag::C));
    // The ROM writes the bytes as they arrive and only then discovers the
    // checksum is wrong, and so does this: what it does not do is claim to
    // have loaded them.
    assert_eq!(machine.memory.read(SCRATCH), 0x01);
}

#[test]
fn the_trap_fails_a_block_of_the_wrong_length() {
    let (mut cpu, mut machine) = ready_to_load(tap_of(&[0x01, 0x02, 0x03]), DATA_FLAG, 2);
    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Rejected { block: 0 }
    );
    assert!(!cpu.regs.flag(flag::C));
}

#[test]
fn the_trap_has_nothing_to_do_without_a_tape() {
    let mut machine = Spectrum::new();
    let mut cpu = Cpu::new();
    assert_eq!(ld_bytes(&mut cpu, &mut machine), Loaded::NoTape);
    // The PC is untouched, so the caller can let the ROM's own routine run and
    // wait for a tape that is not coming, exactly as a real machine does.
    assert_eq!(cpu.regs.pc, 0);
}

#[test]
fn verifying_compares_rather_than_writes() {
    let data = [0xA0, 0xB0];
    let (mut cpu, mut machine) = ready_to_load(tap_of(&data), DATA_FLAG, 2);
    machine.memory.write(SCRATCH, 0xA0);
    machine.memory.write(SCRATCH + 1, 0xB0);
    cpu.regs.set_flag(flag::C, false);

    assert!(matches!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Ok { .. }
    ));
    assert!(cpu.regs.flag(flag::C));

    // And a machine whose memory says something else fails the comparison.
    let (mut cpu, mut machine) = ready_to_load(tap_of(&data), DATA_FLAG, 2);
    machine.memory.write(SCRATCH, 0x00);
    cpu.regs.set_flag(flag::C, false);
    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Rejected { block: 0 }
    );
    assert_eq!(machine.memory.read(SCRATCH), 0x00, "verify writes nothing");
}

// ---------------------------------------------------------------------------
// The ROM itself, against the waveform.
// ---------------------------------------------------------------------------

/// The skip every ROM test here starts with.
macro_rules! rom {
    () => {
        match common::rom() {
            Some(rom) => rom,
            None => {
                eprintln!("{}", common::skip_message());
                return;
            }
        }
    };
}

/// A booted machine with `tap` in the deck, set up to call a ROM routine and
/// return to [`RETURN_TO`], where a `HALT` waits.
fn booted_with(rom: &[u8], tap: Tap) -> (Cpu, Spectrum) {
    let mut board = common::Board::new(rom);
    // Far enough for the ROM to have set its system variables up: `LD-BYTES`
    // and `SA-BYTES` both end in `SA/LD-RET`, which reads `BORDCR` and scans
    // the keyboard for BREAK.
    board.run_frames(150);
    let mut machine = board.machine;
    machine.mount_tape(Arc::new(tap));
    machine.memory.write(RETURN_TO, 0x76); // HALT
    (board.cpu, machine)
}

/// Enter a ROM routine as though it had been `CALL`ed from [`RETURN_TO`].
fn call(cpu: &mut Cpu, machine: &mut Spectrum, routine: u16) {
    cpu.regs.sp = 0xFF00;
    machine.memory.write(0xFF00, RETURN_TO as u8);
    machine.memory.write(0xFF01, (RETURN_TO >> 8) as u8);
    cpu.regs.pc = routine;
}

#[test]
fn the_rom_loads_a_block_off_the_waveform() {
    let rom = rom!();
    let data: Vec<u8> = (0..=255u8).collect();
    let (mut cpu, mut machine) = booted_with(&rom, tap_of(&data));

    cpu.regs.ix = SCRATCH;
    cpu.regs.set_de(data.len() as u16);
    cpu.regs.a = DATA_FLAG;
    cpu.regs.set_flag(flag::C, true);
    call(&mut cpu, &mut machine, rkw_spectrum::LD_BYTES);
    machine.play_tape();

    // The pilot alone is 3223 pulses of 2168 T-states, which is two seconds of
    // emulated time; ten is room for the whole block and a wrong answer.
    assert!(
        common::run_until_pc(&mut cpu, &mut machine, RETURN_TO, 10 * CLOCK_HZ),
        "LD-BYTES did not return"
    );
    assert!(cpu.regs.flag(flag::C), "the ROM reported a loading error");
    let loaded: Vec<u8> = (0..data.len())
        .map(|i| machine.memory.read(SCRATCH + i as u16))
        .collect();
    assert_eq!(loaded, data);
}

#[test]
fn the_rom_rejects_a_block_the_waveform_corrupted() {
    let rom = rom!();
    // One bit flipped in the middle of the data, which is what a dropout on a
    // real tape does and what the checksum is for.
    let mut body = vec![DATA_FLAG, 0x01, 0x02, 0x03];
    body.push(rkw_tape::checksum(&body));
    body[2] ^= 0x08;
    let (mut cpu, mut machine) = booted_with(&rom, Tap::builder().body(&body).build());

    cpu.regs.ix = SCRATCH;
    cpu.regs.set_de(3);
    cpu.regs.a = DATA_FLAG;
    cpu.regs.set_flag(flag::C, true);
    call(&mut cpu, &mut machine, rkw_spectrum::LD_BYTES);
    machine.play_tape();

    assert!(common::run_until_pc(
        &mut cpu,
        &mut machine,
        RETURN_TO,
        10 * CLOCK_HZ
    ));
    assert!(
        !cpu.regs.flag(flag::C),
        "a corrupt block loaded without complaint"
    );
}

#[test]
fn load_from_basic_reads_a_program_off_the_tape() {
    let rom = rom!();
    // 10 REM hello, as the ROM keeps it: line number big-endian, length
    // little-endian, then the tokens and a carriage return.
    let program = [
        0x00, 0x0A, 0x07, 0x00, 0xEA, b'h', b'e', b'l', b'l', b'o', 0x0D,
    ];
    let mut header = [0u8; 17];
    header[0] = 0; // a program
    header[1..11].copy_from_slice(b"hello     ");
    header[11..13].copy_from_slice(&(program.len() as u16).to_le_bytes());
    header[13..15].copy_from_slice(&0x8000u16.to_le_bytes()); // no autostart
    header[15..17].copy_from_slice(&(program.len() as u16).to_le_bytes());
    let tap = Tap::builder()
        .block(HEADER_FLAG, &header)
        .block(DATA_FLAG, &program)
        .build();

    let mut board = common::Board::new(&rom);
    board.run_frames(150);
    board.machine.mount_tape(Arc::new(tap));
    // `J` at the `K` cursor is the LOAD keyword, and a pair of quotes is an
    // empty file name: LOAD "".
    board.type_text("J\"\"\n");
    board.machine.play_tape();

    // The header's own pilot is 8063 pulses — nearly five seconds — and the
    // pause between the blocks is another second.
    for _ in 0..600 {
        board.run_frames(1);
        if board.screen_contains("0 OK") {
            break;
        }
    }
    assert!(
        board.screen_contains("0 OK"),
        "the load did not finish: {:?}",
        board.screen_text()
    );

    // PROG says where a program lives, and what is there is what was on the
    // tape.
    let prog = u16::from_le_bytes([
        board.machine.memory.read(0x5C53),
        board.machine.memory.read(0x5C54),
    ]);
    let loaded: Vec<u8> = (0..program.len())
        .map(|i| board.machine.memory.read(prog + i as u16))
        .collect();
    assert_eq!(loaded, program);
}

// ---------------------------------------------------------------------------
// Saving, and the round trip.
// ---------------------------------------------------------------------------

/// Save `data` with the ROM's `SA-BYTES` and return the tape that came out of
/// the `MIC` socket.
fn rom_save(rom: &[u8], flag_byte: u8, data: &[u8]) -> Tap {
    let (mut cpu, mut machine) = booted_with(rom, Tap::empty());
    for (i, &byte) in data.iter().enumerate() {
        machine.memory.write(SCRATCH + i as u16, byte);
    }
    cpu.regs.ix = SCRATCH;
    cpu.regs.set_de(data.len() as u16);
    cpu.regs.a = flag_byte;
    call(&mut cpu, &mut machine, SA_BYTES);

    let mut saving = Saving::new(machine);
    assert!(
        common::run_until_pc(&mut cpu, &mut saving, RETURN_TO, 20 * CLOCK_HZ),
        "SA-BYTES did not return"
    );
    // A block is not published until the silence after it has been waited out,
    // and the recorder only notices silence when a frame ends.
    let end = saving.t_states() + 4 * T_STATES_PER_FRAME;
    common::run_to(&mut cpu, &mut saving, end);

    assert_eq!(saving.recorder().dropped(), 0);
    assert_eq!(saving.recorder().lost_blocks(), 0);
    saving.to_tap()
}

#[test]
fn the_rom_saves_a_block_a_real_spectrum_would_load() {
    let rom = rom!();
    let data: Vec<u8> = (0..64u8).map(|i| i.wrapping_mul(7)).collect();

    let saved = rom_save(&rom, DATA_FLAG, &data);
    assert_eq!(saved.len(), 1);
    let block = saved.block(0).expect("one block");
    assert_eq!(block.flag(), DATA_FLAG);
    assert_eq!(block.data(), &data[..]);
    assert!(block.checksum_ok());
    // Bit for bit what this crate would have written for the same data, which
    // is the sense in which a real machine would load it.
    assert_eq!(saved, Tap::builder().block(DATA_FLAG, &data).build());
}

#[test]
fn what_the_rom_saves_the_rom_loads_again() {
    let rom = rom!();
    let data: Vec<u8> = (0..200u8).map(|i| i ^ 0x5A).collect();
    let saved = rom_save(&rom, DATA_FLAG, &data);

    let (mut cpu, mut machine) = booted_with(&rom, saved);
    cpu.regs.ix = SCRATCH;
    cpu.regs.set_de(data.len() as u16);
    cpu.regs.a = DATA_FLAG;
    cpu.regs.set_flag(flag::C, true);
    call(&mut cpu, &mut machine, rkw_spectrum::LD_BYTES);
    machine.play_tape();

    assert!(common::run_until_pc(
        &mut cpu,
        &mut machine,
        RETURN_TO,
        10 * CLOCK_HZ
    ));
    assert!(cpu.regs.flag(flag::C), "the round trip did not load");
    let loaded: Vec<u8> = (0..data.len())
        .map(|i| machine.memory.read(SCRATCH + i as u16))
        .collect();
    assert_eq!(loaded, data);
}

#[test]
fn a_header_saves_with_the_longer_pilot_and_reads_back_as_a_header() {
    let rom = rom!();
    let mut header = [0u8; 17];
    header[0] = 3; // code
    header[1..11].copy_from_slice(b"pilot     ");
    header[11..13].copy_from_slice(&256u16.to_le_bytes());
    header[13..15].copy_from_slice(&SCRATCH.to_le_bytes());

    let saved = rom_save(&rom, HEADER_FLAG, &header);
    let block = saved.block(0).expect("one block");
    assert!(block.is_header());
    assert_eq!(block.header().expect("a header").name(), "pilot");
    assert_eq!(block.header().expect("a header").length, 256);
}

#[test]
fn a_beeper_tune_is_not_a_save() {
    // The two bits share the log and share the amplifier, and only bit 3 is
    // the tape. A recorder that watched bit 4 as well would find a block in
    // every piece of beeper music.
    let mut saving = Saving::new(Spectrum::new());
    let mut cpu = Cpu::new();
    for frame in 0..3u64 {
        // A square wave on the speaker at about a pilot pulse's period, which
        // is the closest a tune gets to looking like a tape.
        for i in 0..30u64 {
            let t = frame * T_STATES_PER_FRAME + i * 2168;
            saving
                .as_mut()
                .ula
                .write_port_fe(t, if i % 2 == 0 { 0x10 } else { 0x00 });
        }
        common::run_to(&mut cpu, &mut saving, (frame + 1) * T_STATES_PER_FRAME);
    }

    assert_eq!(saving.recorder().len(), 0);
    assert_eq!(saving.recorder().lost_blocks(), 0);
    assert!(!saving.recorder().recording());
}

#[test]
fn the_recorder_reads_the_mic_bit_through_the_wrapper() {
    // The other half of the previous test: the same log, the same wrapper, and
    // a waveform on bit 3 does produce a block.
    let data = [0x12, 0x34, 0x56];
    let tap = tap_of(&data);
    let timing = Timing::rom(CLOCK_HZ).with_pause(0);
    let mut saving = Saving::new(Spectrum::new());
    let mut cpu = Cpu::new();

    // Play the tape into the `MIC` bit, which is what a saving program does.
    let mut player = Player::new();
    let mut t = 0;
    while let Some(pulse) = player.next_pulse(&tap, &timing) {
        // At the clock the machine has actually reached, which is a `NOP` or
        // two past where the last pulse ended — the same few T-states of slop
        // a real program's `OUT` lands with.
        let now = saving.t_states();
        saving
            .as_mut()
            .ula
            .write_port_fe(now, if pulse.level { 0x08 } else { 0x00 });
        t += u64::from(pulse.ticks);
        common::run_to(&mut cpu, &mut saving, t);
    }
    common::run_to(&mut cpu, &mut saving, t + 4 * T_STATES_PER_FRAME);

    assert_eq!(saving.to_tap(), tap);
    assert_eq!(saving.recorder().dropped(), 0);
}

#[test]
fn a_saving_machine_wraps_a_sounding_one() {
    // The stack a front end will run: the recorder reads the edge log first,
    // the beeper reads it second, and the machine underneath ends the frame.
    use rkw_audio::ring;
    use rkw_spectrum::AudioMachine;

    let (tx, _rx) = ring::channel(4096);
    let audio = AudioMachine::with_defaults(Spectrum::new(), 48_000, tx);
    let mut saving = Saving::new(audio);
    let mut cpu = Cpu::new();

    saving.as_mut().mount_tape(Arc::new(tap_of(&[0x00])));
    saving.as_mut().play_tape();
    common::run_to(&mut cpu, &mut saving, 2 * T_STATES_PER_FRAME);

    assert_eq!(saving.as_ref().ula.frames(), 2);
    assert!(saving.as_ref().tape.is_playing());
    assert_eq!(saving.recorder().len(), 0);
}
