//! The Fuse emulator's Z80 conformance suite.
//!
//! 1335 cases, each specifying a complete starting state and the exact state
//! that should result — registers, interrupt flags, memory, total T-states,
//! and the sequence of bus cycles with the time each one completed.
//!
//! That last part is what makes this suite worth more here than a CRC-based
//! one: it pins down *where* each machine cycle sits inside an instruction,
//! not merely how many there are in total. Contended memory is defined as
//! wait states inserted at particular cycles, so a core that gets the totals
//! right but the placement wrong will produce plausible, subtly wrong timing
//! the moment a ULA is attached. This catches that now.
//!
//! The data is not vendored; run `scripts/fetch-testdata.sh` first.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use z80::{Bus, Cpu, InterruptMode, Regs};

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

/// A bus cycle, as the suite records it. Only the accesses are compared;
/// the contention markers (`MC`, `PC`) describe wait states this core does
/// not model yet.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    time: u64,
    kind: String,
    addr: u16,
    data: Option<u8>,
}

impl Event {
    fn is_access(&self) -> bool {
        matches!(self.kind.as_str(), "MR" | "MW" | "PR" | "PW")
    }

    fn render(&self) -> String {
        match self.data {
            Some(d) => format!("{:5} {} {:04x} {:02x}", self.time, self.kind, self.addr, d),
            None => format!("{:5} {} {:04x}", self.time, self.kind, self.addr),
        }
    }
}

/// The register and interrupt state shared by the input and expected files.
#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
    af: u16,
    bc: u16,
    de: u16,
    hl: u16,
    af_: u16,
    bc_: u16,
    de_: u16,
    hl_: u16,
    ix: u16,
    iy: u16,
    sp: u16,
    pc: u16,
    i: u8,
    r: u8,
    iff1: bool,
    iff2: bool,
    im: u8,
    halted: bool,
    /// In an input file this is the point at which to stop running; in an
    /// expected file it is the total the run should have reached.
    tstates: u64,
}

#[derive(Debug, Clone)]
struct Case {
    name: String,
    state: State,
    memory: Vec<(u16, Vec<u8>)>,
    events: Vec<Event>,
}

fn parse_state(regs_line: &str, state_line: &str) -> State {
    let r: Vec<u16> = regs_line
        .split_whitespace()
        .map(|w| u16::from_str_radix(w, 16).expect("register field"))
        .collect();
    assert_eq!(r.len(), 12, "expected 12 register fields in {regs_line:?}");

    let s: Vec<&str> = state_line.split_whitespace().collect();
    assert_eq!(s.len(), 7, "expected 7 state fields in {state_line:?}");

    State {
        af: r[0],
        bc: r[1],
        de: r[2],
        hl: r[3],
        af_: r[4],
        bc_: r[5],
        de_: r[6],
        hl_: r[7],
        ix: r[8],
        iy: r[9],
        sp: r[10],
        pc: r[11],
        i: u8::from_str_radix(s[0], 16).expect("i"),
        r: u8::from_str_radix(s[1], 16).expect("r"),
        iff1: s[2] != "0",
        iff2: s[3] != "0",
        im: s[4].parse().expect("im"),
        halted: s[5] != "0",
        tstates: s[6].parse().expect("tstates"),
    }
}

/// `addr b b b ... -1`
fn parse_memory_line(line: &str) -> (u16, Vec<u8>) {
    let mut words = line.split_whitespace();
    let addr = u16::from_str_radix(words.next().expect("memory address"), 16).expect("address");
    let bytes = words
        .take_while(|w| *w != "-1")
        .map(|w| u8::from_str_radix(w, 16).expect("memory byte"))
        .collect();
    (addr, bytes)
}

/// The input file: name, registers, state, then memory blocks terminated by a
/// line holding only `-1`.
fn parse_input(text: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }

        let regs_line = lines.next().expect("register line");
        let state_line = lines.next().expect("state line");
        let state = parse_state(regs_line, state_line);

        let mut memory = Vec::new();
        for mem_line in lines.by_ref() {
            let mem_line = mem_line.trim();
            if mem_line == "-1" || mem_line.is_empty() {
                break;
            }
            memory.push(parse_memory_line(mem_line));
        }

        cases.push(Case {
            name: name.to_string(),
            state,
            memory,
            events: Vec::new(),
        });
    }

    cases
}

