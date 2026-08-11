//! Instruction encoding: the disassembler read backwards.
//!
//! Encoding happens in two steps, and the split is not cosmetic. [`plan`] works
//! out the *shape* of an instruction — which prefix, which opcode, where the
//! operand bytes go — from the parse tree alone, without evaluating anything.
//! [`emit`] then fills the operand bytes in once the values are known.
//!
//! That is possible because on the Z80 an instruction's length never depends on
//! a value: `JR` and `JP` are different mnemonics the programmer chooses, and
//! `(IX+3)` and `(IX+100)` are both three bytes (ADR-0014). So the first pass
//! can advance the location counter past an instruction whose operand refers to
//! a label it has not reached yet, and the addresses it produces are real
//! rather than provisional.
//!
//! The opcode tables follow the `z80` crate's disassembler arm for arm — the same octal
//! decomposition of the opcode byte, read in the opposite direction, so that
//! `0x40 | dst << 3 | src` appears here as arithmetic exactly where the
//! disassembler pulls the same fields out. Keeping the two shaped alike is what
//! stops them drifting; the round-trip test is what proves they have not.

use crate::ast::{Expr, ExprKind, Op};
use crate::diag::Diagnostic;
use crate::eval::{EvalError, Site, eval, fit_byte, fit_relative, fit_signed_byte, fit_word};
use crate::operand::{
    self, Index, R, Reg16, as_address, as_condition, as_r, as_reg16, as_register_indirect,
    is_af_shadow,
};
use crate::symbols::Symbols;

/// One byte of an instruction: either known from the syntax, or a value that
/// has still to be worked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Byte {
    Fixed(u8),
    /// An immediate byte, accepting the signed and unsigned readings alike.
    Imm8(Expr),
    /// An index displacement, which is signed only.
    Displacement(Expr),
    /// A 16-bit immediate, emitted low byte first.
    Imm16(Expr),
    /// A `JR`/`DJNZ` target, stored as the address to jump to and emitted as
    /// the distance from the instruction after this one.
    Relative(Expr),
    /// An opcode byte with a small value shifted into it: the bit number in
    /// `BIT n,r`, which is part of the opcode rather than an operand byte.
    Field {
        base: u8,
        shift: u8,
        limit: i64,
        what: &'static str,
        value: Expr,
    },
    /// `RST n`, whose target is encoded in the opcode and must be a multiple
    /// of eight below 64.
    Rst(Expr),
    /// `IM n`, which is three opcodes rather than a field.
    Im(Expr),
}

/// The shape of an instruction, known before any value is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub bytes: Vec<Byte>,
}

