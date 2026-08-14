//! TZX against the ROM.
//!
//! The unit tests in `rkw-tape` check that each block type produces the pulses
//! the format says it does; they cannot check that those pulses are a tape,
//! because the only thing that has an opinion about that is a loader. So these
//! are the same tests done the other way round: build a waveform out of turbo
//! blocks, pure tones, bare pulse sequences and a sampled recording, play it at
//! the real `LD-BYTES`, and require the bytes to arrive.
//!
//! What that catches is everything an assertion about pulse lengths cannot: a
//! pilot too short for the ROM to lock on to, a sync pair in the wrong order, a
//! last bit with no edge after it, a pause that swallowed the block behind it.
//!
//! The tests skip without a ROM, as everything else here does — see
//! `scripts/fetch-rom.sh`.

mod common;

use rkw_debug::machine::Machine;
use rkw_spectrum::frame::CLOCK_HZ;
use rkw_spectrum::{Loaded, Spectrum, ld_bytes};
use rkw_tape::tap::{DATA_FLAG, HEADER_FLAG, checksum};
use rkw_tape::tzx::Turbo;
use rkw_tape::{Image, Player, Timing, Tzx};
use z80::{Bus, Cpu, flag};

/// Where a test's own data goes: clear of the screen, the system variables and
/// anything the ROM touches.
const SCRATCH: u16 = 0x9000;

/// The sentinel a called ROM routine returns to, where a `HALT` waits.
const RETURN_TO: u16 = 0x8000;

/// Pilot pulses these tests write in front of a block.
///
/// The ROM is strict about this and the number is not arbitrary: `LD-START`
/// finds one edge, waits out about a second of pilot without looking at it,
/// and only then counts the 256 consecutive pilot pulses `LD-LEADER` wants.
/// Under about 1900 pulses of 2168 T-states there is nothing left by the time
/// it starts counting, and the routine waits for a tape that has moved on.
///
/// 2500 is over that and well under the 3223 the ROM writes, which is the
/// point: the number comes off the block rather than out of the ROM's
/// constants, and a player that used its own would run past the sync pair.
const PILOT_PULSES: u16 = 2500;

/// The skip every test here starts with.
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

/// A block body as the ROM writes one: a flag, the data, and the XOR of both.
fn body(flag: u8, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(data.len() + 2);
    body.push(flag);
    body.extend_from_slice(data);
    body.push(checksum(&body));
    body
}

/// A booted machine with `image` in the deck, set up to call a ROM routine and
/// return to [`RETURN_TO`].
fn booted_with(rom: &[u8], image: impl Into<Image>) -> (Cpu, Spectrum) {
    let mut board = common::Board::new(rom);
    // Far enough for the ROM to have set up the system variables `LD-BYTES`
    // reads on its way out.
    board.run_frames(150);
    let mut machine = board.machine;
    machine.mount_tape(image);
    machine.memory.write(RETURN_TO, 0x76); // HALT
    (board.cpu, machine)
}

/// Call `LD-BYTES` at a played tape and say whether it came back with carry
/// set, which is the ROM's way of saying the block loaded.
fn load(cpu: &mut Cpu, machine: &mut Spectrum, into: u16, length: u16, flag: u8) -> bool {
    cpu.regs.ix = into;
    cpu.regs.set_de(length);
    cpu.regs.a = flag;
    cpu.regs.set_flag(flag::C, true);
    cpu.regs.sp = 0xFF00;
    machine.memory.write(0xFF00, RETURN_TO as u8);
    machine.memory.write(0xFF01, (RETURN_TO >> 8) as u8);
    cpu.regs.pc = rkw_spectrum::LD_BYTES;
    machine.play_tape();

    // Ten seconds of emulated time: the pilot alone is most of one, and the
    // rest is room for a wrong answer to be wrong in.
    assert!(
        common::run_until_pc(cpu, machine, RETURN_TO, 10 * CLOCK_HZ),
        "LD-BYTES did not return"
    );
    cpu.regs.flag(flag::C)
}

/// Run the machine's schedule, with no CPU in it, until the clock reaches `t`.
/// What the tape does between two loads is the deck's business and not the
/// ROM's.
fn tick_to(machine: &mut Spectrum, t: u64) {
    while machine.t_states() < t {
        let event = machine.next_event().expect("the ULA schedules frames");
        let step = event.min(t).saturating_sub(machine.t_states());
        machine.tick(step as u32);
        if machine.t_states() >= event {
            machine.service_event();
        }
    }
}