/// The expected file: name, indented event lines, registers, state, then any
/// memory blocks, terminated by a blank line.
fn parse_expected(text: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }

        // Event lines are indented; the register line that follows them is not.
        let mut events = Vec::new();
        while lines
            .peek()
            .is_some_and(|l| l.starts_with(' ') || l.starts_with('\t'))
        {
            let fields: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
            events.push(Event {
                time: fields[0].parse().expect("event time"),
                kind: fields[1].to_string(),
                addr: u16::from_str_radix(fields[2], 16).expect("event address"),
                data: fields
                    .get(3)
                    .map(|d| u8::from_str_radix(d, 16).expect("event data")),
            });
        }

        let regs_line = lines.next().expect("register line");
        let state_line = lines.next().expect("state line");
        let state = parse_state(regs_line, state_line);

        let mut memory = Vec::new();
        while let Some(mem_line) = lines.peek() {
            if mem_line.trim().is_empty() {
                break;
            }
            memory.push(parse_memory_line(lines.next().unwrap().trim()));
        }

        cases.push(Case {
            name: name.to_string(),
            state,
            memory,
            events,
        });
    }

    cases
}

// ---------------------------------------------------------------------------
// The bus the suite expects
// ---------------------------------------------------------------------------

/// Records the time at which each bus cycle completed.
///
/// The cycle wrappers are overridden rather than the raw accessors because the
/// suite's timestamps are cycle *end* times for memory and the `IORQ` instant
/// for ports, which is precisely the distinction the wrappers encode.
struct TestBus {
    ram: Box<[u8; 0x1_0000]>,
    t: u64,
    events: Vec<Event>,
}

impl TestBus {
    fn new() -> Self {
        Self {
            ram: Box::new([0; 0x1_0000]),
            t: 0,
            events: Vec::new(),
        }
    }

    fn log(&mut self, kind: &str, addr: u16, data: u8) {
        self.events.push(Event {
            time: self.t,
            kind: kind.to_string(),
            addr,
            data: Some(data),
        });
    }
}

impl Bus for TestBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.ram[addr as usize]
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.ram[addr as usize] = value;
    }

    /// Fuse's harness answers every port with the high half of the port
    /// address, which makes each read distinguishable in the expected output.
    fn input(&mut self, port: u16) -> u8 {
        (port >> 8) as u8
    }

    fn output(&mut self, _port: u16, _value: u8) {}

    fn tick(&mut self, t: u32) {
        self.t += u64::from(t);
    }

    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        let v = self.ram[addr as usize];
        self.tick(4);
        self.log("MR", addr, v);
        v
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        let v = self.ram[addr as usize];
        self.tick(3);
        self.log("MR", addr, v);
        v
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.ram[addr as usize] = value;
        self.tick(3);
        self.log("MW", addr, value);
    }

    fn input_cycle(&mut self, port: u16) -> u8 {
        self.tick(1);
        let v = (port >> 8) as u8;
        self.log("PR", port, v);
        self.tick(3);
        v
    }

    fn output_cycle(&mut self, port: u16, value: u8) {
        self.tick(1);
        self.log("PW", port, value);
        self.tick(3);
    }
}

// ---------------------------------------------------------------------------
// Running a case
// ---------------------------------------------------------------------------

fn run_case(input: &Case) -> (Cpu, TestBus) {
    let mut bus = TestBus::new();
    for (addr, bytes) in &input.memory {
        for (i, b) in bytes.iter().enumerate() {
            bus.ram[(addr.wrapping_add(i as u16)) as usize] = *b;
        }
    }

    let s = &input.state;
    let mut cpu = Cpu::new();
    cpu.regs = Regs {
        i: s.i,
        r: s.r,
        iff1: s.iff1,
        iff2: s.iff2,
        im: match s.im {
            0 => InterruptMode::Im0,
            1 => InterruptMode::Im1,
            _ => InterruptMode::Im2,
        },
        halted: s.halted,
        ix: s.ix,
        iy: s.iy,
        sp: s.sp,
        pc: s.pc,
        wz: 0,
        // The suite gives a starting F but not how the machine got there. The
        // ordinary way to arrive at a given F is to have just executed an
        // instruction that wrote it, so Q starts equal to F. That is also the
        // assumption baked into the expected results: this suite predates the
        // discovery of Q, and Fuse's SCF/CCF take their undocumented bits from
        // A alone, which is the Q-was-written case.
        q: s.af as u8,
        ..Default::default()
    };
    cpu.regs.set_af(s.af);
    cpu.regs.set_bc(s.bc);
    cpu.regs.set_de(s.de);
    cpu.regs.set_hl(s.hl);
    cpu.regs.a_ = (s.af_ >> 8) as u8;
    cpu.regs.f_ = s.af_ as u8;
    cpu.regs.b_ = (s.bc_ >> 8) as u8;
    cpu.regs.c_ = s.bc_ as u8;
    cpu.regs.d_ = (s.de_ >> 8) as u8;
    cpu.regs.e_ = s.de_ as u8;
    cpu.regs.h_ = (s.hl_ >> 8) as u8;
    cpu.regs.l_ = s.hl_ as u8;

    // The suite's runner executes whole instructions until the clock reaches
    // the requested point, which is how the repeating block instructions get
    // to run their many iterations.
    while bus.t < s.tstates {
        cpu.step(&mut bus);
    }

    (cpu, bus)
}