impl Plan {
    /// How many bytes this instruction occupies. Available without evaluating
    /// anything, which is what lets the first pass assign real addresses.
    ///
    /// Not the number of entries: a 16-bit immediate is one entry and two
    /// bytes.
    pub fn len(&self) -> usize {
        self.bytes
            .iter()
            .map(|byte| match byte {
                Byte::Imm16(_) => 2,
                _ => 1,
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// Nothing wrong with the values; the instruction itself does not exist.
    Form(Diagnostic),
    /// The instruction exists but an operand could not be worked out.
    Value(EvalError),
}

impl EncodeError {
    pub fn diagnostic(&self) -> Diagnostic {
        match self {
            EncodeError::Form(d) => d.clone(),
            EncodeError::Value(e) => e.diagnostic(),
        }
    }

    /// True while this only means "come back on the next pass".
    pub fn is_forward_reference(&self) -> bool {
        match self {
            EncodeError::Value(e) => e.is_forward_reference(),
            EncodeError::Form(_) => false,
        }
    }
}

impl From<Diagnostic> for EncodeError {
    fn from(d: Diagnostic) -> Self {
        EncodeError::Form(d)
    }
}

impl From<EvalError> for EncodeError {
    fn from(e: EvalError) -> Self {
        EncodeError::Value(e)
    }
}

/// Plan and emit in one step.
pub fn encode(op: &Op, site: Site, symbols: &mut Symbols) -> Result<Vec<u8>, EncodeError> {
    emit(&plan(op)?, site, symbols)
}

/// True if `name` is an instruction this module can encode. The parser cannot
/// tell an instruction from a macro call, so something has to.
pub fn is_instruction(name: &str) -> bool {
    MNEMONICS.contains(&name.to_ascii_uppercase().as_str())
}

/// Fill in an instruction's operand bytes.
pub fn emit(plan: &Plan, site: Site, symbols: &mut Symbols) -> Result<Vec<u8>, EncodeError> {
    let length = plan.len() as i64;
    let mut out = Vec::with_capacity(plan.len());
    for byte in &plan.bytes {
        match byte {
            Byte::Fixed(b) => out.push(*b),
            Byte::Imm8(e) => out.push(fit_byte(eval(e, site, symbols)?, e.span)?),
            Byte::Displacement(e) => {
                out.push(fit_signed_byte(eval(e, site, symbols)?, e.span)? as u8)
            }
            Byte::Imm16(e) => {
                let value = fit_word(eval(e, site, symbols)?, e.span)?;
                out.extend_from_slice(&value.to_le_bytes());
            }
            Byte::Relative(e) => {
                // The CPU adds the displacement to the address of the *next*
                // instruction, so that is what it is measured from.
                let target = eval(e, site, symbols)?;
                out.push(fit_relative(site.here + length, target, e.span)? as u8);
            }
            Byte::Field {
                base,
                shift,
                limit,
                what,
                value,
            } => {
                let v = eval(value, site, symbols)?;
                if v < 0 || v > *limit {
                    return Err(EvalError::OutOfRange {
                        value: v,
                        min: 0,
                        max: *limit,
                        what,
                        span: value.span,
                    }
                    .into());
                }
                out.push(base | ((v as u8) << shift));
            }
            Byte::Rst(e) => {
                let v = eval(e, site, symbols)?;
                if !(0..=0x38).contains(&v) || v % 8 != 0 {
                    return Err(EncodeError::Form(
                        Diagnostic::error(e.span, format!("`RST {v}` is not a restart address"))
                            .with_caret_label("expected a multiple of 8 from 0 to $38"),
                    ));
                }
                out.push(0xC7 | v as u8);
            }
            Byte::Im(e) => {
                let v = eval(e, site, symbols)?;
                let opcode = match v {
                    0 => 0x46,
                    1 => 0x56,
                    2 => 0x5E,
                    _ => {
                        return Err(EncodeError::Form(
                            Diagnostic::error(e.span, format!("there is no interrupt mode {v}"))
                                .with_caret_label("expected 0, 1 or 2"),
                        ));
                    }
                };
                out.push(opcode);
            }
        }
    }
    Ok(out)
}

/// Work out an instruction's shape from its syntax.
pub fn plan(op: &Op) -> Result<Plan, Diagnostic> {
    let name = op.name.to_ascii_uppercase();
    let args = &op.args[..];

    let planned = match name.as_str() {
        "NOP" => fixed(args, &[0x00]),
        "HALT" => fixed(args, &[0x76]),
        "DI" => fixed(args, &[0xF3]),
        "EI" => fixed(args, &[0xFB]),
        "EXX" => fixed(args, &[0xD9]),
        "RLCA" => fixed(args, &[0x07]),
        "RRCA" => fixed(args, &[0x0F]),
        "RLA" => fixed(args, &[0x17]),
        "RRA" => fixed(args, &[0x1F]),
        "DAA" => fixed(args, &[0x27]),
        "CPL" => fixed(args, &[0x2F]),
        "SCF" => fixed(args, &[0x37]),
        "CCF" => fixed(args, &[0x3F]),
        "NEG" => fixed(args, &[0xED, 0x44]),
        "RETN" => fixed(args, &[0xED, 0x45]),
        "RETI" => fixed(args, &[0xED, 0x4D]),
        "RRD" => fixed(args, &[0xED, 0x67]),
        "RLD" => fixed(args, &[0xED, 0x6F]),

        "LDI" => fixed(args, &[0xED, 0xA0]),
        "CPI" => fixed(args, &[0xED, 0xA1]),
        "INI" => fixed(args, &[0xED, 0xA2]),
        "OUTI" => fixed(args, &[0xED, 0xA3]),
        "LDD" => fixed(args, &[0xED, 0xA8]),
        "CPD" => fixed(args, &[0xED, 0xA9]),
        "IND" => fixed(args, &[0xED, 0xAA]),
        "OUTD" => fixed(args, &[0xED, 0xAB]),
        "LDIR" => fixed(args, &[0xED, 0xB0]),
        "CPIR" => fixed(args, &[0xED, 0xB1]),
        "INIR" => fixed(args, &[0xED, 0xB2]),
        "OTIR" => fixed(args, &[0xED, 0xB3]),
        "LDDR" => fixed(args, &[0xED, 0xB8]),
        "CPDR" => fixed(args, &[0xED, 0xB9]),
        "INDR" => fixed(args, &[0xED, 0xBA]),
        "OTDR" => fixed(args, &[0xED, 0xBB]),

        "LD" => ld(args),
        "ADD" => add(args),
        "ADC" => adc_sbc(args, 1, 0x4A),
        "SBC" => adc_sbc(args, 3, 0x42),
        "SUB" => alu(args, 2),
        "AND" => alu(args, 4),
        "XOR" => alu(args, 5),
        "OR" => alu(args, 6),
        "CP" => alu(args, 7),
        "INC" => inc_dec(args, 0x04, 0x03),
        "DEC" => inc_dec(args, 0x05, 0x0B),

        "RLC" => rotate(args, 0),
        "RRC" => rotate(args, 1),
        "RL" => rotate(args, 2),
        "RR" => rotate(args, 3),
        "SLA" => rotate(args, 4),
        "SRA" => rotate(args, 5),
        "SLL" | "SLI" => rotate(args, 6),
        "SRL" => rotate(args, 7),
        "BIT" => bit(args, 1),
        "RES" => bit(args, 2),
        "SET" => bit(args, 3),

        "PUSH" => stack(args, 0xC5),
        "POP" => stack(args, 0xC1),
        "JP" => jp(args),
        "JR" => jr(args),
        "DJNZ" => djnz(args),
        "CALL" => call(args),
        "RET" => ret(args),
        "RST" => rst(args),
        "EX" => ex(args),
        "IN" => in_(args),
        "OUT" => out(args),
        "IM" => im(args),

        _ => return Err(unknown_instruction(op)),
    };

    planned.ok_or_else(|| no_such_form(op))
}

fn unknown_instruction(op: &Op) -> Diagnostic {
    Diagnostic::error(op.name_span, format!("unknown instruction `{}`", op.name))
}

fn no_such_form(op: &Op) -> Diagnostic {
    let operands = op
        .args
        .iter()
        .map(Expr::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let message = match operands.is_empty() {
        true => format!("`{}` needs operands", op.name),
        false => format!("no form of `{}` takes `{operands}`", op.name),
    };
    Diagnostic::error(op.span, message)
}

// -- builders ---------------------------------------------------------------

fn plan_of(bytes: Vec<Byte>) -> Option<Plan> {
    Some(Plan { bytes })
}

/// An instruction with no operands and a fixed encoding.
fn fixed(args: &[Expr], bytes: &[u8]) -> Option<Plan> {
    if !args.is_empty() {
        return None;
    }
    plan_of(bytes.iter().copied().map(Byte::Fixed).collect())
}

/// Start a byte list with the prefix an index register needs, if any.
fn with_prefix(index: Index) -> Vec<Byte> {
    match index.prefix() {
        Some(p) => vec![Byte::Fixed(p)],
        None => Vec::new(),
    }
}

/// A bare expression operand: not parenthesised, and not a register name.
///
/// Rejecting register names is what keeps `LD A,B` from being read as `LD A,n`
/// with a symbol called `B` if the register form is ever missed.
fn as_value(expr: &Expr) -> Option<&Expr> {
    if expr.as_parenthesised().is_some() {
        return None;
    }
    if let ExprKind::Ident(name) = &expr.kind {
        if operand::reg8(name).is_some() || operand::reg16(name).is_some() {
            return None;
        }
    }
    Some(expr)
}

/// A jump or call target, which may be written with or without parentheses.
fn as_target(expr: &Expr) -> Option<&Expr> {
    as_address(expr).or_else(|| as_value(expr))
}

/// True if this operand is the register `A`.
fn is_a(expr: &Expr) -> bool {
    matches!(
        as_r(expr),
        Some(R::Reg {
            slot: 7,
            index: Index::Hl
        })
    )
}

fn named(expr: &Expr, name: &str) -> bool {
    expr.as_ident()
        .is_some_and(|n| n.eq_ignore_ascii_case(name))
}

/// A parenthesised register name, matched case-insensitively.
fn indirect(expr: &Expr, name: &str) -> bool {
    as_register_indirect(expr).is_some_and(|n| n.eq_ignore_ascii_case(name))
}

// -- LD ---------------------------------------------------------------------

fn ld(args: &[Expr]) -> Option<Plan> {
    let [dst, src] = args else { return None };

    // LD r,r' — including (HL) and (IX+d) on either side.
    if let (Some(d), Some(s)) = (as_r(dst), as_r(src)) {
        return ld_reg_reg(&d, &s);
    }

    // The special forms come before the immediate one, because `I` and `R` are
    // register names that `LD A,n` would otherwise take for symbols.
    if let Some(plan) = ld_special(dst, src) {
        return Some(plan);
    }
    if let Some(plan) = ld_sixteen(dst, src) {
        return Some(plan);
    }

    // LD r,n — the destination may be memory, giving LD (HL),n and
    // LD (IX+d),n, where the displacement precedes the immediate.
    if let (Some(d), Some(value)) = (as_r(dst), as_value(src)) {
        let mut bytes = with_prefix(d.index());
        bytes.push(Byte::Fixed(0x06 | (d.slot() << 3)));
        if let Some(disp) = d.displacement() {
            bytes.push(Byte::Displacement(disp.clone()));
        }
        bytes.push(Byte::Imm8(value.clone()));
        return plan_of(bytes);
    }
    None
}

fn ld_reg_reg(d: &R, s: &R) -> Option<Plan> {
    // `LD (HL),(HL)` is the encoding of HALT, so it cannot also mean this.
    if d.is_memory() && s.is_memory() {
        return None;
    }

    let index = if d.is_memory() || s.is_memory() {
        // With a memory operand the other one keeps its unprefixed meaning:
        // `DD 74` is `LD (IX+d),H`, never `LD (IX+d),IXH`.
        let (memory, other) = if d.is_memory() { (d, s) } else { (s, d) };
        if other.index() != Index::Hl {
            return None;
        }
        memory.index()
    } else {
        d.index().combine(s.index())?
    };

    let mut bytes = with_prefix(index);
    bytes.push(Byte::Fixed(0x40 | (d.slot() << 3) | s.slot()));
    if let Some(disp) = d.displacement().or_else(|| s.displacement()) {
        bytes.push(Byte::Displacement(disp.clone()));
    }
    plan_of(bytes)
}

/// The `LD` forms involving `A`, `I`, `R` and the register-indirect pairs.
fn ld_special(dst: &Expr, src: &Expr) -> Option<Plan> {
    if is_a(dst) {
        if indirect(src, "bc") {
            return plan_of(vec![Byte::Fixed(0x0A)]);
        }
        if indirect(src, "de") {
            return plan_of(vec![Byte::Fixed(0x1A)]);
        }
        if named(src, "i") {
            return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x57)]);
        }
        if named(src, "r") {
            return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x5F)]);
        }
        if let Some(address) = as_address(src) {
            return plan_of(vec![Byte::Fixed(0x3A), Byte::Imm16(address.clone())]);
        }
    }
    if is_a(src) {
        if indirect(dst, "bc") {
            return plan_of(vec![Byte::Fixed(0x02)]);
        }
        if indirect(dst, "de") {
            return plan_of(vec![Byte::Fixed(0x12)]);
        }
        if named(dst, "i") {
            return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x47)]);
        }
        if named(dst, "r") {
            return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x4F)]);
        }
        if let Some(address) = as_address(dst) {
            return plan_of(vec![Byte::Fixed(0x32), Byte::Imm16(address.clone())]);
        }
    }
    None
}

