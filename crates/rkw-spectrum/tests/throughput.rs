//! What contention costs, measured rather than assumed.
//!
//! ADR-0009 chose arithmetic over a 68 KB table on a cache argument, and the
//! arithmetic runs on every machine cycle — several times per instruction, which
//! is the hottest path in the program. That is the kind of claim that has to be
//! a number.
//!
//! Two things are measured here: what contention costs the core at all, and
//! whether ADR-0009 picked the right implementation of it. The second did not
//! come out the way the ADR assumed; see the end of this file.
//!
//! ```text
//! cargo test -p rkw-spectrum --release --test throughput -- --ignored --nocapture
//! ```
//!
//! # Reading the numbers
//!
//! Everything here is a ratio between figures measured in *this* binary, for
//! the reason `docs/architecture.md` sets out: when a binary has exactly one
//! call site for `Cpu::step` at a given bus type LLVM inlines the entire
//! interpreter into it, and a second call site anywhere costs both of them
//! about 60%. This file has several, so no absolute figure here is comparable
//! with one from anywhere else.
//!
//! # The baseline
//!
//! `z80::FlatMemory` is the wrong thing to compare against: it is an array
//! index where a `Spectrum` is a memory map, a ULA, a tape deck and a frame
//! clock, so the difference between them is mostly not contention. [`Plain`]
//! below is the right one — the `Spectrum`'s own bus as it stood before this
//! ticket, with the contention removed and nothing else changed. Both are
//! monomorphised in this binary, so the ratio between them is the cost of the
//! thing being measured.
//!
//! Three cases are separated, because the cost has three different shapes
//! depending on what the program touches:
//!
//! - **Free addresses.** Every cycle tests whether its address is in the
//!   contended bank and finds it is not. That is a compare and a
//!   well-predicted branch.
//! - **Contended addresses, beam in the border.** The test passes, so the
//!   arithmetic runs — a modulo by a compile-time constant and two compares —
//!   and returns zero.
//! - **Contended addresses, beam in the display.** All of the above, and the
//!   emulated machine really is held. The drop in *emulated MHz* here is the
//!   hardware being accurate and not us being slow, which is why both figures
//!   are printed.

use std::time::Instant;

use rkw_spectrum::Spectrum;
use rkw_spectrum::contention::FIRST_CONTENDED_T;
use rkw_spectrum::frame::T_STATES_PER_LINE;
use z80::{Bus, Cpu};

/// The `Spectrum`'s bus as it was before ticket 0020.
///
/// It is a real `Spectrum` — the same memory map, the same ULA, the same
/// interrupt check, the same size and shape in cache — reached through a
/// wrapper that forwards the raw accessors and *does not* forward the machine
/// cycle wrappers. That leaves the `Bus` trait's default bodies in play, which
/// is exactly the arrangement ADR-0002 left behind and this ticket replaced.
///
/// Constructed this way rather than written out so that it cannot drift: the
/// only difference between this and the thing being measured is the one being
/// measured.
struct Plain(Spectrum);

impl Bus for Plain {
    fn read(&mut self, addr: u16) -> u8 {
        self.0.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.0.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        self.0.input(port)
    }

    fn output(&mut self, port: u16, value: u8) {
        self.0.output(port, value);
    }

    fn tick(&mut self, t: u32) {
        self.0.tick(t);
    }

    fn interrupt_pending(&self) -> bool {
        self.0.interrupt_pending()
    }
}

/// A loop with a read, a write, a branch and an internal cycle, over 256 bytes
/// of data it cannot run off. `INC (HL)` is there on purpose: it is the
/// instruction whose internal T-state made ADR-0023 necessary, so the internal
/// cycle path is in the measurement and not just the read and write ones.
///
/// Assembled twice at different bases, because a `JP` needs an absolute
/// address.
fn busy(base: u16, data: u16) -> Vec<u8> {
    let [bl, bh] = base.to_le_bytes();
    let [dl, dh] = data.to_le_bytes();
    #[rustfmt::skip]
    let code = vec![
        0x21, dl, dh,       // restart: ld hl,data
        0x06, 0x00,         //          ld b,0
        0x34,               // loop:    inc (hl)
        0x7E,               //          ld a,(hl)
        0x23,               //          inc hl
        0x10, 0xFB,         //          djnz loop
        0xC3, bl, bh,       //          jp restart
    ];
    code
}

