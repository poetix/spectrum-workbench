//! Disassembly.
//!
//! This mirrors the octal decode in [`crate::cpu`] arm for arm, deliberately:
//! the two are the same table read in opposite directions, and keeping their
//! shapes identical is what stops them drifting apart. A round-trip test
//! against the assembler will eventually pin that down mechanically.
//!
//! Beyond producing text, the disassembler reports what each instruction does
//! to control flow. A debugger needs that to implement "step over" (run to the
//! instruction after a `CALL`), "run to return", and to follow static branch
//! targets when disassembling forwards from an entry point.
//!
//! # Decoding and formatting are separate
//!
//! [`decode`] answers what an instruction *is* — its length, its effect on
//! control flow, whether it is undocumented — and allocates nothing, so it can
//! be called at emulation rate by the trace ring and by step-over. [`text`]
//! and [`write_text`] produce the human-readable form, which is a debugger
//! pane's concern rather than the emulation thread's. [`Instruction`] is the
//! two composed, for callers that want both.
//!
//! There is still only one opcode table. Both halves run the same walk,
//! generic over where the text goes: decoding sends it to a sink whose methods
//! are empty and compile away. A second table would be a second thing to keep
//! in step, which is the mistake this module is arranged to avoid.

use core::fmt::{self, Write as _};

/// What an instruction does to the program counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Falls through to the next instruction.
    Normal,
    /// Transfers control, possibly conditionally. `target` is `None` when it
    /// is only known at run time, as with `JP (HL)`.
    Jump {
        target: Option<u16>,
        conditional: bool,
    },
    /// Pushes a return address first. Step-over sets its breakpoint at the
    /// following instruction.
    Call {
        target: Option<u16>,
        conditional: bool,
    },
    Return {
        conditional: bool,
    },
    /// `RST n`, which is a call to a fixed low address.
    Rst(u16),
    /// Stops until an interrupt arrives.
    Halt,
    /// A repeating block instruction: re-executes in place until its counter
    /// runs out, so PC does not advance between iterations.
    Repeat,
}

impl Flow {
    /// True if execution can continue at the following address.
    pub fn falls_through(self) -> bool {
        match self {
            Flow::Normal | Flow::Halt | Flow::Repeat => true,
            Flow::Call { .. } | Flow::Rst(_) => true,
            Flow::Jump { conditional, .. } | Flow::Return { conditional } => conditional,
        }
    }
}

/// What one instruction is, without saying how it reads.
///
/// Everything a debugger needs in order to *move* — step over, run to return,
/// follow a branch, record a trace entry — and nothing that requires the heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded {
    pub addr: u16,
    /// Encoded length in bytes. Normally 1 to 4; a chain of `DD`/`FD` prefixes
    /// is longer, because the CPU treats each prefix as an instruction in its
    /// own right and so does this.
    pub len: u8,
    pub flow: Flow,
    /// True for opcodes outside the official instruction set: `SLL`, the
    /// `IXH`/`IXL` halves, the `DD CB` register-copy forms, duplicate `IM`
    /// encodings, and the `ED` page's two-byte no-ops.
    pub undocumented: bool,
}

impl Decoded {
    /// The address execution reaches by falling through this instruction, and
    /// so where a step-over breakpoint goes.
    pub fn next_addr(self) -> u16 {
        self.addr.wrapping_add(u16::from(self.len))
    }
}

/// One decoded instruction with its bytes and text: [`Decoded`] plus what it
/// costs to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub addr: u16,
    /// Encoded length in bytes, 1 to 4.
    pub len: u8,
    pub bytes: Vec<u8>,
    pub text: String,
    pub flow: Flow,
    /// True for opcodes outside the official instruction set: `SLL`, the
    /// `IXH`/`IXL` halves, the `DD CB` register-copy forms, duplicate `IM`
    /// encodings, and the `ED` page's two-byte no-ops.
    pub undocumented: bool,
}

impl Instruction {
    /// Fill in the bytes and text for an instruction already decoded at
    /// `d.addr`.
    pub fn render<P: Peek>(mem: &P, d: &Decoded) -> Instruction {
        let bytes = (0..u16::from(d.len))
            .map(|i| mem.peek(d.addr.wrapping_add(i)))
            .collect();
        Instruction {
            addr: d.addr,
            len: d.len,
            bytes,
            text: text(mem, d),
            flow: d.flow,
            undocumented: d.undocumented,
        }
    }