/// `LD` between a register pair and an immediate, an address, or `HL`.
fn ld_sixteen(dst: &Expr, src: &Expr) -> Option<Plan> {
    if let Some(pair) = as_reg16(dst) {
        // LD SP,HL / LD SP,IX / LD SP,IY
        if pair == Reg16::Sp {
            if let Some(from) = as_reg16(src) {
                if matches!(from, Reg16::Hl | Reg16::Ix | Reg16::Iy) {
                    let mut bytes = with_prefix(from.index());
                    bytes.push(Byte::Fixed(0xF9));
                    return plan_of(bytes);
                }
            }
        }
        let (p, index) = pair.rp()?;
        if let Some(address) = as_address(src) {
            // HL has a one-byte form of its own; the others go through ED.
            if p == 2 {
                let mut bytes = with_prefix(index);
                bytes.push(Byte::Fixed(0x2A));
                bytes.push(Byte::Imm16(address.clone()));
                return plan_of(bytes);
            }
            return plan_of(vec![
                Byte::Fixed(0xED),
                Byte::Fixed(0x4B | (p << 4)),
                Byte::Imm16(address.clone()),
            ]);
        }
        if let Some(value) = as_value(src) {
            let mut bytes = with_prefix(index);
            bytes.push(Byte::Fixed(0x01 | (p << 4)));
            bytes.push(Byte::Imm16(value.clone()));
            return plan_of(bytes);
        }
        return None;
    }

    // LD (nn),rp
    let address = as_address(dst)?;
    let (p, index) = as_reg16(src)?.rp()?;
    if p == 2 {
        let mut bytes = with_prefix(index);
        bytes.push(Byte::Fixed(0x22));
        bytes.push(Byte::Imm16(address.clone()));
        return plan_of(bytes);
    }
    plan_of(vec![
        Byte::Fixed(0xED),
        Byte::Fixed(0x43 | (p << 4)),
        Byte::Imm16(address.clone()),
    ])
}

