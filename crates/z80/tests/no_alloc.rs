//! Decoding does not allocate.
//!
//! ADR-0007 puts the whole debugger design on one rule: nothing on the
//! emulation thread allocates. The trace ring records an instruction per step
//! and step-over needs an instruction's length, so decoding runs on that
//! thread at emulation rate — 157 million instructions a second — where a
//! single `Vec` per instruction would cost more than everything else the
//! emulator does.
//!
//! The split that makes that true (`decode` for what an instruction is,
//! `text` for how it reads) is easy to undo by accident: adding a `String`
//! somewhere in the walk breaks no other test, and the cost does not show up
//! until something is measured under load. So it is asserted here instead.

use z80::{FlatMemory, decode, disassemble};

#[global_allocator]
static ALLOC: alloc_check::Counting = alloc_check::Counting;

/// Every address in a 64K space filled with a repeating byte pattern, so that
/// every opcode value is decoded in every prefix page, including the `DD CB`
/// forms and the prefix chains.
fn filled() -> FlatMemory {
    let mut mem = FlatMemory::new();
    let bytes: Vec<u8> = (0..=0xFFFFu32).map(|i| (i % 253) as u8).collect();
    mem.load(0, &bytes);
    mem
}

#[test]
fn decoding_allocates_nothing() {
    let mem = filled();

    // The control. Without this, an allocator that failed to install would
    // report zero for everything and the test would pass vacuously.
    let (insn, allocations) = alloc_check::count(|| disassemble(&mem, 0x8000));
    assert!(
        allocations > 0,
        "rendering {} allocated nothing, so the counting allocator is not \
         installed and this test proves nothing",
        insn.text
    );

    // The real assertion. The accumulator keeps the loop from being optimised
    // away, and reads every field so that a decode reduced to nothing would
    // not pass by being nothing.
    let (acc, allocations) = alloc_check::count(|| {
        let mut acc = 0usize;
        for addr in 0..=0xFFFFu16 {
            let d = decode(&mem, addr);
            acc += usize::from(d.len)
                + usize::from(d.undocumented)
                + usize::from(d.flow.falls_through())
                + usize::from(d.next_addr());
        }
        acc
    });
    std::hint::black_box(acc);
    assert_eq!(
        allocations, 0,
        "decoding 65536 instructions allocated {allocations} times"
    );
}