    /// The decoded form on its own.
    pub fn decoded(&self) -> Decoded {
        Decoded {
            addr: self.addr,
            len: self.len,
            flow: self.flow,
            undocumented: self.undocumented,
        }
    }

    /// `8000  DD 7E 05     LD A,(IX+$05)`
    pub fn listing_line(&self) -> String {
        let mut hex = String::new();
        for b in &self.bytes {
            let _ = write!(hex, "{b:02X} ");
        }
        format!("{:04X}  {:<12} {}", self.addr, hex.trim_end(), self.text)
    }
}

const R_NAMES: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const RP_NAMES: [&str; 4] = ["BC", "DE", "HL", "SP"];
const RP2_NAMES: [&str; 4] = ["BC", "DE", "HL", "AF"];
const CC_NAMES: [&str; 8] = ["NZ", "Z", "NC", "C", "PO", "PE", "P", "M"];
const ALU_NAMES: [&str; 8] = [
    "ADD A,", "ADC A,", "SUB ", "SBC A,", "AND ", "XOR ", "OR ", "CP ",
];
const ROT_NAMES: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SLL", "SRL"];
const BLOCK_NAMES: [[&str; 4]; 4] = [
    ["LDI", "CPI", "INI", "OUTI"],
    ["LDD", "CPD", "IND", "OUTD"],
    ["LDIR", "CPIR", "INIR", "OTIR"],
    ["LDDR", "CPDR", "INDR", "OTDR"],
];

pub fn hex8(v: u8) -> String {
    format!("${v:02X}")
}

pub fn hex16(v: u16) -> String {
    format!("${v:04X}")
}

/// Which register pair a `DD`/`FD` prefix has substituted for `HL`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Idx {
    Hl,
    Ix,
    Iy,
}

impl Idx {
    fn name(self) -> &'static str {
        match self {
            Idx::Hl => "HL",
            Idx::Ix => "IX",
            Idx::Iy => "IY",
        }
    }

    fn indexed(self) -> bool {
        self != Idx::Hl
    }
}

/// Where an instruction's text goes.
///
/// The walk emits through this rather than building strings, so that decoding
/// can instantiate it with [`Discard`] and pay nothing for text it does not
/// want. The vocabulary is deliberately small: everything the disassembler
/// prints is a fixed string, a hex number, a decimal digit, or an index
/// operand.
trait Sink {
    fn str(&mut self, s: &str);
    /// `$XX`
    fn hex8(&mut self, v: u8);
    /// `$XXXX`
    fn hex16(&mut self, v: u16);
    /// A small decimal number: the bit index of `BIT`/`RES`/`SET`, the mode of
    /// `IM`.
    fn dec(&mut self, v: u8);
    /// `(IX+$05)` / `(IX-$03)`, matching the sign the CPU applies.
    fn index(&mut self, idx: Idx, d: u8);
}

/// A [`Sink`] that keeps nothing. Every method is empty, so a decode-only walk
/// optimises down to the byte reads and the flow arithmetic.
struct Discard;

impl Sink for Discard {
    fn str(&mut self, _: &str) {}
    fn hex8(&mut self, _: u8) {}
    fn hex16(&mut self, _: u16) {}
    fn dec(&mut self, _: u8) {}
    fn index(&mut self, _: Idx, _: u8) {}
}

/// A [`Sink`] that writes text, holding on to the first write error rather
/// than making every arm of the decode return a `Result`.
struct Text<W: fmt::Write> {
    out: W,
    res: fmt::Result,
}

impl<W: fmt::Write> Text<W> {
    fn put(&mut self, args: fmt::Arguments<'_>) {
        if self.res.is_ok() {
            self.res = self.out.write_fmt(args);
        }
    }
}

impl<W: fmt::Write> Sink for Text<W> {
    fn str(&mut self, s: &str) {
        if self.res.is_ok() {
            self.res = self.out.write_str(s);
        }
    }

    fn hex8(&mut self, v: u8) {
        self.put(format_args!("${v:02X}"));
    }

    fn hex16(&mut self, v: u16) {
        self.put(format_args!("${v:04X}"));
    }

    fn dec(&mut self, v: u8) {
        self.put(format_args!("{v}"));
    }

    fn index(&mut self, idx: Idx, d: u8) {
        let signed = d as i8;
        if signed < 0 {
            self.put(format_args!(
                "({}-${:02X})",
                idx.name(),
                signed.unsigned_abs()
            ));
        } else {
            self.put(format_args!("({}+${:02X})", idx.name(), signed));
        }
    }
}