// -- arithmetic -------------------------------------------------------------

/// The eight `alu[]` operations against `A`, in their one- and two-operand
/// spellings: `SUB B` and `SUB A,B` are the same instruction.
fn alu(args: &[Expr], operation: u8) -> Option<Plan> {
    let source = match args {
        [source] => source,
        [first, source] if is_a(first) => source,
        _ => return None,
    };
    if let Some(r) = as_r(source) {
        let mut bytes = with_prefix(r.index());
        bytes.push(Byte::Fixed(0x80 | (operation << 3) | r.slot()));
        if let Some(disp) = r.displacement() {
            bytes.push(Byte::Displacement(disp.clone()));
        }
        return plan_of(bytes);
    }
    let value = as_value(source)?;
    plan_of(vec![
        Byte::Fixed(0xC6 | (operation << 3)),
        Byte::Imm8(value.clone()),
    ])
}

fn add(args: &[Expr]) -> Option<Plan> {
    if let [dst, src] = args {
        if let Some(pair) = as_reg16(dst) {
            return add_sixteen(pair, src);
        }
    }
    alu(args, 0)
}

/// `ADD HL,rp`, and the prefixed `ADD IX,rp`.
fn add_sixteen(dst: Reg16, src: &Expr) -> Option<Plan> {
    if !matches!(dst, Reg16::Hl | Reg16::Ix | Reg16::Iy) {
        return None;
    }
    let index = dst.index();
    let (p, source_index) = as_reg16(src)?.rp()?;
    // `ADD IX,HL` does not exist: under a prefix, slot 2 is the index register
    // itself, so the source must be the same one or an unprefixed pair.
    if p == 2 && source_index != index {
        return None;
    }
    let mut bytes = with_prefix(index);
    bytes.push(Byte::Fixed(0x09 | (p << 4)));
    plan_of(bytes)
}