/// Compare one case, returning a description of every difference found.
fn compare(input: &Case, expected: &Case, cpu: &Cpu, bus: &TestBus) -> Vec<String> {
    let mut diffs = Vec::new();
    let r = &cpu.regs;
    let e = &expected.state;

    let mut cmp16 = |name: &str, got: u16, want: u16| {
        if got != want {
            diffs.push(format!("{name}: got {got:04x}, want {want:04x}"));
        }
    };
    cmp16("af", r.af(), e.af);
    cmp16("bc", r.bc(), e.bc);
    cmp16("de", r.de(), e.de);
    cmp16("hl", r.hl(), e.hl);
    cmp16("af'", u16::from_be_bytes([r.a_, r.f_]), e.af_);
    cmp16("bc'", u16::from_be_bytes([r.b_, r.c_]), e.bc_);
    cmp16("de'", u16::from_be_bytes([r.d_, r.e_]), e.de_);
    cmp16("hl'", u16::from_be_bytes([r.h_, r.l_]), e.hl_);
    cmp16("ix", r.ix, e.ix);
    cmp16("iy", r.iy, e.iy);
    cmp16("sp", r.sp, e.sp);

    // While halted this core leaves PC on the HALT instruction, which is where
    // the real chip keeps re-fetching from and what a debugger should show.
    // Fuse instead advances past it and compensates elsewhere; the two agree
    // on every externally visible effect, including the address an interrupt
    // pushes, so the difference is normalised away here.
    let pc = if r.halted { r.pc.wrapping_add(1) } else { r.pc };
    cmp16("pc", pc, e.pc);

    if r.i != e.i {
        diffs.push(format!("i: got {:02x}, want {:02x}", r.i, e.i));
    }
    if r.r != e.r {
        diffs.push(format!("r: got {:02x}, want {:02x}", r.r, e.r));
    }
    if r.iff1 != e.iff1 {
        diffs.push(format!("iff1: got {}, want {}", r.iff1, e.iff1));
    }
    if r.iff2 != e.iff2 {
        diffs.push(format!("iff2: got {}, want {}", r.iff2, e.iff2));
    }
    let im = match r.im {
        InterruptMode::Im0 => 0,
        InterruptMode::Im1 => 1,
        InterruptMode::Im2 => 2,
    };
    if im != e.im {
        diffs.push(format!("im: got {im}, want {}", e.im));
    }
    if r.halted != e.halted {
        diffs.push(format!("halted: got {}, want {}", r.halted, e.halted));
    }
    if bus.t != e.tstates {
        diffs.push(format!("tstates: got {}, want {}", bus.t, e.tstates));
    }

    // Memory: the expected file lists only the blocks it cares about, but a
    // wrong write elsewhere would be invisible, so compare against the initial
    // image overlaid with the expected blocks.
    let mut want_mem: BTreeMap<u16, u8> = BTreeMap::new();
    for (addr, bytes) in &input.memory {
        for (i, b) in bytes.iter().enumerate() {
            want_mem.insert(addr.wrapping_add(i as u16), *b);
        }
    }
    for (addr, bytes) in &expected.memory {
        for (i, b) in bytes.iter().enumerate() {
            want_mem.insert(addr.wrapping_add(i as u16), *b);
        }
    }
    for (addr, want) in want_mem {
        let got = bus.ram[addr as usize];
        if got != want {
            diffs.push(format!("mem[{addr:04x}]: got {got:02x}, want {want:02x}"));
        }
    }

    diffs.extend(compare_events(&bus.events, &expected.events));

    diffs
}