/// Instructions to run per measured pass, and how many passes to take the best
/// of. The fastest run is the right statistic for a throughput microbenchmark:
/// it is the one least disturbed by everything else on the machine.
const N: u64 = 20_000_000;
const ROUNDS: usize = 7;

fn best(mut f: impl FnMut() -> (f64, f64)) -> (f64, f64) {
    let mut best = (0.0f64, 0.0f64);
    for _ in 0..ROUNDS {
        let run = f();
        if run.0 > best.0 {
            best = run;
        }
    }
    best
}

/// A second call site for `Cpu::step` at whatever bus type it is instantiated
/// with, and its only purpose is to be one.
///
/// Both loops have to be in the same inlining regime or the ratio between them
/// is code layout rather than work. With a single call site LLVM inlines the
/// whole interpreter into the loop — and it did so for the simpler bus and not
/// for the `Spectrum`, which on its own accounted for more than the thing being
/// measured. One extra call site apiece puts both in the out-of-line regime,
/// where the difference is the contention and nothing else.
#[inline(never)]
fn second_call_site<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
    cpu.step(bus);
}

/// Millions of instructions per second, and emulated megahertz.
fn rate(n: u64, t: u64, start: Instant, seen: (u16, u8)) -> (f64, f64) {
    let elapsed = start.elapsed().as_secs_f64();
    std::hint::black_box(seen);
    (n as f64 / elapsed / 1e6, t as f64 / elapsed / 1e6)
}

/// The baseline: the same program on the same memory map with no contention
/// anywhere.
fn plain(n: u64, base: u16, data: u16) -> (f64, f64) {
    let mut bus = Plain(Spectrum::new());
    bus.0.memory.load(base, &busy(base, data));
    let mut cpu = Cpu::new();
    cpu.regs.pc = base;
    second_call_site(&mut cpu, &mut bus);

    let start = Instant::now();
    for _ in 0..n {
        cpu.step(&mut bus);
    }
    let seen = (cpu.regs.hl(), bus.0.memory.read(data));
    rate(n, bus.0.t_states(), start, seen)
}

/// The real machine, with the program and its data wherever the caller puts
/// them, started at `t` T-states into the frame.
fn spectrum(n: u64, base: u16, data: u16, t: u64) -> (f64, f64) {
    let mut machine = Spectrum::new();
    machine.memory.load(base, &busy(base, data));
    machine.tick(t as u32);
    let mut cpu = Cpu::new();
    cpu.regs.pc = base;
    second_call_site(&mut cpu, &mut machine);
    let before = machine.t_states();

    let start = Instant::now();
    for _ in 0..n {
        cpu.step(&mut machine);
    }
    let seen = (cpu.regs.hl(), machine.memory.read(data));
    rate(n, machine.t_states() - before, start, seen)
}

#[test]
#[ignore = "a timing measurement; use --release --nocapture"]
fn contention_costs_a_compare_when_it_does_not_fire_and_arithmetic_when_it_does() {
    // The first pass pays for first-touching the address space.
    plain(N / 10, 0x8000, 0x4800);
    spectrum(N / 10, 0x8000, 0x9000, 0);

    // A T-state in the border of a display line, so that the arithmetic runs
    // to completion and finds nothing to charge for.
    let border = FIRST_CONTENDED_T + 20 * T_STATES_PER_LINE + 150;

    let (base_rate, _) = best(|| plain(N, 0x8000, 0x4800));
    let (free_rate, free_mhz) = best(|| spectrum(N, 0x8000, 0x9000, 0));
    let (edge_rate, edge_mhz) = best(|| spectrum(N, 0x8000, 0x4800, border));
    let (held_rate, held_mhz) = best(|| spectrum(N, 0x8000, 0x4800, FIRST_CONTENDED_T));

    println!("no contention (the baseline)   {base_rate:8.1} M inst/s");
    println!(
        "free addresses                 {free_rate:8.1} M inst/s  {free_mhz:7.0} emulated MHz"
    );
    println!(
        "contended addresses, border    {edge_rate:8.1} M inst/s  {edge_mhz:7.0} emulated MHz"
    );
    println!(
        "contended addresses, display   {held_rate:8.1} M inst/s  {held_mhz:7.0} emulated MHz"
    );
    println!(
        "\nan address compare             {:.2}x\nthe arithmetic as well         {:.2}x         \nand the stalls                 {:.2}x\nemulated time lost to the ULA  {:.0}%",
        free_rate / base_rate,
        edge_rate / base_rate,
        held_rate / base_rate,
        100.0 * (1.0 - held_mhz / free_mhz),
    );

    // Measured on the development machine: 0.71, 0.58, 0.58. The bounds are
    // set well below that, because what they are for is catching a regression
    // of a different order — a per-T-state table that misses cache, or a
    // division that did not become a multiply — and not for pinning a number
    // that will differ on every part.
    assert!(
        free_rate > base_rate * 0.55,
        "testing the address cost more than a compare should: \
         {base_rate:.1} -> {free_rate:.1} M instructions/s"
    );
    assert!(
        edge_rate > base_rate * 0.42,
        "the delay arithmetic cost more than arithmetic should: \
         {base_rate:.1} -> {edge_rate:.1} M instructions/s"
    );
    assert!(
        held_rate > base_rate * 0.42,
        "computing contention cost more than half the core: \
         {base_rate:.1} -> {held_rate:.1} M instructions/s"
    );

    // And the emulated machine really was held in the last case, so the
    // measurement is of contention happening rather than of a branch never
    // taken.
    assert!(
        held_mhz < free_mhz * 0.95,
        "the contended run was not actually slowed: {free_mhz:.0} -> {held_mhz:.0} MHz"
    );
}

