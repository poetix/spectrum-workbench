//! Frames leaving the emulation thread.
//!
//! The unit tests in `present.rs` check the swap chain in isolation. These
//! check it where it is actually used: a machine running under the slice loop,
//! publishing as each frame ends, with a consumer on the other side reading at
//! its own rate.

use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu, RunState};
use rkw_spectrum::frame::{BORDER_TOP, LINES_PER_FRAME, T_STATES_PER_FRAME};
use rkw_spectrum::{Presenting, Spectrum, present};
use z80::Cpu;

/// Set the border to `E`, count down, and stop — so the frames a run produces
/// have a border that changes from one to the next and a screen byte that
/// says which frame it was.
///
/// ```text
/// 8000  1E 02      loop:   ld e,2
/// 8002  7B                 ld a,e
/// 8003  D3 FE              out ($FE),a
/// 8005  3A 00 40           ld a,($4000)
/// 8008  3C                 inc a
/// 8009  32 00 40           ld ($4000),a
/// 800C  18 F2              jr loop
/// ```
#[rustfmt::skip]
const PAINTING: &[u8] = &[
    0x1E, 0x02,
    0x7B,
    0xD3, 0xFE,
    0x3A, 0x00, 0x40,
    0x3C,
    0x32, 0x00, 0x40,
    0x18, 0xF2,
];

fn emu(sink: rkw_spectrum::FrameSink) -> (Emu<Presenting<Spectrum>>, rkw_debug::emu::Handle) {
    let mut spectrum = Spectrum::new();
    spectrum.memory.load(0x8000, PAINTING);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x8000;
    cpu.regs.sp = 0xFF00;
    Emu::new(
        cpu,
        Presenting::new(spectrum, sink),
        rkw_debug::Debugger::new(),
        Config::default(),
    )
}

/// Slices enough to cover `frames` frames of a 48K machine, at one scanline
/// each.
fn run_frames(emu: &mut Emu<Presenting<Spectrum>>, frames: u64) {
    let slices = frames * T_STATES_PER_FRAME / 224 + 2;
    for _ in 0..slices {
        assert_eq!(emu.slice(), RunState::Running);
    }
}

#[test]
fn a_frame_is_published_for_every_frame_the_machine_finishes() {
    let (sink, mut frames) = present::channel();
    let (mut emu, mut handle) = emu(sink);
    handle.send(Command::Resume).unwrap();

    run_frames(&mut emu, 5);

    // Five frames ran, so five were painted — one per `end_frame` and not one
    // per `service_event`, which a tape would call thousands of times.
    assert_eq!(emu.machine.published(), 5);
    assert!(frames.take().is_some());
    // The consumer took the newest and the other four were dropped, which is
    // what a window that redraws once is entitled to.
    assert_eq!(frames.missed(), 4);
}

/// The border of the frame that has just ended, not the one before it: the ULA
/// presents the border at `end_frame`, so painting before that call would draw
/// last frame's stripes around this frame's screen.
#[test]
fn the_frame_carries_the_border_the_program_set_in_it() {
    let (sink, mut frames) = present::channel();
    let (mut emu, mut handle) = emu(sink);
    handle.send(Command::Resume).unwrap();

    run_frames(&mut emu, 2);
    let frame = frames.take().expect("a frame");

    // Red, top to bottom: the program sets it on every pass and never changes
    // it, so every visible line of the border is the same and it is not the
    // black of a machine that has published a frame that never ran.
    assert_eq!(frame.pixel(0, 0), 2);
    assert_eq!(frame.pixel(0, BORDER_TOP + 1), 2);
    assert_eq!(
        emu.machine.as_ref().ula.border_lines()[LINES_PER_FRAME - 1],
        2
    );
}

/// Nothing published means nothing to draw: the window keeps what it has
/// rather than being handed the same frame again.
#[test]
fn a_paused_machine_publishes_nothing() {
    let (sink, mut frames) = present::channel();
    let (mut emu, mut handle) = emu(sink);
    handle.send(Command::Resume).unwrap();
    run_frames(&mut emu, 1);
    assert!(frames.take().is_some());

    handle.send(Command::Pause).unwrap();
    for _ in 0..500 {
        emu.slice();
    }
    assert!(frames.take().is_none());
}
