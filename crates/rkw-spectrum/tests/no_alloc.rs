//! Running a Spectrum on the emulation thread does not allocate.
//!
//! ADR-0007's rule applies to this crate as much as to the debugger: the
//! machine is what the slice loop runs, and a border write, a frame boundary
//! and an accepted interrupt all happen on that thread — the first of them
//! thousands of times a second in software that stripes the border.
//!
//! The beeper is measured separately, and has to be. It runs on the emulation
//! thread once a frame — an edge into a fixed log on every `OUT`, and a
//! frame's worth of resampling and filtering inside `service_event` — but none
//! of it is reached by the plain `Emu<Spectrum>` case below, because that
//! machine has no beeper in it. A test that only covered the plain case would
//! leave every line of the new per-frame work outside the assertion that
//! exists to protect it.
//!
//! Rendering is measured too, and twice. Pulled — a debugger asking for a
//! picture — it is not on the emulation thread at all, and what must not
//! happen is for it to allocate 104 KB per frame when handed a buffer to
//! reuse. Pushed, which is what a frontend gets (ADR-0025), it *is* on the
//! emulation thread, fifty times a second, and the swap chain exists so that
//! publishing a frame costs an exchange of two boxes rather than a copy or an
//! allocation.

use std::sync::Arc;

use rkw_audio::ring;
use rkw_debug::Debugger;
use rkw_debug::command::Command;
use rkw_debug::emu::{Config, Emu, RunState};
use rkw_spectrum::keymap::{HostKey, HostKeys, KeyMap};
use rkw_spectrum::{
    AudioMachine, Flash, Framebuffer, Presenting, SCREEN_BASE, Saving, Spectrum, present,
};
use rkw_tape::Tap;
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
fn the_slice_loop_making_sound_does_not_allocate() {
    // STRIPES writes port $FE every forty T-states with an incrementing
    // accumulator, so bit 4 moves every sixteenth pass — about a hundred and
    // ten edges a frame, which is a real beeper load and comfortably inside
    // the log. Whether it fits is not the point; whether recording it, and
    // resampling it, and filtering it, and pushing it into the ring, touch the
    // heap is.
    let (cpu, spectrum) = machine();
    let (tx, rx) = ring::channel(1 << 16);
    let machine = AudioMachine::with_defaults(spectrum, 48_000, tx);

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
        "{SLICES} slices with the beeper running allocated {allocations} times"
    );

    // The run did what it was supposed to: frames ended, edges were recorded
    // and turned into sound, and nothing was lost at either end.
    assert!(emu.machine.spectrum.ula.frames() >= 6);
    assert!(
        rx.len() > 5_000,
        "six frames should be five thousand samples, not {}",
        rx.len()
    );
    assert_eq!(emu.machine.edges_dropped(), 0, "the edge log overflowed");
    assert_eq!(emu.machine.dropped(), 0, "the sample ring overflowed");
    assert!(
        emu.machine.fill() > 0.0,
        "nothing reached the ring, so the beeper was never exercised"
    );
}

#[test]
fn the_slice_loop_loading_and_saving_a_tape_does_not_allocate() {
    // A tape running turns `service_event` from a fifty-times-a-second thing
    // into a two-thousand-times-a-second one, and every one of those calls
    // pulls a pulse out of the player. The recorder is the other half: STRIPES
    // moves bit 3 along with the rest of the accumulator, so the `MIC` path is
    // fed as well.
    //
    // The tape is mounted outside the measured region on purpose — reading a
    // file and parsing it allocates, which is why it happens at the rate a
    // person mounts tapes and not on the emulation thread.
    let (cpu, mut spectrum) = machine();
    let tap = Tap::builder().block(0xFF, &[0x5A; 512]).build();
    spectrum.mount_tape(Arc::new(tap));
    spectrum.play_tape();
    let machine = Saving::new(spectrum);

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

    const SLICES: usize = 2_000;
    let (_, allocations) = alloc_check::count(|| {
        for _ in 0..SLICES {
            assert_eq!(emu.slice(), RunState::Running);
        }
    });
    assert_eq!(
        allocations, 0,
        "{SLICES} slices with a tape running allocated {allocations} times"
    );

    // The run did what it was supposed to: the tape moved, the machine saw the
    // edges, and the recorder was fed.
    let spectrum: &Spectrum = emu.machine.as_ref();
    assert!(spectrum.ula.frames() >= 6);
    assert!(
        spectrum.tape.is_playing(),
        "the tape stopped, so most of these slices had no pulses in them"
    );
    assert!(
        !spectrum.ula.audio().frame().0.is_empty(),
        "no port writes were logged, so the recorder was never fed"
    );
}