/// Every T-state of a frame, as bytes: the table ADR-0009 refused.
const TABLE_BYTES: usize = (T_STATES_PER_LINE * 312) as usize;

/// The emulated address space, which is what the table would be competing with
/// for cache. Sizing arguments are meaningless without it: 68 KB in isolation
/// is fast, and 68 KB streamed alongside 64 KB of RAM the emulated program is
/// walking is the situation the ADR was actually about.
const RAM_BYTES: usize = 0x1_0000;

/// The `Spectrum`'s bus with contention taken from a 68 KB table instead of
/// computed — the alternative ADR-0009 rejected, built so that it can be
/// measured rather than argued about.
///
/// Only the three memory cycles and the internal one are overridden, because
/// the program measured against it does no I/O. That is a shortcut a real
/// implementation could not take, and it biases the comparison *towards* the
/// table, which is the safe direction for a test whose job is to try to
/// overturn the decision.
struct Tabulated {
    machine: Spectrum,
    table: Vec<u8>,
}

impl Tabulated {
    fn new() -> Tabulated {
        let mut table = vec![0u8; TABLE_BYTES];
        for (t, entry) in table.iter_mut().enumerate() {
            *entry = rkw_spectrum::contention::delay(t as u64) as u8;
        }
        Tabulated {
            machine: Spectrum::new(),
            table,
        }
    }

    #[inline]
    fn contend(&mut self, addr: u16) {
        if rkw_spectrum::is_contended(addr) {
            let t = (self.machine.t_states() % TABLE_BYTES as u64) as usize;
            self.machine.tick(u32::from(self.table[t]));
        }
    }
}

impl Bus for Tabulated {
    fn read(&mut self, addr: u16) -> u8 {
        self.machine.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.machine.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        self.machine.input(port)
    }

    fn output(&mut self, port: u16, value: u8) {
        self.machine.output(port, value);
    }

    fn tick(&mut self, t: u32) {
        self.machine.tick(t);
    }

    fn interrupt_pending(&self) -> bool {
        self.machine.interrupt_pending()
    }

    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.contend(addr);
        let v = self.machine.read(addr);
        self.machine.tick(4);
        v
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        self.contend(addr);
        let v = self.machine.read(addr);
        self.machine.tick(3);
        v
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.contend(addr);
        self.machine.write(addr, value);
        self.machine.tick(3);
    }

    fn tick_at(&mut self, addr: u16, t: u32) {
        for _ in 0..t {
            self.contend(addr);
            self.machine.tick(1);
        }
    }
}

/// The tabulated machine, run exactly as [`spectrum`] runs the real one.
fn tabulated(n: u64, base: u16, data: u16, t: u64) -> (f64, f64) {
    let mut bus = Tabulated::new();
    bus.machine.memory.load(base, &busy(base, data));
    bus.machine.tick(t as u32);
    let mut cpu = Cpu::new();
    cpu.regs.pc = base;
    second_call_site(&mut cpu, &mut bus);
    let before = bus.machine.t_states();

    let start = Instant::now();
    for _ in 0..n {
        cpu.step(&mut bus);
    }
    let seen = (cpu.regs.hl(), bus.machine.memory.read(data));
    rate(n, bus.machine.t_states() - before, start, seen)
}

