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

use core::fmt::Write as _;

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

/// One decoded instruction.
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

    /// The `r[]` slot name, with `H`/`L` rewritten to the index halves.
    fn reg(self, slot: u8) -> String {
        match (self, slot) {
            (Idx::Hl, _) => R_NAMES[slot as usize].to_string(),
            (_, 4) => format!("{}H", self.name()),
            (_, 5) => format!("{}L", self.name()),
            (_, _) => R_NAMES[slot as usize].to_string(),
        }
    }
}

/// `(IX+$05)` / `(IX-$03)`, matching the sign the CPU applies.
fn index_operand(idx: Idx, d: u8) -> String {
    let signed = d as i8;
    if signed < 0 {
        format!("({}-${:02X})", idx.name(), signed.unsigned_abs())
    } else {
        format!("({}+${:02X})", idx.name(), signed)
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

/// Walks bytes forwards from `start`, recording what it consumed.
struct Cursor<'a, P: Peek> {
    mem: &'a P,
    start: u16,
    pos: u16,
    bytes: Vec<u8>,
    undocumented: bool,
}

impl<'a, P: Peek> Cursor<'a, P> {
    fn new(mem: &'a P, start: u16) -> Self {
        Self {
            mem,
            start,
            pos: start,
            bytes: Vec::with_capacity(4),
            undocumented: false,
        }
    }

    fn byte(&mut self) -> u8 {
        let v = self.mem.peek(self.pos);
        self.pos = self.pos.wrapping_add(1);
        self.bytes.push(v);
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

    fn finish(self, text: String, flow: Flow) -> Instruction {
        Instruction {
            addr: self.start,
            len: self.pos.wrapping_sub(self.start) as u8,
            bytes: self.bytes,
            text,
            flow,
            undocumented: self.undocumented,
        }
    }
}

/// Decode the instruction at `addr`.
///
/// Never fails: every one of the 256 opcode values decodes to something, and
/// the genuinely inert ones come back as `NOP` marked undocumented.
pub fn disassemble<P: Peek>(mem: &P, addr: u16) -> Instruction {
    let mut c = Cursor::new(mem, addr);
    let (text, flow) = decode(&mut c, Idx::Hl);
    c.finish(text, flow)
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

fn decode<P: Peek>(c: &mut Cursor<P>, idx: Idx) -> (String, Flow) {
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
            let operand = operand_r(c, idx, z);
            (format!("{}{}", ALU_NAMES[y as usize], operand), Flow::Normal)
        }
        _ => decode_x3(c, idx, y, z, p, q),
    }
}

/// Render the `r[]` operand in slot `slot`, consuming a displacement byte if
/// the slot is `(HL)` under an index prefix.
fn operand_r<P: Peek>(c: &mut Cursor<P>, idx: Idx, slot: u8) -> String {
    if slot == 6 {
        if idx.indexed() {
            let d = c.byte();
            index_operand(idx, d)
        } else {
            "(HL)".to_string()
        }
    } else {
        if idx.indexed() && (slot == 4 || slot == 5) {
            c.undocumented = true;
        }
        idx.reg(slot)
    }
}

fn decode_x0<P: Peek>(c: &mut Cursor<P>, idx: Idx, y: u8, z: u8, p: u8, q: u8) -> (String, Flow) {
    match z {
        0 => match y {
            0 => ("NOP".into(), Flow::Normal),
            1 => ("EX AF,AF'".into(), Flow::Normal),
            2 => {
                let t = c.relative();
                (
                    format!("DJNZ {}", hex16(t)),
                    Flow::Jump {
                        target: Some(t),
                        conditional: true,
                    },
                )
            }
            3 => {
                let t = c.relative();
                (
                    format!("JR {}", hex16(t)),
                    Flow::Jump {
                        target: Some(t),
                        conditional: false,
                    },
                )
            }
            _ => {
                let t = c.relative();
                (
                    format!("JR {},{}", CC_NAMES[(y - 4) as usize], hex16(t)),
                    Flow::Jump {
                        target: Some(t),
                        conditional: true,
                    },
                )
            }
        },
        1 => {
            if q == 0 {
                let nn = c.word();
                (
                    format!("LD {},{}", rp_name(p, idx), hex16(nn)),
                    Flow::Normal,
                )
            } else {
                (
                    format!("ADD {},{}", idx.name(), rp_name(p, idx)),
                    Flow::Normal,
                )
            }
        }
        2 => decode_x0_z2(c, idx, p, q),
        3 => {
            let op = if q == 0 { "INC" } else { "DEC" };
            (format!("{op} {}", rp_name(p, idx)), Flow::Normal)
        }
        4 | 5 => {
            let op = if z == 4 { "INC" } else { "DEC" };
            let operand = operand_r(c, idx, y);
            (format!("{op} {operand}"), Flow::Normal)
        }
        6 => {
            // The displacement precedes the immediate byte in the encoding.
            let operand = operand_r(c, idx, y);
            let n = c.byte();
            (format!("LD {operand},{}", hex8(n)), Flow::Normal)
        }
        _ => {
            const NAMES: [&str; 8] = ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"];
            (NAMES[y as usize].into(), Flow::Normal)
        }
    }
}

fn decode_x0_z2<P: Peek>(c: &mut Cursor<P>, idx: Idx, p: u8, q: u8) -> (String, Flow) {
    let text = match (q, p) {
        (0, 0) => "LD (BC),A".to_string(),
        (0, 1) => "LD (DE),A".to_string(),
        (0, 2) => format!("LD ({}),{}", hex16(c.word()), idx.name()),
        (0, _) => format!("LD ({}),A", hex16(c.word())),
        (_, 0) => "LD A,(BC)".to_string(),
        (_, 1) => "LD A,(DE)".to_string(),
        (_, 2) => format!("LD {},({})", idx.name(), hex16(c.word())),
        (_, _) => format!("LD A,({})", hex16(c.word())),
    };
    (text, Flow::Normal)
}

fn decode_load<P: Peek>(c: &mut Cursor<P>, idx: Idx, y: u8, z: u8) -> (String, Flow) {
    if y == 6 && z == 6 {
        return ("HALT".into(), Flow::Halt);
    }

    // With a memory operand the *other* operand keeps its unprefixed meaning,
    // so `DD 74` is `LD (IX+d),H` rather than `LD (IX+d),IXH`.
    if y == 6 || z == 6 {
        let (mem_slot, reg_slot) = if y == 6 { (y, z) } else { (z, y) };
        let mem = operand_r(c, idx, mem_slot);
        let reg = R_NAMES[reg_slot as usize].to_string();
        let text = if y == 6 {
            format!("LD {mem},{reg}")
        } else {
            format!("LD {reg},{mem}")
        };
        return (text, Flow::Normal);
    }

    if idx.indexed() && (matches!(y, 4 | 5) || matches!(z, 4 | 5)) {
        c.undocumented = true;
    }
    (
        format!("LD {},{}", idx.reg(y), idx.reg(z)),
        Flow::Normal,
    )
}

fn decode_x3<P: Peek>(c: &mut Cursor<P>, idx: Idx, y: u8, z: u8, p: u8, q: u8) -> (String, Flow) {
    match z {
        0 => (
            format!("RET {}", CC_NAMES[y as usize]),
            Flow::Return { conditional: true },
        ),
        1 => {
            if q == 0 {
                (format!("POP {}", rp2_name(p, idx)), Flow::Normal)
            } else {
                match p {
                    0 => (
                        "RET".into(),
                        Flow::Return {
                            conditional: false,
                        },
                    ),
                    1 => ("EXX".into(), Flow::Normal),
                    2 => (
                        format!("JP ({})", idx.name()),
                        Flow::Jump {
                            target: None,
                            conditional: false,
                        },
                    ),
                    _ => (format!("LD SP,{}", idx.name()), Flow::Normal),
                }
            }
        }
        2 => {
            let nn = c.word();
            (
                format!("JP {},{}", CC_NAMES[y as usize], hex16(nn)),
                Flow::Jump {
                    target: Some(nn),
                    conditional: true,
                },
            )
        }
        3 => match y {
            0 => {
                let nn = c.word();
                (
                    format!("JP {}", hex16(nn)),
                    Flow::Jump {
                        target: Some(nn),
                        conditional: false,
                    },
                )
            }
            1 => decode_cb(c, idx),
            2 => (format!("OUT ({}),A", hex8(c.byte())), Flow::Normal),
            3 => (format!("IN A,({})", hex8(c.byte())), Flow::Normal),
            4 => (format!("EX (SP),{}", idx.name()), Flow::Normal),
            5 => ("EX DE,HL".into(), Flow::Normal),
            6 => ("DI".into(), Flow::Normal),
            _ => ("EI".into(), Flow::Normal),
        },
        4 => {
            let nn = c.word();
            (
                format!("CALL {},{}", CC_NAMES[y as usize], hex16(nn)),
                Flow::Call {
                    target: Some(nn),
                    conditional: true,
                },
            )
        }
        5 => {
            if q == 0 {
                (format!("PUSH {}", rp2_name(p, idx)), Flow::Normal)
            } else {
                match p {
                    0 => {
                        let nn = c.word();
                        (
                            format!("CALL {}", hex16(nn)),
                            Flow::Call {
                                target: Some(nn),
                                conditional: false,
                            },
                        )
                    }
                    1 => decode(c, Idx::Ix),
                    2 => decode_ed(c),
                    _ => decode(c, Idx::Iy),
                }
            }
        }
        6 => {
            let n = c.byte();
            (
                format!("{}{}", ALU_NAMES[y as usize], hex8(n)),
                Flow::Normal,
            )
        }
        _ => {
            let target = u16::from(y) * 8;
            (format!("RST {}", hex8(target as u8)), Flow::Rst(target))
        }
    }
}

fn decode_cb<P: Peek>(c: &mut Cursor<P>, idx: Idx) -> (String, Flow) {
    // `DD CB d op`: the displacement comes before the opcode byte.
    let displacement = if idx.indexed() { Some(c.byte()) } else { None };
    let op = c.byte();
    let x = op >> 6;
    let y = (op >> 3) & 7;
    let z = op & 7;

    let operand = match displacement {
        Some(d) => index_operand(idx, d),
        None => R_NAMES[z as usize].to_string(),
    };

    if x == 1 {
        // BIT ignores the z field entirely under an index prefix.
        return (format!("BIT {y},{operand}"), Flow::Normal);
    }

    let mnemonic = match x {
        0 => {
            if y == 6 {
                c.undocumented = true; // SLL
            }
            ROT_NAMES[y as usize].to_string()
        }
        2 => format!("RES {y},"),
        _ => format!("SET {y},"),
    };
    let mnemonic = if x == 0 {
        format!("{mnemonic} ")
    } else {
        mnemonic
    };

    // The indexed forms also copy the result into a register unless z is the
    // `(HL)` slot. That copy is undocumented and worth showing.
    if displacement.is_some() && z != 6 {
        c.undocumented = true;
        return (
            format!("{mnemonic}{operand},{}", R_NAMES[z as usize]),
            Flow::Normal,
        );
    }
    (format!("{mnemonic}{operand}"), Flow::Normal)
}

fn decode_ed<P: Peek>(c: &mut Cursor<P>) -> (String, Flow) {
    let op = c.byte();
    let x = op >> 6;
    let y = (op >> 3) & 7;
    let z = op & 7;
    let p = y >> 1;
    let q = y & 1;

    if x == 2 && z <= 3 && y >= 4 {
        let name = BLOCK_NAMES[(y - 4) as usize][z as usize];
        let flow = if y >= 6 { Flow::Repeat } else { Flow::Normal };
        return (name.into(), flow);
    }

    if x != 1 {
        c.undocumented = true;
        return ("NOP".into(), Flow::Normal);
    }

    match z {
        0 => {
            if y == 6 {
                c.undocumented = true;
                ("IN (C)".into(), Flow::Normal)
            } else {
                (format!("IN {},(C)", R_NAMES[y as usize]), Flow::Normal)
            }
        }
        1 => {
            if y == 6 {
                c.undocumented = true;
                ("OUT (C),0".into(), Flow::Normal)
            } else {
                (format!("OUT (C),{}", R_NAMES[y as usize]), Flow::Normal)
            }
        }
        2 => {
            let op = if q == 0 { "SBC" } else { "ADC" };
            (
                format!("{op} HL,{}", RP_NAMES[p as usize]),
                Flow::Normal,
            )
        }
        3 => {
            let nn = c.word();
            let rp = RP_NAMES[p as usize];
            let text = if q == 0 {
                format!("LD ({}),{rp}", hex16(nn))
            } else {
                format!("LD {rp},({})", hex16(nn))
            };
            (text, Flow::Normal)
        }
        4 => {
            if y != 0 {
                c.undocumented = true;
            }
            ("NEG".into(), Flow::Normal)
        }
        5 => {
            if y == 1 {
                (
                    "RETI".into(),
                    Flow::Return {
                        conditional: false,
                    },
                )
            } else {
                if y != 0 {
                    c.undocumented = true;
                }
                (
                    "RETN".into(),
                    Flow::Return {
                        conditional: false,
                    },
                )
            }
        }
        6 => {
            const MODES: [u8; 8] = [0, 0, 1, 2, 0, 0, 1, 2];
            if matches!(y, 1 | 4 | 5) {
                c.undocumented = true;
            }
            (format!("IM {}", MODES[y as usize]), Flow::Normal)
        }
        _ => {
            const NAMES: [&str; 8] = [
                "LD I,A", "LD R,A", "LD A,I", "LD A,R", "RRD", "RLD", "NOP", "NOP",
            ];
            if y >= 6 {
                c.undocumented = true;
            }
            (NAMES[y as usize].into(), Flow::Normal)
        }
    }
}

fn rp_name(p: u8, idx: Idx) -> String {
    if p == 2 {
        idx.name().to_string()
    } else {
        RP_NAMES[p as usize].to_string()
    }
}

fn rp2_name(p: u8, idx: Idx) -> String {
    if p == 2 {
        idx.name().to_string()
    } else {
        RP2_NAMES[p as usize].to_string()
    }
}
