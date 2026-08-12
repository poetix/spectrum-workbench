//! What the debugger costs when it is not doing anything.
//!
//! Timing is a benchmark rather than an assertion, so this is `#[ignore]`d and
//! wants a release build:
//!
//! ```text
//! cargo test --release --test throughput -- --ignored --nocapture
//! ```
//!
//! Each figure is the best of several runs, which is the right statistic for a
//! throughput microbenchmark: the fastest run is the one least disturbed by
//! everything else the machine was doing.
//!
//! # Reading the numbers
//!
//! Absolute figures from this file are not comparable with figures from
//! anywhere else, because of an inlining effect worth knowing about. When a
//! binary contains exactly one call site for `Cpu::step` at a given bus type,
//! LLVM inlines the whole interpreter into that loop — inlining into a sole
//! call site costs no code growth — and it then runs at around 270 M
//! instructions/s on the development machine. Add a second call site anywhere
//! and both fall back to a real call, at around 165. This file has several, so
//! every figure here is in the second regime. What *is* comparable is the
//! ratio between figures measured here, which is what the assertions use.
//!
//! # What is asserted
//!
//! That attaching the debugger with nothing armed costs nothing, and that
//! arming execution breakpoints which do not fire costs nothing either. Both
//! run the machine's own bus through the same loop, so the only per-
//! instruction addition is the bit test of ADR-0008.
//!
//! A memory or port watchpoint is a different matter, and is reported rather
//! than asserted tightly: watching the bus means the CPU runs against a
//! wrapper that reaches the real bus through a pointer, and that indirection —
//! not the bit test it carries — is what a watchpoint costs. In the
//! sole-call-site regime it is around 40%; here it is a few percent, because
//! everything in this binary has already lost the inlining for other reasons.
//!
//! Measured ratios against the bare core on the development machine are 0.99
//! to 1.01 for both, so the bounds below have about the width of the noise.
//!
//! The deterministic half of the acceptance criterion — that nothing reaches
//! tier 3 while nothing is firing — is asserted in `breakpoints.rs` and
//! `no_alloc.rs`, where a busy machine cannot perturb it.

mod common;

use std::time::Instant;

use common::machine;
use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu};
use rkw_debug::{Debugger, StopReason};
use z80::{Cpu, FlatMemory};

/// A loop with a read, a write and a branch, staying inside 256 bytes of data
/// so it cannot write over itself however long it runs.
const BUSY: &[u8] = &[
    0x21, 0x00, 0x90, // restart: LD HL,$9000
    0x06, 0x00, //       LD B,0
    0x7E, //             loop: LD A,(HL)
    0x3C, //             INC A
    0x77, //             LD (HL),A
    0x23, //             INC HL
    0x10, 0xFA, //       DJNZ loop
    0xC3, 0x00, 0x80, // JP restart
];

const N: u64 = 20_000_000;
const ROUNDS: usize = 9;

fn bare(n: u64) -> f64 {
    let (mut cpu, mut mem) = machine(BUSY);
    let start = Instant::now();
    for _ in 0..n {
        cpu.step(&mut mem);
    }
    rate(n, start, &cpu, &mem)
}

fn attached(n: u64, arm: impl FnOnce(&mut Debugger)) -> f64 {
    let (mut cpu, mut mem) = machine(BUSY);
    let mut dbg = Debugger::new();
    arm(&mut dbg);
    let start = Instant::now();
    let stop = dbg.resume(&mut cpu, &mut mem, n);
    let rate = rate(n, start, &cpu, &mem);
    assert_eq!(stop, StopReason::OutOfBudget, "the run should not stop");
    assert_eq!(dbg.breakpoints.detail_probes(), 0, "nothing should fire");
    rate
}

/// Millions of instructions per second. The CPU and memory are read from so
/// that the work cannot be optimised away.
fn rate(n: u64, start: Instant, cpu: &Cpu, mem: &FlatMemory) -> f64 {
    let elapsed = start.elapsed().as_secs_f64();
    std::hint::black_box((cpu.regs.hl(), mem.ram[0x9000]));
    n as f64 / elapsed / 1e6
}

fn best(mut f: impl FnMut() -> f64) -> f64 {
    let mut best = 0.0f64;
    for _ in 0..ROUNDS {
        best = best.max(f());
    }
    best
}