/// Key events arrive on whatever thread the frontend runs on and are applied
/// on the emulation thread (0026), and rebuilding the matrix from the host
/// keys held is what happens at that end. Fixed arrays throughout, which is
/// what [`rkw_spectrum::keymap::MAX_HELD`] exists for; this is the assertion
/// that says so.
/// Contention runs several times per instruction and the floating bus runs on
/// every `IN` from an unattached port. Neither has any business touching the
/// heap, and both are new enough that nothing else here reaches them: STRIPES
/// writes to the contended bank but never reads a port that floats.
#[test]
fn contention_and_the_floating_bus_do_not_allocate() {
    // Code and data both in the contended bank, and an odd port nothing
    // answers, so every machine cycle is contended and every IN goes through
    // the floating bus. E accumulates the AND of everything read back, so that
    // the run can prove it saw a fetched byte and not only an idle one.
    #[rustfmt::skip]
    const SPINNING: &[u8] = &[
        0x21, 0x00, 0x41,  // ld hl,$4100
        0x01, 0xFF, 0x00,  // ld bc,$00ff
        0x1E, 0xFF,        // ld e,$ff
        0x34,              // loop: inc (hl)
        0xED, 0x78,        //       in a,(c)
        0xA3,              //       and e
        0x5F,              //       ld e,a
        0x23,              //       inc hl
        0x18, 0xF8,        //       jr loop
    ];

    let mut machine = Spectrum::new();
    machine.memory.load(0x4000, SPINNING);
    let mut cpu = Cpu::new();
    cpu.regs.pc = 0x4000;
    cpu.regs.sp = 0xFF00;

    let (_, allocations) = alloc_check::count(Framebuffer::new);
    assert!(
        allocations > 0,
        "allocating a framebuffer allocated nothing, so the counting allocator \
         is not installed and this test proves nothing"
    );

    let (_, allocations) = alloc_check::count(|| {
        for _ in 0..200_000 {
            cpu.step(&mut machine);
        }
    });
    assert_eq!(
        allocations, 0,
        "a contended run allocated {allocations} times"
    );

    // The run did what it was supposed to: it spent most of its time inside
    // the display, so the arithmetic ran rather than short-circuiting.
    assert!(machine.t_states() > 3 * rkw_spectrum::T_STATES_PER_FRAME);
    assert_ne!(
        cpu.regs.e, 0xFF,
        "the floating bus never returned a fetched byte"
    );
}

#[test]
fn turning_host_key_events_into_a_matrix_does_not_allocate() {
    let mut host = HostKeys::new();
    let mut machine = Spectrum::new();

    let (held, allocations) = alloc_check::count(|| {
        let mut held = 0;
        for c in "the quick brown fox".chars() {
            let key = if c == ' ' {
                HostKey::Space
            } else {
                HostKey::Char(c)
            };
            host.press(HostKey::Shift);
            host.press(key);
            machine.ula.keyboard = host.matrix(&KeyMap::PC);
            held += usize::from(machine.ula.keyboard.any_pressed());
            host.release(key);
            host.release(HostKey::Shift);
            machine.ula.keyboard = host.matrix(&KeyMap::PC);
        }
        held
    });
    assert_eq!(allocations, 0, "key events allocated {allocations} times");
    // The run did what it was supposed to: every character put keys down, and
    // letting go put them all back up.
    assert_eq!(held, "the quick brown fox".len());
    assert_eq!(machine.ula.keyboard, rkw_spectrum::Keyboard::new());
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

#[test]
fn the_slice_loop_presenting_frames_does_not_allocate() {
    // The frontend's own machine, three wrappers deep: a Spectrum making sound
    // and painting each finished frame for a window. Every per-frame host job
    // in the program runs in this one, which is the case a frontend actually
    // has.
    let (cpu, spectrum) = machine();
    let (tx, _rx) = ring::channel(1 << 16);
    let (sink, mut frames) = present::channel();
    let machine = Presenting::new(AudioMachine::with_defaults(spectrum, 48_000, tx), sink);

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

    const SLICES: usize = 2_000;
    let (_, allocations) = alloc_check::count(|| {
        for _ in 0..SLICES {
            assert_eq!(emu.slice(), RunState::Running);
            // A consumer taking frames as they come, because a swap that only
            // ever ran on one side would not be exercising the exchange.
            frames.take();
        }
    });
    assert_eq!(
        allocations, 0,
        "{SLICES} slices publishing frames allocated {allocations} times"
    );

    // The run did what it was supposed to: frames were painted and taken.
    assert!(emu.machine.published() >= 6, "{}", emu.machine.published());
    assert!(frames.taken() >= 6, "{}", frames.taken());
}
