//! Running under the debugger does not allocate.
//!
//! ADR-0007's rule is that nothing on the emulation thread allocates, and this
//! crate is what the emulation thread will be running (ticket 0009). The
//! breakpoint check is on the per-instruction path and the watchpoint check is
//! on the per-access path, so a `Vec` or a `String` reached from either would
//! cost more than everything else the emulator does.
//!
//! Arming and disarming are allowed to allocate — they happen at command rate,
//! when a person types something — so what is asserted here is the running.

mod common;

use common::machine;
use rkw_debug::{Debugger, StopReason};

#[global_allocator]
static ALLOC: alloc_check::Counting = alloc_check::Counting;

/// A loop with a memory read, a memory write and a branch in it, so the run
/// exercises the instruction path and both access paths.
///
/// ```text
/// 8000  21 00 90  restart: LD HL,$9000
/// 8003  06 00              LD B,0
/// 8005  7E        loop:    LD A,(HL)
/// 8006  3C                 INC A
/// 8007  77                 LD (HL),A
/// 8008  23                 INC HL
/// 8009  10 FA              DJNZ loop
/// 800B  C3 00 80           JP restart
/// ```
///
/// `HL` never leaves the 256 bytes at $9000, so the program cannot write over
/// itself however long it runs.
const BUSY: &[u8] = &[
    0x21, 0x00, 0x90, 0x06, 0x00, 0x7E, 0x3C, 0x77, 0x23, 0x10, 0xFA, 0xC3, 0x00, 0x80,
];

const INSTRUCTIONS: u64 = 100_000;

#[test]
fn running_does_not_allocate() {
    let (mut cpu, mut mem) = machine(BUSY);
    let mut dbg = Debugger::new();

    // The control: without it, an allocator that failed to install would
    // report zero for everything and this test would prove nothing.
    let (_, allocations) = alloc_check::count(|| dbg.breakpoints.add_exec(0x1234));
    assert!(
        allocations > 0,
        "arming a breakpoint allocated nothing, so the counting allocator is \
         not installed and this test proves nothing"
    );

    // Armed, but at an address the program never reaches, and watching an
    // address it never touches: the bitmaps answer every question.
    dbg.breakpoints.add_exec(0x4321);
    dbg.breakpoints.watch_mem(0xA000, true, true);
    dbg.breakpoints.watch_port(0x00FF, 0x00FE, true, true);

    let (stop, allocations) = alloc_check::count(|| dbg.resume(&mut cpu, &mut mem, INSTRUCTIONS));
    assert_eq!(stop, StopReason::OutOfBudget);
    assert_eq!(
        allocations, 0,
        "running {INSTRUCTIONS} instructions allocated {allocations} times"
    );
    assert_eq!(dbg.breakpoints.detail_probes(), 0);
}

#[test]
fn stopping_does_not_allocate_either() {
    let (mut cpu, mut mem) = machine(BUSY);
    let mut dbg = Debugger::new();
    let id = dbg.breakpoints.add_exec(0x8005);
    dbg.breakpoints.watch_mem(0x9000, false, true);

    let (stop, allocations) = alloc_check::count(|| dbg.resume(&mut cpu, &mut mem, INSTRUCTIONS));
    assert_eq!(
        stop,
        StopReason::Breakpoint { id, addr: 0x8005 },
        "reaching a breakpoint is the path that consults the map"
    );
    assert_eq!(allocations, 0);

    let (stop, allocations) = alloc_check::count(|| dbg.resume(&mut cpu, &mut mem, INSTRUCTIONS));
    assert!(
        matches!(stop, StopReason::Watchpoint { .. }),
        "and this is the path that consults it from the bus: {stop:?}"
    );
    assert_eq!(allocations, 0);
}