/// `ADC` and `SBC`, which have a 16-bit form against `HL` only.
fn adc_sbc(args: &[Expr], operation: u8, ed_base: u8) -> Option<Plan> {
    if let [dst, src] = args {
        if let Some(Reg16::Hl) = as_reg16(dst) {
            let (p, index) = as_reg16(src)?.rp()?;
            if index != Index::Hl {
                return None;
            }
            return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(ed_base | (p << 4))]);
        }
    }
    alu(args, operation)
}

fn inc_dec(args: &[Expr], reg_base: u8, pair_base: u8) -> Option<Plan> {
    let [target] = args else { return None };
    if let Some(r) = as_r(target) {
        let mut bytes = with_prefix(r.index());
        bytes.push(Byte::Fixed(reg_base | (r.slot() << 3)));
        if let Some(disp) = r.displacement() {
            bytes.push(Byte::Displacement(disp.clone()));
        }
        return plan_of(bytes);
    }
    let (p, index) = as_reg16(target)?.rp()?;
    let mut bytes = with_prefix(index);
    bytes.push(Byte::Fixed(pair_base | (p << 4)));
    plan_of(bytes)
}

// -- CB page ----------------------------------------------------------------

/// A shift or rotate, including the undocumented indexed form that also copies
/// the result into a register: `RLC (IX+d),B`.
fn rotate(args: &[Expr], operation: u8) -> Option<Plan> {
    let (target, copy_to) = match args {
        [target] => (target, None),
        [target, copy] => (target, Some(copy)),
        _ => return None,
    };
    let r = as_r(target)?;
    let opcode = |slot: u8| Byte::Fixed((operation << 3) | slot);
    cb_form(&r, copy_to, opcode)
}