fn loaded(machine: &Spectrum, at: u16, length: usize) -> Vec<u8> {
    (0..length)
        .map(|i| machine.memory.read(at + i as u16))
        .collect()
}

#[test]
fn the_rom_loads_a_standard_speed_block_off_a_tzx() {
    let rom = rom!();
    let data: Vec<u8> = (0..=255u8).collect();
    let tzx = Tzx::builder()
        .archive_info(&[(0x00, "a test tape")])
        .text("this block is not a waveform")
        .block(DATA_FLAG, &data, 100)
        .build();
    let (mut cpu, mut machine) = booted_with(&rom, tzx);

    // The text and archive info blocks come first and the loader never sees
    // them: a tape image that made them audible would fail here.
    assert!(load(
        &mut cpu,
        &mut machine,
        SCRATCH,
        data.len() as u16,
        DATA_FLAG
    ));
    assert_eq!(loaded(&machine, SCRATCH, data.len()), data);
}

#[test]
fn the_rom_loads_a_turbo_block_at_the_lengths_the_block_gives() {
    let rom = rom!();
    let data: Vec<u8> = (0..64u8).collect();
    // The ROM's own pulse lengths, because those are the ones it can read —
    // but a third of the pilot it would have written itself, which is a number
    // that exists nowhere but in this block.
    let turbo = Turbo {
        pilot_pulses: PILOT_PULSES,
        pause_ms: 100,
        ..Turbo::default()
    };
    let tzx = Tzx::builder()
        .turbo(&turbo, &body(DATA_FLAG, &data))
        .build();
    let (mut cpu, mut machine) = booted_with(&rom, tzx);

    assert!(load(
        &mut cpu,
        &mut machine,
        SCRATCH,
        data.len() as u16,
        DATA_FLAG
    ));
    assert_eq!(loaded(&machine, SCRATCH, data.len()), data);
}

#[test]
fn a_block_built_out_of_a_tone_a_pulse_pair_and_pure_data_is_a_block() {
    let rom = rom!();
    let data: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(7)).collect();
    // What a custom loader's tape looks like written down: the pilot, the sync
    // pair and the data are three separate blocks, and only the waveform they
    // add up to is a block at all. Nothing here says "flag byte" or "pilot" —
    // it is a run of pulses of 2168, two more of 667 and 735, and then bits.
    let tzx = Tzx::builder()
        .tone(2168, PILOT_PULSES)
        .pulses(&[667, 735])
        .pure_data(855, 1710, 8, 100, &body(DATA_FLAG, &data))
        .build();
    let (mut cpu, mut machine) = booted_with(&rom, tzx);

    assert!(load(
        &mut cpu,
        &mut machine,
        SCRATCH,
        data.len() as u16,
        DATA_FLAG
    ));
    assert_eq!(loaded(&machine, SCRATCH, data.len()), data);
}

#[test]
fn the_rom_loads_a_block_out_of_a_direct_recording() {
    let rom = rom!();
    let data: Vec<u8> = (0..32u8).collect();
    // A millisecond of pause, which is the edge that ends the last bit and
    // nothing more: a recording made without one is a recording of a block
    // whose last bit never finished.
    let turbo = Turbo {
        pilot_pulses: PILOT_PULSES,
        pause_ms: 1,
        ..Turbo::default()
    };
    let played = Tzx::builder()
        .turbo(&turbo, &body(DATA_FLAG, &data))
        .build();

    // Sample that waveform at 70 T-states — a shade under 50 kHz, which is
    // what a real recording of a tape is — and put the samples back on a tape
    // as a direct recording. The rounding is the interesting part: every pulse
    // lands on a sample boundary, so what the ROM times is up to 70 T-states
    // out on every edge, and its thresholds have to be wide enough to absorb
    // that. They are, and this is what says so.
    const EACH: u16 = 70;
    let samples = sample(&played, EACH);
    let tzx = Tzx::builder().direct(EACH, 100, 8, &samples).build();
    let (mut cpu, mut machine) = booted_with(&rom, tzx);

    assert!(load(
        &mut cpu,
        &mut machine,
        SCRATCH,
        data.len() as u16,
        DATA_FLAG
    ));
    assert_eq!(loaded(&machine, SCRATCH, data.len()), data);
}

/// Play `tzx` and write down what the line was doing every `each` T-states,
/// most significant bit first — which is a `0x15` direct recording of it.
fn sample(tzx: &Tzx, each: u16) -> Vec<u8> {
    let timing = Timing::rom(CLOCK_HZ);
    let mut player = Player::new();
    let mut bits = Vec::new();
    while let Some(pulse) = player.next_pulse(tzx, &timing) {
        for _ in 0..pulse.ticks.div_ceil(u32::from(each)) {
            bits.push(pulse.level);
        }
    }
    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |byte, (i, &bit)| byte | (u8::from(bit) << (7 - i)))
        })
        .collect()
}