#[test]
#[ignore = "a timing measurement; use --release --nocapture"]
fn the_debugger_costs_almost_nothing_when_nothing_fires() {
    // The first pass pays for first-touching the address space.
    bare(N / 10);

    let bare_rate = best(|| bare(N));
    let idle_rate = best(|| attached(N, |_| {}));
    let armed_rate = best(|| {
        attached(N, |dbg| {
            // Addresses the program never executes.
            for addr in [0x1234u16, 0x4321, 0xC000] {
                dbg.breakpoints.add_exec(addr);
            }
        })
    });
    let watched_rate = best(|| {
        attached(N, |dbg| {
            // An address the program never touches, and a port it never uses.
            dbg.breakpoints.watch_mem(0xA000, true, true);
            dbg.breakpoints.watch_port(0x00FF, 0x00FE, true, true);
        })
    });

    println!("bare core                   {bare_rate:8.1} M instructions/s");
    println!("attached, nothing armed     {idle_rate:8.1} M instructions/s");
    println!("attached, breakpoints armed {armed_rate:8.1} M instructions/s");
    println!("attached, watchpoints armed {watched_rate:8.1} M instructions/s");

    assert!(
        idle_rate > bare_rate * 0.93,
        "attaching the debugger cost more than a few percent: \
         {bare_rate:.1} -> {idle_rate:.1}"
    );
    assert!(
        armed_rate > bare_rate * 0.93,
        "arming breakpoints that never fire cost more than a few percent: \
         {bare_rate:.1} -> {armed_rate:.1}"
    );
    // Loose, because it is measuring the wrapper rather than the check. A lost
    // tier — a hash probe per instruction — would be a five- to tenfold
    // slowdown, which this would still catch.
    assert!(
        watched_rate > bare_rate * 0.5,
        "watching the bus cost more than wrapping it should: \
         {bare_rate:.1} -> {watched_rate:.1}"
    );
}

/// T-states rather than instructions, because the slice loop is paced by the
/// clock and the two units are not interchangeable.
const T_STATES: u64 = 700_000_000;

/// Run in one go, as a baseline: the same work with the control tick taken
/// out.
fn free_running(target: u64) -> f64 {
    let (mut cpu, mut mem) = machine(BUSY);
    let mut dbg = Debugger::new();
    let start = Instant::now();
    while mem.t < target {
        dbg.resume(&mut cpu, &mut mem, 100_000);
    }
    mhz(mem.t, start, &cpu, &mem)
}

/// Run the same work through the slice loop, draining an empty command ring
/// at every tick.
fn sliced(target: u64, interval: u64) -> f64 {
    let (cpu, mem) = machine(BUSY);
    let (mut emu, mut handle) = Emu::new(
        cpu,
        mem,
        Debugger::new(),
        Config {
            control_interval: interval,
            ..Config::default()
        },
    );
    handle.send(Command::Resume).unwrap();
    let start = Instant::now();
    while emu.machine.t < target {
        emu.slice();
    }
    mhz(emu.machine.t, start, &emu.cpu, &emu.machine)
}

/// Emulated megahertz: how fast the machine's own clock runs.
fn mhz(t: u64, start: Instant, cpu: &Cpu, mem: &FlatMemory) -> f64 {
    let elapsed = start.elapsed().as_secs_f64();
    std::hint::black_box((cpu.regs.hl(), mem.ram[0x9000]));
    t as f64 / elapsed / 1e6
}

/// What handing control back once per scanline costs.
///
/// ADR-0007 puts the figure at under 1%, on the grounds that a slice boundary
/// is a comparison the loop needs anyway and a drain of an empty ring is two
/// atomic loads. This is that claim, measured. The scanline column is the one
/// that matters; the others are there to show the shape — the cost is per
/// tick, so it only becomes visible when the ticks are absurdly close
/// together.
///
/// The measured figures have the sliced loop a few percent *ahead* of the
/// free-running one, which is the inlining effect described at the top of this
/// file rather than slicing being free money. Read the assertions as bounds:
/// what they rule out is a control tick that costs something worth naming.
#[test]
#[ignore = "a timing measurement; use --release --nocapture"]
fn slicing_at_scanline_granularity_costs_almost_nothing() {
    free_running(T_STATES / 10);

    let free = best(|| free_running(T_STATES));
    let scanline = best(|| sliced(T_STATES, 224));
    let frame = best(|| sliced(T_STATES, 69_888));
    let tight = best(|| sliced(T_STATES, 16));

    println!("free running                {free:8.1} emulated MHz");
    println!("sliced per frame            {frame:8.1} emulated MHz");
    println!("sliced per scanline         {scanline:8.1} emulated MHz");
    println!("sliced every 16 T-states    {tight:8.1} emulated MHz");
    println!("real Spectrum                    3.5 emulated MHz");

    assert!(
        scanline > free * 0.97,
        "a control tick per scanline cost more than a few percent: \
         {free:.1} -> {scanline:.1}"
    );
    assert!(
        frame > free * 0.97,
        "a control tick per frame cost anything at all: {free:.1} -> {frame:.1}"
    );
}
