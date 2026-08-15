//! Play a note on the emulated beeper and write it to a `.wav`, so that the
//! sound can be listened to before there is a front end to play it through.
//!
//! ```sh
//! cargo run --example beep -p rkw-spectrum
//! cargo run --example beep -p rkw-spectrum -- --speaker flat --delay 264
//! afplay beep.wav
//! ```
//!
//! Ticket 0019 opens the audio device; nothing here does. Everything up to
//! that point is exercised, though — the same `Emu<AudioMachine>` the front
//! end will spawn, running the same Z80 loop through the same resampler and
//! speaker model — so what comes out of this file is what will come out of the
//! speaker.

use std::fs::File;
use std::io::{BufWriter, Write};

use rkw_audio::beeper::Config;
use rkw_audio::filter::Speaker;
use rkw_audio::ring;
use rkw_debug::Debugger;
use rkw_debug::command::Command;
use rkw_debug::emu::{self, Emu, RunState};
use rkw_spectrum::{AudioMachine, CLOCK_HZ, Spectrum, T_STATES_PER_FRAME};
use z80::Cpu;

const SAMPLE_RATE: u32 = 48_000;

/// The tone loop from `tests/audio.rs`: one flip of bit 4 per pass, taking
/// `45 + 13(n−1)` T-states, so the note is `3_500_000 / (2 × that)`.
fn tone(delay: u8) -> Vec<u8> {
    vec![
        0x3E, 0x10, // ld a,$10
        0xD3, 0xFE, // loop: out ($fe),a
        0x06, delay, //       ld b,delay
        0x10, 0xFE, //       djnz -2
        0xEE, 0x10, //       xor $10
        0x18, 0xF6, //       jr loop
    ]
}

/// `EI` and `RETI` at the `IM 1` vector.
const HANDLER: &[u8] = &[0xFB, 0xED, 0x4D];

fn main() {
    let mut delay = 132u8;
    let mut speaker = Speaker::Piezo;
    let mut seconds = 2.0f64;
    let mut path = String::from("beep.wav");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--delay" => delay = args.next().and_then(|v| v.parse().ok()).unwrap_or(delay),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            "--out" => path = args.next().unwrap_or(path),
            "--speaker" => {
                speaker = match args.next().as_deref() {
                    Some("flat") => Speaker::Flat,
                    Some("tv") => Speaker::TvSpeaker,
                    _ => Speaker::Piezo,
                }
            }
            other => {
                eprintln!("unknown argument {other:?}");
                eprintln!(
                    "usage: beep [--delay N] [--seconds S] [--speaker piezo|tv|flat] [--out FILE]"
                );
                std::process::exit(2);
            }
        }
    }

    let half_period = 45.0 + 13.0 * (f64::from(delay) - 1.0);
    let note = CLOCK_HZ as f64 / (2.0 * half_period);
    println!("{note:.1} Hz, {speaker:?}, {seconds} s");

    let samples = play(delay, speaker, seconds);
    write_wav(&path, &samples).expect("could not write the wav");
    println!("{} samples to {path}", samples.len());
}

/// Run the machine for `seconds` of emulated time and collect what it played.
fn play(delay: u8, speaker: Speaker, seconds: f64) -> Vec<f32> {
    let frames = (seconds * CLOCK_HZ as f64 / T_STATES_PER_FRAME as f64) as u64;

    // Room for the whole run, so that nothing has to be drained mid-flight.
    let capacity = ((seconds * f64::from(SAMPLE_RATE)) as usize)
        .next_power_of_two()
        .max(1 << 12);
    let (tx, mut rx) = ring::channel(capacity);

    let mut spectrum = Spectrum::new();
    spectrum.memory.load(0x8000, &tone(delay));
    spectrum.memory.load(0x0038, HANDLER);

    let config = Config::new(CLOCK_HZ, T_STATES_PER_FRAME as u32, SAMPLE_RATE).speaker(speaker);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;

    let (mut e, mut handle) = Emu::new(
        cpu,
        AudioMachine::new(spectrum, config, tx),
        Debugger::new(),
        emu::Config::default(),
    );
    handle.send(Command::Resume).unwrap();

    while e.machine.spectrum.ula.frames() < frames {
        assert_eq!(e.slice(), RunState::Running);
    }
    if e.machine.dropped() > 0 {
        eprintln!("the ring overflowed by {} samples", e.machine.dropped());
    }

    let mut samples = vec![0.0f32; rx.len()];
    let n = rx.pop(&mut samples);
    samples.truncate(n);
    samples
}

/// A 16-bit mono RIFF/WAVE file. Sixteen bits because every player reads it,
/// and mono because there is one speaker in a Spectrum.
fn write_wav(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    let data_bytes = (samples.len() * 2) as u32;
    let byte_rate = SAMPLE_RATE * 2;

    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?; // PCM header length
    out.write_all(&1u16.to_le_bytes())?; // uncompressed
    out.write_all(&1u16.to_le_bytes())?; // one channel
    out.write_all(&SAMPLE_RATE.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?; // bytes per frame
    out.write_all(&16u16.to_le_bytes())?; // bits per sample
    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())?;

    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        out.write_all(&value.to_le_bytes())?;
    }
    out.flush()
}
