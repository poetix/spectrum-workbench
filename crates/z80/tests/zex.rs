//! Frank Cringle's Z80 instruction exerciser, `zexdoc` and `zexall`.
//!
//! Each of the ~70 test groups drives one instruction (or family) through a
//! large space of operand and flag combinations, CRCs every byte of resulting
//! register and flag state, and compares against a CRC taken from real
//! hardware. Where the Fuse suite checks one carefully chosen case per opcode,
//! this checks millions of arbitrary ones — which is what it takes to be
//! confident about the undocumented flag bits.
//!
//! `zexdoc` masks off the undocumented bits 3 and 5 before hashing; `zexall`
//! includes them, and additionally settles the `BIT n,(HL)` question the Fuse
//! suite cannot: it reaches `BIT` through instruction sequences that leave WZ
//! holding a determinate value, so the flags it produces are checkable.
//!
//! These run for billions of T-states, so they are `#[ignore]`d and want a
//! release build:
//!
//! ```text
//! cargo test --release --test zex -- --ignored --nocapture
//! ```
//!
//! The images are CP/M `.com` files; see `scripts/fetch-testdata.sh`.

use std::path::PathBuf;
use std::time::Instant;

use z80::{Bus, Cpu};

/// Where CP/M loads a transient program, and therefore the entry point.
const TPA: u16 = 0x0100;
/// The BDOS entry vector. A call here is intercepted rather than executed.
const BDOS: u16 = 0x0005;
/// Programs return to CP/M by returning to address zero.
const EXIT: u16 = 0x0000;

struct CpmBus {
    ram: Box<[u8; 0x1_0000]>,
    t: u64,
}

impl Bus for CpmBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.ram[addr as usize]
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.ram[addr as usize] = value;
    }
    fn input(&mut self, _port: u16) -> u8 {
        0xFF
    }
    fn output(&mut self, _port: u16, _value: u8) {}
    fn tick(&mut self, t: u32) {
        self.t += u64::from(t);
    }
}

/// Console output, accumulated a line at a time so progress can be printed as
/// the exerciser produces it rather than only at the end.
#[derive(Default)]
struct Console {
    line: String,
    lines: Vec<String>,
}

impl Console {
    fn put(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                let line = std::mem::take(&mut self.line);
                let trimmed = line.trim_end().to_string();
                if !trimmed.is_empty() {
                    println!("    {trimmed}");
                }
                self.lines.push(trimmed);
            }
            b'\r' => {}
            _ => self.line.push(ch as char),
        }
    }

    fn finish(&mut self) {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            println!("    {}", line.trim_end());
            self.lines.push(line.trim_end().to_string());
        }
    }
}

/// Emulate the two BDOS calls the exerciser uses.
///
/// Function 2 writes the character in `E`; function 9 writes the `$`-terminated
/// string at `DE`. Everything else the exerciser never asks for.
fn bdos(cpu: &mut Cpu, bus: &mut CpmBus, console: &mut Console) {
    match cpu.regs.c {
        2 => console.put(cpu.regs.e),
        9 => {
            let mut addr = cpu.regs.de();
            // Bounded so a runaway pointer cannot spin forever.
            for _ in 0..0x1_0000 {
                let ch = bus.ram[addr as usize];
                if ch == b'$' {
                    break;
                }
                console.put(ch);
                addr = addr.wrapping_add(1);
            }
        }
        other => panic!("unimplemented BDOS call {other}"),
    }

    // Return to the caller. The exerciser reaches BDOS through CALL, so the
    // return address is on the stack.
    let lo = bus.ram[cpu.regs.sp as usize];
    let hi = bus.ram[cpu.regs.sp.wrapping_add(1) as usize];
    cpu.regs.sp = cpu.regs.sp.wrapping_add(2);
    cpu.regs.pc = u16::from_le_bytes([lo, hi]);
}

struct Outcome {
    lines: Vec<String>,
    instructions: u64,
    tstates: u64,
    elapsed: f64,
}

fn run(image: &[u8]) -> Outcome {
    let mut bus = CpmBus {
        ram: Box::new([0; 0x1_0000]),
        t: 0,
    };
    bus.ram[TPA as usize..TPA as usize + image.len()].copy_from_slice(image);

    let mut cpu = Cpu::new();
    cpu.reset();
    cpu.regs.pc = TPA;
    // Leave the stack somewhere harmless with a return to zero on top, so the
    // exerciser's final RET lands on the exit address.
    cpu.regs.sp = 0xF000;
    bus.ram[0xF000] = (EXIT & 0xFF) as u8;
    bus.ram[0xF001] = (EXIT >> 8) as u8;

    let mut console = Console::default();
    let start = Instant::now();

    loop {
        match cpu.regs.pc {
            BDOS => bdos(&mut cpu, &mut bus, &mut console),
            EXIT => break,
            _ => {
                cpu.step(&mut bus);
            }
        }
    }

    console.finish();
    Outcome {
        lines: console.lines,
        instructions: cpu.instructions,
        tstates: bus.t,
        elapsed: start.elapsed().as_secs_f64(),
    }
}

fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/zex")
        .join(name);
    std::fs::read(path).ok()
}

fn exercise(name: &str) {
    let Some(image) = fixture(name) else {
        eprintln!("skipping {name}: not found, run scripts/fetch-testdata.sh");
        return;
    };

    println!("running {name} ({} bytes)", image.len());
    let outcome = run(&image);

    println!(
        "\n  {} instructions, {} T-states, {:.1}s wall ({:.0}M instructions/sec)",
        outcome.instructions,
        outcome.tstates,
        outcome.elapsed,
        outcome.instructions as f64 / outcome.elapsed / 1e6,
    );

    // Each group prints either "...OK" or "... ERROR **** crc expected ...".
    let errors: Vec<&String> = outcome
        .lines
        .iter()
        .filter(|l| l.contains("ERROR"))
        .collect();
    let passed = outcome.lines.iter().filter(|l| l.contains("OK")).count();

    assert!(
        passed > 0,
        "{name} produced no results at all; the harness is wrong, not the CPU"
    );
    assert!(
        errors.is_empty(),
        "{name}: {} of {} groups failed:\n{}",
        errors.len(),
        passed + errors.len(),
        errors
            .iter()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    println!("  {name}: all {passed} groups passed");
}

/// Documented flags only.
#[test]
#[ignore = "runs for billions of T-states; use --release"]
fn zexdoc() {
    exercise("zexdoc.com");
}

/// Documented and undocumented flags, including the X/Y bits and MEMPTR.
#[test]
#[ignore = "runs for billions of T-states; use --release"]
fn zexall() {
    exercise("zexall.com");
}