/// `BIT`, `RES` and `SET`, whose bit number is part of the opcode byte.
fn bit(args: &[Expr], group: u8) -> Option<Plan> {
    let (number, target, copy_to) = match args {
        [number, target] => (number, target, None),
        [number, target, copy] => (number, target, Some(copy)),
        _ => return None,
    };
    let number = as_value(number)?;
    let r = as_r(target)?;
    // BIT has no register-copy form: it writes no result to copy.
    if group == 1 && copy_to.is_some() {
        return None;
    }
    let opcode = |slot: u8| Byte::Field {
        base: (group << 6) | slot,
        shift: 3,
        limit: 7,
        what: "a bit number",
        value: number.clone(),
    };
    cb_form(&r, copy_to, opcode)
}

/// Assemble a `CB`-page instruction, which puts its displacement before the
/// opcode rather than after it.
fn cb_form(r: &R, copy_to: Option<&Expr>, opcode: impl Fn(u8) -> Byte) -> Option<Plan> {
    let slot = match copy_to {
        // The copy target is only available on the indexed forms, and it
        // occupies the operand slot that would otherwise select the register.
        Some(copy) => {
            if !matches!(r, R::Indexed { .. }) {
                return None;
            }
            match as_r(copy)? {
                R::Reg {
                    slot,
                    index: Index::Hl,
                } => slot,
                _ => return None,
            }
        }
        None => r.slot(),
    };

    let mut bytes = with_prefix(r.index());
    bytes.push(Byte::Fixed(0xCB));
    if let Some(disp) = r.displacement() {
        bytes.push(Byte::Displacement(disp.clone()));
    }
    bytes.push(opcode(slot));
    plan_of(bytes)
}

// -- control flow -----------------------------------------------------------

fn stack(args: &[Expr], base: u8) -> Option<Plan> {
    let [target] = args else { return None };
    let (p, index) = as_reg16(target)?.rp2()?;
    let mut bytes = with_prefix(index);
    bytes.push(Byte::Fixed(base | (p << 4)));
    plan_of(bytes)
}

fn jp(args: &[Expr]) -> Option<Plan> {
    match args {
        [target] => {
            // `JP (HL)` is an indirect jump through the register, not through
            // memory, which is why it has no address operand.
            for (name, index) in [("hl", Index::Hl), ("ix", Index::Ix), ("iy", Index::Iy)] {
                if indirect(target, name) {
                    let mut bytes = with_prefix(index);
                    bytes.push(Byte::Fixed(0xE9));
                    return plan_of(bytes);
                }
            }
            let target = as_target(target)?;
            plan_of(vec![Byte::Fixed(0xC3), Byte::Imm16(target.clone())])
        }
        [condition, target] => {
            let cc = as_condition(condition)?;
            let target = as_target(target)?;
            plan_of(vec![
                Byte::Fixed(0xC2 | (cc << 3)),
                Byte::Imm16(target.clone()),
            ])
        }
        _ => None,
    }
}

fn jr(args: &[Expr]) -> Option<Plan> {
    match args {
        [target] => plan_of(vec![
            Byte::Fixed(0x18),
            Byte::Relative(as_target(target)?.clone()),
        ]),
        [condition, target] => {
            // Only the first four conditions have a relative form; there is no
            // `JR PO,e`.
            let cc = as_condition(condition).filter(|cc| *cc < 4)?;
            plan_of(vec![
                Byte::Fixed(0x20 | (cc << 3)),
                Byte::Relative(as_target(target)?.clone()),
            ])
        }
        _ => None,
    }
}

fn djnz(args: &[Expr]) -> Option<Plan> {
    let [target] = args else { return None };
    plan_of(vec![
        Byte::Fixed(0x10),
        Byte::Relative(as_target(target)?.clone()),
    ])
}