/// Byte source for the disassembler. A slice, a `FlatMemory`, or a live
/// emulated machine can all supply this without any of them being coupled to
/// the others.
pub trait Peek {
    fn peek(&self, addr: u16) -> u8;
}

impl<F: Fn(u16) -> u8> Peek for F {
    fn peek(&self, addr: u16) -> u8 {
        self(addr)
    }
}

impl Peek for [u8; 0x1_0000] {
    fn peek(&self, addr: u16) -> u8 {
        self[addr as usize]
    }
}

/// A slice read as though it starts at address 0.
impl Peek for &[u8] {
    fn peek(&self, addr: u16) -> u8 {
        self.get(addr as usize).copied().unwrap_or(0)
    }
}

impl Peek for crate::bus::FlatMemory {
    fn peek(&self, addr: u16) -> u8 {
        self.ram[addr as usize]
    }
}

/// Walks bytes forwards from `start`, emitting text as it goes.
///
/// Bytes are always read in encoding order, which is not always the order they
/// are printed in — `DD CB d op` reads its displacement before the opcode that
/// names the instruction. So an arm reads what it needs first, then emits.
struct Cursor<'a, P: Peek, S: Sink> {
    mem: &'a P,
    out: S,
    start: u16,
    pos: u16,
    undocumented: bool,
}

impl<'a, P: Peek, S: Sink> Cursor<'a, P, S> {
    fn new(mem: &'a P, start: u16, out: S) -> Self {
        Self {
            mem,
            out,
            start,
            pos: start,
            undocumented: false,
        }
    }

    fn byte(&mut self) -> u8 {
        let v = self.mem.peek(self.pos);
        self.pos = self.pos.wrapping_add(1);
        v
    }

    fn word(&mut self) -> u16 {
        let lo = self.byte();
        let hi = self.byte();
        u16::from_le_bytes([lo, hi])
    }

    /// A `JR`/`DJNZ` target, resolved against the address *after* the
    /// displacement byte.
    fn relative(&mut self) -> u16 {
        let d = self.byte() as i8;
        self.pos.wrapping_add(d as u16)
    }

    fn s(&mut self, s: &str) {
        self.out.str(s);
    }

    fn h8(&mut self, v: u8) {
        self.out.hex8(v);
    }

    fn h16(&mut self, v: u16) {
        self.out.hex16(v);
    }

    fn dec(&mut self, v: u8) {
        self.out.dec(v);
    }

    fn index(&mut self, idx: Idx, d: u8) {
        self.out.index(idx, d);
    }

    /// The `r[]` slot name, with `H`/`L` rewritten to the index halves.
    fn reg(&mut self, idx: Idx, slot: u8) {
        match (idx, slot) {
            (Idx::Hl, _) => self.s(R_NAMES[slot as usize]),
            (_, 4) => {
                self.s(idx.name());
                self.s("H");
            }
            (_, 5) => {
                self.s(idx.name());
                self.s("L");
            }
            (_, _) => self.s(R_NAMES[slot as usize]),
        }
    }

    fn finish(self, flow: Flow) -> Decoded {
        Decoded {
            addr: self.start,
            len: self.pos.wrapping_sub(self.start) as u8,
            flow,
            undocumented: self.undocumented,
        }
    }
}

/// Decode the instruction at `addr`, without allocating.
///
/// Never fails: every one of the 256 opcode values decodes to something, and
/// the genuinely inert ones come back as `NOP` marked undocumented.
pub fn decode<P: Peek>(mem: &P, addr: u16) -> Decoded {
    let mut c = Cursor::new(mem, addr, Discard);
    let flow = decode_op(&mut c, Idx::Hl);
    c.finish(flow)
}

/// Write the text of the instruction at `d.addr` to `out`.
///
/// Only `d.addr` is used: the bytes are re-read, which is a handful of loads
/// and keeps [`Decoded`] the size it needs to be for a trace record.
pub fn write_text<P: Peek, W: fmt::Write>(mem: &P, d: &Decoded, out: W) -> fmt::Result {
    let mut c = Cursor::new(mem, d.addr, Text { out, res: Ok(()) });
    decode_op(&mut c, Idx::Hl);
    c.out.res
}

/// The text of the instruction at `d.addr`.
pub fn text<P: Peek>(mem: &P, d: &Decoded) -> String {
    let mut s = String::new();
    // Writing to a String cannot fail.
    let _ = write_text(mem, d, &mut s);
    s
}