/// Compare the bus cycle sequences.
///
/// Fuse's own core skips memory reads whose result it does not need — the
/// operand bytes of a conditional jump or call that is not taken, and the
/// displacement byte of the final `DJNZ` pass. It still spends the machine
/// cycle, so its log holds the contention marker without the matching read.
/// A real Z80 performs those reads; the timing is identical either way, and
/// this core does them.
///
/// So an extra `MR` on our side is accepted when the expected log has an `MC`
/// at the same address exactly three T-states earlier — that is, when Fuse
/// spent the same read cycle but recorded only its start.
fn compare_events(got_all: &[Event], want_all: &[Event]) -> Vec<String> {
    let got: Vec<&Event> = got_all.iter().filter(|e| e.is_access()).collect();
    let want: Vec<&Event> = want_all.iter().filter(|e| e.is_access()).collect();

    let elided = |e: &Event| {
        e.kind == "MR"
            && want_all
                .iter()
                .any(|m| m.kind == "MC" && m.addr == e.addr && m.time + 3 == e.time)
    };

    let mut diffs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < got.len() && j < want.len() {
        if got[i] == want[j] {
            i += 1;
            j += 1;
        } else if elided(got[i]) {
            i += 1;
        } else {
            diffs.push(format!(
                "bus cycle {j}: got `{}`, want `{}`",
                got[i].render(),
                want[j].render()
            ));
            return diffs; // one desync makes every later cycle wrong too
        }
    }

    // Any tail on our side must be reads Fuse elided; any tail on theirs is a
    // cycle we failed to perform.
    for e in &got[i..] {
        if !elided(e) {
            diffs.push(format!("unexpected bus cycle `{}`", e.render()));
        }
    }
    for e in &want[j..] {
        diffs.push(format!("missing bus cycle `{}`", e.render()));
    }

    diffs
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fuse")
}

fn load() -> Option<(Vec<Case>, Vec<Case>)> {
    let dir = fixture_dir();
    let input = std::fs::read_to_string(dir.join("tests.in")).ok()?;
    let expected = std::fs::read_to_string(dir.join("tests.expected")).ok()?;
    Some((parse_input(&input), parse_expected(&expected)))
}

/// Cases where this core deliberately disagrees with the suite.
///
/// All four are `BIT n,(HL)`. The suite dates from before MEMPTR was
/// understood, and Fuse copied the undocumented X/Y flags from the byte read
/// out of memory. The researched behaviour is that they come from the high
/// half of the internal WZ pointer, which `BIT n,(HL)` does not itself modify
/// — so the correct result depends on state the input files do not specify,
/// and these cases cannot be satisfied by a MEMPTR-accurate core.
///
/// `zexall` settles this properly: it reaches `BIT` through instruction
/// sequences that give WZ a determinate value, so it can check the flags
/// against real hardware. The other `BIT n,(HL)` cases in this suite pass only
/// because their operands happen to have bits 3 and 5 clear.
const KNOWN_DIVERGENCES: &[&str] = &["cb4e", "cb5e", "cb6e", "cb76"];

/// Registers, flags, memory and total timing.
#[test]
fn fuse_suite() {
    let Some((inputs, expected)) = load() else {
        eprintln!(
            "skipping: Fuse test data not found in {}\n\
             run scripts/fetch-testdata.sh to install it",
            fixture_dir().display()
        );
        return;
    };

    assert_eq!(
        inputs.len(),
        expected.len(),
        "input and expected files disagree on the number of cases"
    );
    assert!(inputs.len() > 1300, "only parsed {} cases", inputs.len());

    let mut failures = Vec::new();
    let mut divergences_seen = Vec::new();
    for (input, want) in inputs.iter().zip(expected.iter()) {
        assert_eq!(input.name, want.name, "case order differs between files");
        let (cpu, bus) = run_case(input);
        let diffs = compare(input, want, &cpu, &bus);
        if diffs.is_empty() {
            continue;
        }
        if KNOWN_DIVERGENCES.contains(&input.name.as_str()) {
            divergences_seen.push(input.name.clone());
        } else {
            failures.push((input.name.clone(), diffs));
        }
    }

    if !failures.is_empty() {
        let mut report = format!(
            "{} of {} Fuse cases failed\n\n",
            failures.len(),
            inputs.len()
        );
        for (name, diffs) in failures.iter().take(25) {
            let _ = writeln!(report, "  {name}:");
            for d in diffs.iter().take(6) {
                let _ = writeln!(report, "      {d}");
            }
        }
        if failures.len() > 25 {
            let _ = writeln!(report, "\n  ... and {} more", failures.len() - 25);
        }
        panic!("{report}");
    }

    // A divergence that starts passing is as much a change worth knowing about
    // as one that starts failing, so the set is pinned rather than ignored.
    assert_eq!(
        divergences_seen, KNOWN_DIVERGENCES,
        "the set of cases that disagree with the suite has changed"
    );

    println!(
        "{} Fuse cases passed, {} known divergences",
        inputs.len() - divergences_seen.len(),
        divergences_seen.len()
    );
}