#[test]
fn load_from_basic_reads_a_program_off_a_tzx() {
    let rom = rom!();
    // 10 REM hello, as the ROM keeps it, and the header that says so.
    let program = [
        0x00, 0x0A, 0x07, 0x00, 0xEA, b'h', b'e', b'l', b'l', b'o', 0x0D,
    ];
    let mut header = [0u8; 17];
    header[1..11].copy_from_slice(b"hello     ");
    header[11..13].copy_from_slice(&(program.len() as u16).to_le_bytes());
    header[13..15].copy_from_slice(&0x8000u16.to_le_bytes()); // no autostart
    header[15..17].copy_from_slice(&(program.len() as u16).to_le_bytes());

    // A tape as a real one is laid out: a group around the loader, a message
    // for the person watching, an unknown block from a later version of the
    // format, and the two blocks that matter. Only the last two make a sound.
    let tzx = Tzx::builder()
        .group_start("hello")
        .text("a program")
        .unknown(0x19, &[0; 32])
        .block(HEADER_FLAG, &header, 100)
        .block(DATA_FLAG, &program, 100)
        .group_end()
        .build();

    let mut board = common::Board::new(&rom);
    board.run_frames(150);
    board.machine.mount_tape(tzx);
    // `J` at the `K` cursor is the LOAD keyword, and a pair of quotes is an
    // empty file name: LOAD "".
    board.type_text("J\"\"\n");
    board.machine.play_tape();

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

    let prog = u16::from_le_bytes([
        board.machine.memory.read(0x5C53),
        board.machine.memory.read(0x5C54),
    ]);
    assert_eq!(loaded(&board.machine, prog, program.len()), program);
}

#[test]
fn a_tape_that_stops_itself_waits_to_be_started_again() {
    let rom = rom!();
    let first: Vec<u8> = (0..16u8).collect();
    let second: Vec<u8> = (16..32u8).collect();
    // What a two-part game writes: load one block, stop, wait for the person
    // to be ready, load the next.
    let tzx = Tzx::builder()
        .block(DATA_FLAG, &first, 100)
        .pause(0)
        .block(DATA_FLAG, &second, 100)
        .build();
    let (mut cpu, mut machine) = booted_with(&rom, tzx);

    assert!(load(&mut cpu, &mut machine, SCRATCH, 16, DATA_FLAG));
    assert_eq!(loaded(&machine, SCRATCH, 16), first);

    // `LD-BYTES` returns as soon as the checksum is in, which is a tenth of a
    // second of pause before the block that stops the tape. A real deck keeps
    // turning, so the clock has to as well.
    let second_later = machine.t_states() + CLOCK_HZ;
    tick_to(&mut machine, second_later);
    assert!(machine.tape.stopped_by_tape(), "the tape stopped itself");
    assert!(
        !machine.tape.finished(),
        "and it is not the end of the tape"
    );

    // Starting it again picks up at the block after the one that stopped it.
    assert!(load(&mut cpu, &mut machine, SCRATCH + 16, 16, DATA_FLAG));
    assert_eq!(loaded(&machine, SCRATCH + 16, 16), second);
}

#[test]
fn the_trap_reads_past_the_blocks_a_loader_cannot_hear() {
    // The accelerated `LD-BYTES` works in bytes rather than pulses, so a TZX's
    // text and control blocks are not something it can play past: it has to
    // skip them itself, or a tape whose first block is its title never loads.
    let data: Vec<u8> = (0..8u8).collect();
    let tzx = Tzx::builder()
        .text("a title")
        .tone(2168, 100)
        .block(DATA_FLAG, &data, 0)
        .build();
    let mut machine = Spectrum::new();
    machine.mount_tape(tzx);
    let mut cpu = Cpu::new();
    cpu.regs.sp = 0xFF00;
    cpu.regs.ix = SCRATCH;
    cpu.regs.set_de(data.len() as u16);
    cpu.regs.a = DATA_FLAG;
    cpu.regs.set_flag(flag::C, true);

    assert_eq!(
        ld_bytes(&mut cpu, &mut machine),
        Loaded::Ok {
            block: 2,
            bytes: data.len() as u16
        }
    );
    assert_eq!(loaded(&machine, SCRATCH, data.len()), data);
    assert_eq!(
        machine.tape.block(),
        3,
        "the head is past the block it read"
    );
}
