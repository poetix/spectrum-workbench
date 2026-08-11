//! The set of names that begin a statement.
//!
//! This exists for one narrow reason. sjasmplus lets a label be written without
//! a colon provided it starts in column 1, so a word in column 1 is a label
//! unless it is a mnemonic or a directive — and there is no way to tell those
//! apart from the characters alone. `push hl` in column 1 has to be an
//! instruction, `push:` has to be a label.
//!
//! It is a set of names, not a table of meanings: nothing here says what `LD`
//! encodes to or what `ORG` does. Instruction selection (ticket 0003) and the
//! directives (0004) own that, and adding a directive there means adding its
//! name here so that a source file using it in column 1 still parses.
//!
//! Indented lines never consult this, so a macro invoked in column 1 is the one
//! construct that needs its colon — the same restriction sjasmplus has.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Every mnemonic the disassembler can emit, including the undocumented ones.
const MNEMONICS: &[&str] = &[
    "ADC", "ADD", "AND", "BIT", "CALL", "CCF", "CP", "CPD", "CPDR", "CPI", "CPIR", "CPL", "DAA",
    "DEC", "DI", "DJNZ", "EI", "EX", "EXX", "HALT", "IM", "IN", "INC", "IND", "INDR", "INI",
    "INIR", "JP", "JR", "LD", "LDD", "LDDR", "LDI", "LDIR", "NEG", "NOP", "OR", "OTDR", "OTIR",
    "OUT", "OUTD", "OUTI", "POP", "PUSH", "RES", "RET", "RETI", "RETN", "RL", "RLA", "RLC", "RLCA",
    "RLD", "RR", "RRA", "RRC", "RRCA", "RRD", "RST", "SBC", "SCF", "SET", "SLA", "SLI", "SLL",
    "SRA", "SRL", "SUB", "XOR",
];

/// Directive names sjasmplus accepts. Not yet implemented — 0004 and 0005 do
/// that — but recognised here so that a file using them parses as intended.
const DIRECTIVES: &[&str] = &[
    "ALIGN",
    "ASSERT",
    "BANK",
    "BLOCK",
    "BYTE",
    "CSPECTMAP",
    "DB",
    "DC",
    "DD",
    "DEFARRAY",
    "DEFB",
    "DEFD",
    "DEFG",
    "DEFINE",
    "DEFL",
    "DEFM",
    "DEFS",
    "DEFW",
    "DEFZ",
    "DEPHASE",
    "DEVICE",
    "DG",
    "DH",
    "DISP",
    "DISPLAY",
    "DM",
    "DS",
    "DUP",
    "DW",
    "DWORD",
    "DZ",
    "EDUP",
    "ELSE",
    "ELSEIF",
    "EMPTYTAP",
    "EMPTYTRD",
    "END",
    "ENDIF",
    "ENDLUA",
    "ENDMAP",
    "ENDMOD",
    "ENDMODULE",
    "ENDM",
    "ENDR",
    "ENDS",
    "ENDT",
    "ENT",
    "EQU",
    "EXPORT",
    "FIELD",
    "FPOS",
    "HEX",
    "IF",
    "IFDEF",
    "IFN",
    "IFNDEF",
    "IFNUSED",
    "IFUSED",
    "INCBIN",
    "INCHOB",
    "INCLUDE",
    "INCLUDELUA",
    "INCTRD",
    "INSERT",
    "LABELSLIST",
    "LUA",
    "MACRO",
    "MAP",
    "MMU",
    "MODULE",
    "ORG",
    "OUTEND",
    "OUTPUT",
    "PAGE",
    "PHASE",
    "REPT",
    "SAVEBIN",
    "SAVEDEV",
    "SAVEHOB",
    "SAVENEX",
    "SAVESNA",
    "SAVETAP",
    "SAVETRD",
    "SETBREAKPOINT",
    "SHELLEXEC",
    "SIZE",
    "SLDOPT",
    "SLOT",
    "STRUCT",
    "TEXTAREA",
    "UNDEFINE",
    "UNPHASE",
    "WHILE",
    "WORD",
];

fn table() -> &'static HashSet<&'static str> {
    static TABLE: OnceLock<HashSet<&'static str>> = OnceLock::new();
    TABLE.get_or_init(|| MNEMONICS.iter().chain(DIRECTIVES).copied().collect())
}

/// True if `name` starts a statement rather than being a label.
///
/// Case-insensitive, and a leading `.` is ignored: sjasmplus accepts `.db` as
/// well as `db`, which is also why `.loop` in column 1 is a local label — it is
/// only a directive if the rest of it is one.
pub fn is_op_name(name: &str) -> bool {
    let bare = name.strip_prefix('.').unwrap_or(name);
    table().contains(bare.to_ascii_uppercase().as_str())
}