/// Decode the instruction at `addr`, with its bytes and text.
pub fn disassemble<P: Peek>(mem: &P, addr: u16) -> Instruction {
    Instruction::render(mem, &decode(mem, addr))
}

/// Decode `count` consecutive instructions starting at `addr`.
pub fn disassemble_range<P: Peek>(mem: &P, addr: u16, count: usize) -> Vec<Instruction> {
    let mut out = Vec::with_capacity(count);
    let mut pc = addr;
    for _ in 0..count {
        let insn = disassemble(mem, pc);
        pc = pc.wrapping_add(u16::from(insn.len));
        out.push(insn);
    }
    out
}

fn decode_op<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx) -> Flow {
    let op = c.byte();
    let x = op >> 6;
    let y = (op >> 3) & 7;
    let z = op & 7;
    let p = y >> 1;
    let q = y & 1;

    match x {
        0 => decode_x0(c, idx, y, z, p, q),
        1 => decode_load(c, idx, y, z),
        2 => {
            c.s(ALU_NAMES[y as usize]);
            operand_r(c, idx, z);
            Flow::Normal
        }
        _ => decode_x3(c, idx, y, z, p, q),
    }
}

/// Emit the `r[]` operand in slot `slot`, consuming a displacement byte if the
/// slot is `(HL)` under an index prefix.
fn operand_r<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx, slot: u8) {
    if slot == 6 {
        if idx.indexed() {
            let d = c.byte();
            c.index(idx, d);
        } else {
            c.s("(HL)");
        }
    } else {
        if idx.indexed() && (slot == 4 || slot == 5) {
            c.undocumented = true;
        }
        c.reg(idx, slot);
    }
}

fn decode_x0<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx, y: u8, z: u8, p: u8, q: u8) -> Flow {
    match z {
        0 => match y {
            0 => {
                c.s("NOP");
                Flow::Normal
            }
            1 => {
                c.s("EX AF,AF'");
                Flow::Normal
            }
            2 => {
                let t = c.relative();
                c.s("DJNZ ");
                c.h16(t);
                Flow::Jump {
                    target: Some(t),
                    conditional: true,
                }
            }
            3 => {
                let t = c.relative();
                c.s("JR ");
                c.h16(t);
                Flow::Jump {
                    target: Some(t),
                    conditional: false,
                }
            }
            _ => {
                let t = c.relative();
                c.s("JR ");
                c.s(CC_NAMES[(y - 4) as usize]);
                c.s(",");
                c.h16(t);
                Flow::Jump {
                    target: Some(t),
                    conditional: true,
                }
            }
        },
        1 => {
            if q == 0 {
                let nn = c.word();
                c.s("LD ");
                rp_name(c, p, idx);
                c.s(",");
                c.h16(nn);
            } else {
                c.s("ADD ");
                c.s(idx.name());
                c.s(",");
                rp_name(c, p, idx);
            }
            Flow::Normal
        }
        2 => decode_x0_z2(c, idx, p, q),
        3 => {
            c.s(if q == 0 { "INC " } else { "DEC " });
            rp_name(c, p, idx);
            Flow::Normal
        }
        4 | 5 => {
            c.s(if z == 4 { "INC " } else { "DEC " });
            operand_r(c, idx, y);
            Flow::Normal
        }
        6 => {
            // The displacement precedes the immediate byte in the encoding.
            c.s("LD ");
            operand_r(c, idx, y);
            let n = c.byte();
            c.s(",");
            c.h8(n);
            Flow::Normal
        }
        _ => {
            const NAMES: [&str; 8] = ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"];
            c.s(NAMES[y as usize]);
            Flow::Normal
        }
    }
}

fn decode_x0_z2<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx, p: u8, q: u8) -> Flow {
    match (q, p) {
        (0, 0) => c.s("LD (BC),A"),
        (0, 1) => c.s("LD (DE),A"),
        (0, 2) => {
            let nn = c.word();
            c.s("LD (");
            c.h16(nn);
            c.s("),");
            c.s(idx.name());
        }
        (0, _) => {
            let nn = c.word();
            c.s("LD (");
            c.h16(nn);
            c.s("),A");
        }
        (_, 0) => c.s("LD A,(BC)"),
        (_, 1) => c.s("LD A,(DE)"),
        (_, 2) => {
            let nn = c.word();
            c.s("LD ");
            c.s(idx.name());
            c.s(",(");
            c.h16(nn);
            c.s(")");
        }
        (_, _) => {
            let nn = c.word();
            c.s("LD A,(");
            c.h16(nn);
            c.s(")");
        }
    }
    Flow::Normal
}