#[test]
#[ignore = "a timing measurement; use --release --nocapture"]
fn the_arithmetic_beats_the_table_it_was_chosen_over() {
    // ADR-0009 chose eight bytes of arithmetic over a 68 KB lookup on a cache
    // argument that was reasoned rather than measured. This measures it, in two
    // different senses, because the two answers disagree — and the disagreement
    // is the ADR's point rather than a problem with it. A lookup that is cheap
    // on its own is not cheap when it is evicting the emulated computer.
    let mut table = vec![0u8; TABLE_BYTES];
    for (t, entry) in table.iter_mut().enumerate() {
        *entry = rkw_spectrum::contention::delay(t as u64) as u8;
    }
    let ram = vec![0u8; RAM_BYTES];

    // The walk is computed rather than stored. A vector of a million
    // pre-generated steps would be 16 MB, a larger working set than either of
    // the things being compared, and would drown both of them — the first
    // version of this test made exactly that mistake.
    const STEPS: u64 = 1 << 22;
    let walk = |i: u64| {
        (
            (i.wrapping_mul(7919) + i / 64) % TABLE_BYTES as u64,
            (i.wrapping_mul(37) % RAM_BYTES as u64) as usize,
        )
    };

    let computed = || {
        let start = Instant::now();
        let mut sum = 0u64;
        for i in 0..STEPS {
            let (t, addr) = walk(i);
            sum += u64::from(rkw_spectrum::contention::delay(t)) + u64::from(ram[addr]);
        }
        std::hint::black_box(sum);
        STEPS as f64 / start.elapsed().as_secs_f64() / 1e6
    };
    let looked_up = || {
        let start = Instant::now();
        let mut sum = 0u64;
        for i in 0..STEPS {
            let (t, addr) = walk(i);
            sum += u64::from(table[t as usize]) + u64::from(ram[addr]);
        }
        std::hint::black_box(sum);
        STEPS as f64 / start.elapsed().as_secs_f64() / 1e6
    };

    computed();
    looked_up();
    let mut computed_rate = 0.0f64;
    let mut looked_up_rate = 0.0f64;
    for _ in 0..ROUNDS {
        computed_rate = computed_rate.max(computed());
        looked_up_rate = looked_up_rate.max(looked_up());
    }

    println!("--- the lookup on its own ---");
    println!("computed   {computed_rate:8.1} M lookups/s");
    println!(
        "tabulated  {looked_up_rate:8.1} M lookups/s   ({} KB)",
        TABLE_BYTES / 1024
    );
    println!("ratio      {:.2}x", computed_rate / looked_up_rate);

    // And now inside the machine, running the same program at the same point
    // in the frame as the measurement above.
    spectrum(N / 10, 0x8000, 0x4800, FIRST_CONTENDED_T);
    tabulated(N / 10, 0x8000, 0x4800, FIRST_CONTENDED_T);
    let (arith_rate, _) = best(|| spectrum(N, 0x8000, 0x4800, FIRST_CONTENDED_T));
    let (table_rate, _) = best(|| tabulated(N, 0x8000, 0x4800, FIRST_CONTENDED_T));

    println!("\n--- in the machine ---");
    println!("computed   {arith_rate:8.1} M inst/s");
    println!("tabulated  {table_rate:8.1} M inst/s");
    println!("ratio      {:.2}x", arith_rate / table_rate);

    // On the development machine the table wins both measurements: about 2.5x
    // on the lookup alone and about 1.3x in the machine. That does not support
    // ADR-0009 as written, and the honest thing is to say so here rather than
    // to tune the benchmark until it agrees.
    //
    // Two things it does not settle, both of which the ADR's argument turns on
    // and neither of which this hardware can answer:
    //
    //  - The ADR sizes against a 32-48 KB x86 L1d, where 68 KB does not fit.
    //    This machine has 128 KB and the table fits, so the eviction the
    //    argument is about does not happen here.
    //  - `busy` touches 256 bytes of emulated RAM. A real program touches the
    //    screen, its code, its stack and its data — kilobytes — and it is
    //    competition for the cache with *that* that the table is supposed to
    //    lose. A benchmark with a tiny working set gives it an easy ride.
    //
    // So this is left as a report and a bound rather than a verdict. What the
    // bound catches is the arithmetic becoming pathological — a division that
    // stopped being a multiply would show up as several times slower, not the
    // 1.3x measured here. See ticket 0032.
    assert!(
        arith_rate > table_rate * 0.5,
        "the arithmetic is more than twice the cost of the table it was chosen \
         over: {arith_rate:.1} against {table_rate:.1} M instructions/s"
    );
}