fn call(args: &[Expr]) -> Option<Plan> {
    match args {
        [target] => plan_of(vec![
            Byte::Fixed(0xCD),
            Byte::Imm16(as_target(target)?.clone()),
        ]),
        [condition, target] => plan_of(vec![
            Byte::Fixed(0xC4 | (as_condition(condition)? << 3)),
            Byte::Imm16(as_target(target)?.clone()),
        ]),
        _ => None,
    }
}

fn ret(args: &[Expr]) -> Option<Plan> {
    match args {
        [] => plan_of(vec![Byte::Fixed(0xC9)]),
        [condition] => plan_of(vec![Byte::Fixed(0xC0 | (as_condition(condition)? << 3))]),
        _ => None,
    }
}

fn rst(args: &[Expr]) -> Option<Plan> {
    let [target] = args else { return None };
    plan_of(vec![Byte::Rst(as_value(target)?.clone())])
}

fn ex(args: &[Expr]) -> Option<Plan> {
    let [first, second] = args else { return None };
    if named(first, "de") && named(second, "hl") {
        return plan_of(vec![Byte::Fixed(0xEB)]);
    }
    if named(first, "af") && is_af_shadow(second) {
        return plan_of(vec![Byte::Fixed(0x08)]);
    }
    if indirect(first, "sp") {
        let index = match as_reg16(second)? {
            Reg16::Hl => Index::Hl,
            Reg16::Ix => Index::Ix,
            Reg16::Iy => Index::Iy,
            _ => return None,
        };
        let mut bytes = with_prefix(index);
        bytes.push(Byte::Fixed(0xE3));
        return plan_of(bytes);
    }
    None
}

fn in_(args: &[Expr]) -> Option<Plan> {
    match args {
        // `IN (C)` reads a port and discards the value, setting only flags.
        [port] if indirect(port, "c") => plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x70)]),
        [destination, port] => {
            if indirect(port, "c") {
                let R::Reg {
                    slot,
                    index: Index::Hl,
                } = as_r(destination)?
                else {
                    return None;
                };
                return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x40 | (slot << 3))]);
            }
            // `IN A,(n)` is the only form with an immediate port, and it does
            // not go through ED.
            if !is_a(destination) {
                return None;
            }
            plan_of(vec![
                Byte::Fixed(0xDB),
                Byte::Imm8(as_address(port)?.clone()),
            ])
        }
        _ => None,
    }
}

fn out(args: &[Expr]) -> Option<Plan> {
    let [port, source] = args else { return None };
    if indirect(port, "c") {
        // `OUT (C),0` writes a constant the CPU supplies, so the operand is
        // the literal zero rather than an expression.
        if is_literal_zero(source) {
            return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x71)]);
        }
        let R::Reg {
            slot,
            index: Index::Hl,
        } = as_r(source)?
        else {
            return None;
        };
        return plan_of(vec![Byte::Fixed(0xED), Byte::Fixed(0x41 | (slot << 3))]);
    }
    if !is_a(source) {
        return None;
    }
    plan_of(vec![
        Byte::Fixed(0xD3),
        Byte::Imm8(as_address(port)?.clone()),
    ])
}

fn is_literal_zero(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::Number { value: 0, .. })
}

fn im(args: &[Expr]) -> Option<Plan> {
    let [mode] = args else { return None };
    plan_of(vec![Byte::Fixed(0xED), Byte::Im(as_value(mode)?.clone())])
}

/// Every mnemonic [`plan`] recognises. Used to tell an instruction from a macro
/// call, which the parser cannot do.
const MNEMONICS: &[&str] = &[
    "ADC", "ADD", "AND", "BIT", "CALL", "CCF", "CP", "CPD", "CPDR", "CPI", "CPIR", "CPL", "DAA",
    "DEC", "DI", "DJNZ", "EI", "EX", "EXX", "HALT", "IM", "IN", "INC", "IND", "INDR", "INI",
    "INIR", "JP", "JR", "LD", "LDD", "LDDR", "LDI", "LDIR", "NEG", "NOP", "OR", "OTDR", "OTIR",
    "OUT", "OUTD", "OUTI", "POP", "PUSH", "RES", "RET", "RETI", "RETN", "RL", "RLA", "RLC", "RLCA",
    "RLD", "RR", "RRA", "RRC", "RRCA", "RRD", "RST", "SBC", "SCF", "SET", "SLA", "SLI", "SLL",
    "SRA", "SRL", "SUB", "XOR",
];