fn decode_load<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx, y: u8, z: u8) -> Flow {
    if y == 6 && z == 6 {
        c.s("HALT");
        return Flow::Halt;
    }

    // With a memory operand the *other* operand keeps its unprefixed meaning,
    // so `DD 74` is `LD (IX+d),H` rather than `LD (IX+d),IXH`.
    if y == 6 {
        c.s("LD ");
        operand_r(c, idx, y);
        c.s(",");
        c.s(R_NAMES[z as usize]);
        return Flow::Normal;
    }
    if z == 6 {
        c.s("LD ");
        c.s(R_NAMES[y as usize]);
        c.s(",");
        operand_r(c, idx, z);
        return Flow::Normal;
    }

    if idx.indexed() && (matches!(y, 4 | 5) || matches!(z, 4 | 5)) {
        c.undocumented = true;
    }
    c.s("LD ");
    c.reg(idx, y);
    c.s(",");
    c.reg(idx, z);
    Flow::Normal
}

fn decode_x3<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx, y: u8, z: u8, p: u8, q: u8) -> Flow {
    match z {
        0 => {
            c.s("RET ");
            c.s(CC_NAMES[y as usize]);
            Flow::Return { conditional: true }
        }
        1 => {
            if q == 0 {
                c.s("POP ");
                rp2_name(c, p, idx);
                Flow::Normal
            } else {
                match p {
                    0 => {
                        c.s("RET");
                        Flow::Return { conditional: false }
                    }
                    1 => {
                        c.s("EXX");
                        Flow::Normal
                    }
                    2 => {
                        c.s("JP (");
                        c.s(idx.name());
                        c.s(")");
                        Flow::Jump {
                            target: None,
                            conditional: false,
                        }
                    }
                    _ => {
                        c.s("LD SP,");
                        c.s(idx.name());
                        Flow::Normal
                    }
                }
            }
        }
        2 => {
            let nn = c.word();
            c.s("JP ");
            c.s(CC_NAMES[y as usize]);
            c.s(",");
            c.h16(nn);
            Flow::Jump {
                target: Some(nn),
                conditional: true,
            }
        }
        3 => match y {
            0 => {
                let nn = c.word();
                c.s("JP ");
                c.h16(nn);
                Flow::Jump {
                    target: Some(nn),
                    conditional: false,
                }
            }
            1 => decode_cb(c, idx),
            2 => {
                let n = c.byte();
                c.s("OUT (");
                c.h8(n);
                c.s("),A");
                Flow::Normal
            }
            3 => {
                let n = c.byte();
                c.s("IN A,(");
                c.h8(n);
                c.s(")");
                Flow::Normal
            }
            4 => {
                c.s("EX (SP),");
                c.s(idx.name());
                Flow::Normal
            }
            5 => {
                c.s("EX DE,HL");
                Flow::Normal
            }
            6 => {
                c.s("DI");
                Flow::Normal
            }
            _ => {
                c.s("EI");
                Flow::Normal
            }
        },
        4 => {
            let nn = c.word();
            c.s("CALL ");
            c.s(CC_NAMES[y as usize]);
            c.s(",");
            c.h16(nn);
            Flow::Call {
                target: Some(nn),
                conditional: true,
            }
        }
        5 => {
            if q == 0 {
                c.s("PUSH ");
                rp2_name(c, p, idx);
                Flow::Normal
            } else {
                match p {
                    0 => {
                        let nn = c.word();
                        c.s("CALL ");
                        c.h16(nn);
                        Flow::Call {
                            target: Some(nn),
                            conditional: false,
                        }
                    }
                    1 => decode_op(c, Idx::Ix),
                    2 => decode_ed(c),
                    _ => decode_op(c, Idx::Iy),
                }
            }
        }
        6 => {
            let n = c.byte();
            c.s(ALU_NAMES[y as usize]);
            c.h8(n);
            Flow::Normal
        }
        _ => {
            let target = u16::from(y) * 8;
            c.s("RST ");
            c.h8(target as u8);
            Flow::Rst(target)
        }
    }
}

