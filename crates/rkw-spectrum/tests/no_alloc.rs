//! Running a Spectrum on the emulation thread does not allocate.
//!
//! ADR-0007's rule applies to this crate as much as to the debugger: the
//! machine is what the slice loop runs, and a border write, a frame boundary
//! and an accepted interrupt all happen on that thread — the first of them
//! thousands of times a second in software that stripes the border.
//!
//! Rendering is measured too, but for a different reason. It is not on the
//! emulation thread at all; it is on whatever thread asked for a picture, at
//! most fifty times a second. What must not happen is for it to allocate 104 KB
//! per frame when handed a buffer to reuse.

use rkw_debug::Debugger;
use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu, RunState};
use rkw_spectrum::{Flash, Framebuffer, SCREEN_BASE, Spectrum};
use z80::Cpu;

#[global_allocator]
static ALLOC: alloc_check::Counting = alloc_check::Counting;

/// Interrupts on, a `HALT` never reached, and an `OUT` to the border on every
/// pass, so the measured run covers the frame clock, the border log and the
/// interrupt path.
///
/// ```text
/// 8000  ED 56              IM 1
/// 8002  FB                 EI
/// 8003  3E 00              LD A,0
/// 8005  D3 FE      loop:   OUT ($FE),A
/// 8007  3C                 INC A
/// 8008  32 00 40           LD ($4000),A
/// 800B  18 F8              JR loop
/// ```
const STRIPES: &[u8] = &[
    0xED, 0x56, 0xFB, 0x3E, 0x00, 0xD3, 0xFE, 0x3C, 0x32, 0x00, 0x40, 0x18, 0xF8,
];

/// `EI` and `RETI`, poked into ROM because that is where `IM 1` sends it.
const HANDLER: &[u8] = &[0xFB, 0xED, 0x4D];

fn machine() -> (Cpu, Spectrum) {
    let mut machine = Spectrum::new();
    machine.memory.load(0x8000, STRIPES);
    machine.memory.load(0x0038, HANDLER);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    (cpu, machine)
}

#[test]
fn the_slice_loop_running_a_spectrum_does_not_allocate() {
    let (cpu, machine) = machine();

    // The control: without it, an allocator that failed to install would
    // report zero for everything and this test would prove nothing.
    let (_, allocations) = alloc_check::count(Framebuffer::new);
    assert!(
        allocations > 0,
        "allocating a framebuffer allocated nothing, so the counting allocator \
         is not installed and this test proves nothing"
    );

    let (mut emu, mut handle) = Emu::new(
        cpu,
        machine,
        Debugger::new(),
        Config {
            event_capacity: 16,
            command_capacity: 16,
            control_interval: 224,
            log_capacity: 0,
        },
    );
    handle.send(Command::Resume).unwrap();

    // Six frames and a bit, at one slice per scanline.
    const SLICES: usize = 2_000;
    let (_, allocations) = alloc_check::count(|| {
        for _ in 0..SLICES {
            assert_eq!(emu.slice(), RunState::Running);
        }
    });
    assert_eq!(
        allocations, 0,
        "{SLICES} slices allocated {allocations} times on the emulation thread"
    );

    // The run did what it was supposed to: frames ended, the border moved, and
    // interrupts were taken.
    assert!(
        emu.machine.ula.frames() >= 6,
        "{}",
        emu.machine.ula.frames()
    );
    let border = emu.machine.ula.border_lines();
    assert!(
        border.iter().any(|&c| c != border[0]),
        "the border never changed, so the log was never exercised"
    );
    assert!(emu.cpu.regs.i == 0 && emu.cpu.regs.iff1);
}

#[test]
fn rendering_into_a_reused_framebuffer_does_not_allocate() {
    let (_, machine) = machine();
    let mut frame = Framebuffer::new();

    let (_, allocations) = alloc_check::count(|| {
        for _ in 0..50 {
            machine.render(&mut frame);
            machine.render_with(0xC000, Flash::Inverted, &mut frame);
        }
    });
    assert_eq!(allocations, 0, "rendering allocated {allocations} times");

    // And the allocating conveniences are the ones that allocate, which is the
    // distinction the two APIs exist to draw.
    let (_, allocations) = alloc_check::count(|| machine.frame());
    assert!(allocations > 0);
    let (_, allocations) =
        alloc_check::count(|| rkw_spectrum::decode(&machine, SCREEN_BASE, Flash::Normal));
    assert!(allocations > 0);
}