fn decode_cb<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx) -> Flow {
    // `DD CB d op`: the displacement comes before the opcode byte, so both are
    // read before anything is printed.
    let displacement = if idx.indexed() { Some(c.byte()) } else { None };
    let op = c.byte();
    let x = op >> 6;
    let y = (op >> 3) & 7;
    let z = op & 7;

    if x == 1 {
        // BIT ignores the z field entirely under an index prefix.
        c.s("BIT ");
        c.dec(y);
        c.s(",");
        cb_operand(c, idx, displacement, z);
        return Flow::Normal;
    }

    match x {
        0 => {
            if y == 6 {
                c.undocumented = true; // SLL
            }
            c.s(ROT_NAMES[y as usize]);
            c.s(" ");
        }
        2 => {
            c.s("RES ");
            c.dec(y);
            c.s(",");
        }
        _ => {
            c.s("SET ");
            c.dec(y);
            c.s(",");
        }
    }
    cb_operand(c, idx, displacement, z);

    // The indexed forms also copy the result into a register unless z is the
    // `(HL)` slot. That copy is undocumented and worth showing.
    if displacement.is_some() && z != 6 {
        c.undocumented = true;
        c.s(",");
        c.s(R_NAMES[z as usize]);
    }
    Flow::Normal
}

fn cb_operand<P: Peek, S: Sink>(c: &mut Cursor<P, S>, idx: Idx, displacement: Option<u8>, z: u8) {
    match displacement {
        Some(d) => c.index(idx, d),
        None => c.s(R_NAMES[z as usize]),
    }
}

fn decode_ed<P: Peek, S: Sink>(c: &mut Cursor<P, S>) -> Flow {
    let op = c.byte();
    let x = op >> 6;
    let y = (op >> 3) & 7;
    let z = op & 7;
    let p = y >> 1;
    let q = y & 1;

    if x == 2 && z <= 3 && y >= 4 {
        c.s(BLOCK_NAMES[(y - 4) as usize][z as usize]);
        return if y >= 6 { Flow::Repeat } else { Flow::Normal };
    }

    if x != 1 {
        c.undocumented = true;
        c.s("NOP");
        return Flow::Normal;
    }

    match z {
        0 => {
            if y == 6 {
                c.undocumented = true;
                c.s("IN (C)");
            } else {
                c.s("IN ");
                c.s(R_NAMES[y as usize]);
                c.s(",(C)");
            }
            Flow::Normal
        }
        1 => {
            if y == 6 {
                c.undocumented = true;
                c.s("OUT (C),0");
            } else {
                c.s("OUT (C),");
                c.s(R_NAMES[y as usize]);
            }
            Flow::Normal
        }
        2 => {
            c.s(if q == 0 { "SBC" } else { "ADC" });
            c.s(" HL,");
            c.s(RP_NAMES[p as usize]);
            Flow::Normal
        }
        3 => {
            let nn = c.word();
            let rp = RP_NAMES[p as usize];
            if q == 0 {
                c.s("LD (");
                c.h16(nn);
                c.s("),");
                c.s(rp);
            } else {
                c.s("LD ");
                c.s(rp);
                c.s(",(");
                c.h16(nn);
                c.s(")");
            }
            Flow::Normal
        }
        4 => {
            if y != 0 {
                c.undocumented = true;
            }
            c.s("NEG");
            Flow::Normal
        }
        5 => {
            if y == 1 {
                c.s("RETI");
            } else {
                if y != 0 {
                    c.undocumented = true;
                }
                c.s("RETN");
            }
            Flow::Return { conditional: false }
        }
        6 => {
            const MODES: [u8; 8] = [0, 0, 1, 2, 0, 0, 1, 2];
            if matches!(y, 1 | 4 | 5) {
                c.undocumented = true;
            }
            c.s("IM ");
            c.dec(MODES[y as usize]);
            Flow::Normal
        }
        _ => {
            const NAMES: [&str; 8] = [
                "LD I,A", "LD R,A", "LD A,I", "LD A,R", "RRD", "RLD", "NOP", "NOP",
            ];
            if y >= 6 {
                c.undocumented = true;
            }
            c.s(NAMES[y as usize]);
            Flow::Normal
        }
    }
}

fn rp_name<P: Peek, S: Sink>(c: &mut Cursor<P, S>, p: u8, idx: Idx) {
    if p == 2 {
        c.s(idx.name());
    } else {
        c.s(RP_NAMES[p as usize]);
    }
}

fn rp2_name<P: Peek, S: Sink>(c: &mut Cursor<P, S>, p: u8, idx: Idx) {
    if p == 2 {
        c.s(idx.name());
    } else {
        c.s(RP2_NAMES[p as usize]);
    }
}
