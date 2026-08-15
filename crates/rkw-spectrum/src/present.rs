//! Getting the picture off the emulation thread.
//!
//! A frame is [`WIDTH`] x [`HEIGHT`] palette indices — a hundred kilobytes,
//! which is six thousand times what the event ring of ADR-0007 carries in a
//! record. So the picture cannot go through the ring, and the two threads need
//! something else: a swap chain of two framebuffers, in which publishing a
//! frame and taking one are both a pointer swap and neither waits for the
//! other (ADR-0025).
//!
//! # Why the machine does not simply hand out a reference
//!
//! [`Spectrum::render`] is pull-based on purpose: a debugger stopped at a
//! breakpoint wants the screen at a moment of its own choosing, and a headless
//! run should not paint anything at all. That works because the caller owns
//! the machine. A frontend does not — the machine is on the emulation thread,
//! behind a [`Handle`](rkw_debug::emu::Handle) that carries sixteen-byte
//! records — so somebody on that thread has to do the painting and leave the
//! result where the window can find it.
//!
//! That somebody is [`Presenting`], which is a machine wrapping a machine in
//! exactly the way [`AudioMachine`](crate::AudioMachine) and
//! [`Saving`](crate::Saving) are, and for the same reason: the per-frame host
//! work has to happen inside `service_event`, and [`Spectrum`] is the one type
//! that must stay plain machine state (ADR-0017).
//!
//! # What the frontend gets, and what it does not
//!
//! It gets the most recent complete frame, and it is told how many were
//! published in between. It does not get all of them: a window that stopped
//! for a fortnight would otherwise hold a fortnight of frames, and the only
//! honest thing to do with a frame nobody drew is drop it and say so. That is
//! the same bargain the event ring makes, for the same reason.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rkw_debug::command::Tape as TapeButton;
use rkw_debug::machine::{Clock, Machine};
use z80::Bus;
use z80::disasm::Peek;

use crate::screen::Framebuffer;
use crate::spectrum::Spectrum;

/// The buffer the two ends swap through, and the counters that say what has
/// been happening to it.
struct Shared {
    /// The frame most recently published, waiting for a taker.
    ///
    /// A `Mutex` around a swap, not around the drawing: neither end holds it
    /// for longer than it takes to exchange two boxes, and the producer never
    /// waits on it at all — see [`FrameSink::publish`].
    slot: Mutex<Framebuffer>,
    published: AtomicU64,
    taken: AtomicU64,
    missed: AtomicU64,
    /// Whether the deck was running when the last frame was published.
    ///
    /// A window that remembered whether it had pressed play would be wrong
    /// twice a load: a tape runs off its end, and a TZX block can stop it. So
    /// the answer comes from the machine, once a frame, up the same path as
    /// the picture — which is quite fast enough for a title bar, and is
    /// nowhere near the emulation thread's hot path.
    tape_playing: AtomicBool,
}

/// The producer's end, which lives on the emulation thread.
pub struct FrameSink {
    shared: Arc<Shared>,
    /// The buffer being painted into. Swapped with the slot's on publication,
    /// so the sink always owns one and never allocates.
    scratch: Framebuffer,
}

/// The consumer's end, which lives wherever the window does.
pub struct FrameSource {
    shared: Arc<Shared>,
    /// The frame handed out last, kept so that [`FrameSource::latest`] can go
    /// on answering when nothing new has arrived.
    current: Framebuffer,
    seen: u64,
}

/// A sink and the source that reads it.
pub fn channel() -> (FrameSink, FrameSource) {
    let shared = Arc::new(Shared {
        slot: Mutex::new(Framebuffer::new()),
        published: AtomicU64::new(0),
        taken: AtomicU64::new(0),
        missed: AtomicU64::new(0),
        tape_playing: AtomicBool::new(false),
    });
    // Take the lock once, here, and let it go. On some platforms a `Mutex`
    // allocates the first time it is locked, and the first lock would
    // otherwise happen on the emulation thread — which is the one place in the
    // program that may not allocate (ADR-0007). `tests/no_alloc.rs` is what
    // notices if this line is removed.
    drop(shared.slot.lock().expect("a fresh lock cannot be poisoned"));

    let sink = FrameSink {
        shared: Arc::clone(&shared),
        scratch: Framebuffer::new(),
    };
    let source = FrameSource {
        shared,
        current: Framebuffer::new(),
        seen: 0,
    };
    (sink, source)
}

impl FrameSink {
    /// Paint `machine` and publish the result.
    ///
    /// Two properties matter, and both come from the swap. The first is that
    /// this never blocks: the lock is taken with `try_lock`, and a frame that
    /// finds the consumer inside its own swap is dropped rather than waited
    /// for — at 50 Hz the next one is twenty milliseconds away and the
    /// consumer holds the lock for a pointer exchange. The second is that it
    /// never allocates: the buffer that goes in is the one that comes out.
    pub fn publish(&mut self, machine: &Spectrum) {
        machine.render(&mut self.scratch);
        // Before the `try_lock`, so a dropped frame still leaves the deck's
        // state current: a load that finished is worth knowing about whether
        // or not the window drew the frame it finished on.
        self.shared
            .tape_playing
            .store(machine.tape.is_playing(), Ordering::Relaxed);
        let Ok(mut slot) = self.shared.slot.try_lock() else {
            self.shared.missed.fetch_add(1, Ordering::Relaxed);
            return;
        };
        std::mem::swap(&mut *slot, &mut self.scratch);
        drop(slot);
        self.shared.published.fetch_add(1, Ordering::Release);
    }

    /// Frames published since the machine started.
    pub fn published(&self) -> u64 {
        self.shared.published.load(Ordering::Relaxed)
    }
}

impl FrameSource {
    /// The newest frame, if one has arrived since the last call.
    ///
    /// `None` means nothing has been published since — a paused machine, or a
    /// window redrawing faster than 50 Hz — and the caller should leave on
    /// screen whatever it drew last. [`FrameSource::latest`] is that frame,
    /// for a caller that would rather not keep track.
    pub fn take(&mut self) -> Option<&Framebuffer> {
        let published = self.shared.published.load(Ordering::Acquire);
        if published == self.seen {
            return None;
        }
        let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::swap(&mut *slot, &mut self.current);
        drop(slot);
        // Frames published while the last one sat here unread. Dropped, like an
        // event the ring overwrote, and counted for the same reason.
        self.shared
            .missed
            .fetch_add(published - self.seen - 1, Ordering::Relaxed);
        self.seen = published;
        self.shared.taken.fetch_add(1, Ordering::Relaxed);
        Some(&self.current)
    }

    /// The last frame taken, whether or not it is still the newest. A window
    /// redrawing because it was resized wants this and not [`take`](Self::take).
    pub fn latest(&self) -> &Framebuffer {
        &self.current
    }

    /// Frames the machine painted that will never be drawn: published while an
    /// unread one was waiting, or published while the consumer was swapping.
    /// A frontend that is keeping up reports zero.
    pub fn missed(&self) -> u64 {
        self.shared.missed.load(Ordering::Relaxed)
    }

    /// Frames handed to the consumer.
    pub fn taken(&self) -> u64 {
        self.shared.taken.load(Ordering::Relaxed)
    }

    /// Whether the tape was running at the last frame the machine painted.
    ///
    /// A frontend shows this rather than what it last asked for, because the
    /// deck stops on its own: at the end of the tape, and at a TZX block that
    /// says to.
    pub fn tape_playing(&self) -> bool {
        self.shared.tape_playing.load(Ordering::Relaxed)
    }
}

/// A machine that paints each finished frame where a frontend can pick it up.
///
/// Generic over what it wraps, like [`Saving`](crate::Saving), so a frontend
/// stacks it over an [`AudioMachine`](crate::AudioMachine) and gets both.
pub struct Presenting<M> {
    inner: M,
    sink: FrameSink,
}

impl<M: Machine + AsRef<Spectrum>> Presenting<M> {
    /// Wrap a machine, publishing its frames to `sink`.
    pub fn new(inner: M, sink: FrameSink) -> Presenting<M> {
        Presenting { inner, sink }
    }

    /// The machine underneath, for everything that is not the picture.
    pub fn inner(&self) -> &M {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut M {
        &mut self.inner
    }

    /// Frames published so far.
    pub fn published(&self) -> u64 {
        self.sink.published()
    }
}

impl<M: Bus> Bus for Presenting<M> {
    fn read(&mut self, addr: u16) -> u8 {
        self.inner.read(addr)
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.inner.write(addr, value);
    }

    fn input(&mut self, port: u16) -> u8 {
        self.inner.input(port)
    }

    fn output(&mut self, port: u16, value: u8) {
        self.inner.output(port, value);
    }

    fn tick(&mut self, t: u32) {
        self.inner.tick(t);
    }

    fn tick_at(&mut self, addr: u16, t: u32) {
        self.inner.tick_at(addr, t);
    }

    fn fetch_opcode(&mut self, addr: u16) -> u8 {
        self.inner.fetch_opcode(addr)
    }

    fn read_cycle(&mut self, addr: u16) -> u8 {
        self.inner.read_cycle(addr)
    }

    fn write_cycle(&mut self, addr: u16, value: u8) {
        self.inner.write_cycle(addr, value);
    }

    fn input_cycle(&mut self, port: u16) -> u8 {
        self.inner.input_cycle(port)
    }

    fn output_cycle(&mut self, port: u16, value: u8) {
        self.inner.output_cycle(port, value);
    }

    fn interrupt_data(&mut self) -> u8 {
        self.inner.interrupt_data()
    }

    fn interrupt_pending(&self) -> bool {
        self.inner.interrupt_pending()
    }

    fn nmi_pending(&mut self) -> bool {
        self.inner.nmi_pending()
    }
}

impl<M: Peek> Peek for Presenting<M> {
    fn peek(&self, addr: u16) -> u8 {
        self.inner.peek(addr)
    }
}

impl<M: AsRef<Spectrum>> AsRef<Spectrum> for Presenting<M> {
    fn as_ref(&self) -> &Spectrum {
        self.inner.as_ref()
    }
}

impl<M: AsMut<Spectrum>> AsMut<Spectrum> for Presenting<M> {
    fn as_mut(&mut self) -> &mut Spectrum {
        self.inner.as_mut()
    }
}

impl<M: Clock> Clock for Presenting<M> {
    fn t_states(&self) -> u64 {
        self.inner.t_states()
    }
}

impl<M: Machine + AsRef<Spectrum>> Machine for Presenting<M> {
    fn next_event(&self) -> Option<u64> {
        self.inner.next_event()
    }

    /// Let the machine finish the frame, then paint it — which is the opposite
    /// order to the beeper's, and for a reason that is easy to get backwards.
    ///
    /// The beeper reads the edge log, which
    /// [`Ula::end_frame`](crate::Ula::end_frame) rolls on, so it has to read
    /// first. The border is the other way round: `end_frame` is what *presents*
    /// it, and painting before the call would draw the frame before last's
    /// border around this frame's screen. Getting either wrong is silent —
    /// silence in one case and a frame of border lag in the other — hence the
    /// tests.
    fn service_event(&mut self) {
        let frame = self.inner.as_ref().frame_due();
        self.inner.service_event();
        if frame {
            self.sink.publish(self.inner.as_ref());
        }
    }

    fn set_keys(&mut self, matrix: u64) {
        self.inner.set_keys(matrix);
    }

    fn tape(&mut self, button: TapeButton) {
        self.inner.tape(button);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{HEIGHT, WIDTH};

    /// A frame's border is only the presented one once the frame has ended,
    /// which is what `Presenting` waits for and what these have to imitate.
    fn frame_with_border(machine: &mut Spectrum, colour: u8) {
        machine.ula.write_port_fe(0, colour);
        machine.ula.end_frame();
    }

    #[test]
    fn nothing_is_taken_until_something_is_published() {
        let (mut sink, mut source) = channel();
        assert!(source.take().is_none());

        let mut machine = Spectrum::new();
        frame_with_border(&mut machine, 2); // A red border.
        sink.publish(&machine);

        assert_eq!(source.take().map(|f| f.pixel(0, 0)), Some(2));
        // And only once: the same frame is not new twice.
        assert!(source.take().is_none());
        assert_eq!(source.latest().pixel(0, 0), 2);
        assert_eq!(source.missed(), 0);
        assert_eq!(source.taken(), 1);
    }

    #[test]
    fn a_consumer_that_falls_behind_gets_the_newest_frame_and_a_count_of_the_rest() {
        let (mut sink, mut source) = channel();
        let mut machine = Spectrum::new();
        for border in 1..=4 {
            frame_with_border(&mut machine, border);
            sink.publish(&machine);
        }

        assert_eq!(source.take().map(|f| f.pixel(0, 0)), Some(4));
        assert_eq!(source.missed(), 3);
        assert_eq!(sink.published(), 4);
    }

    /// The swap is what makes publication allocation-free and the pixels
    /// distinct: the two ends must never end up looking at one buffer.
    #[test]
    fn the_two_ends_hold_different_buffers() {
        let (mut sink, mut source) = channel();
        let mut machine = Spectrum::new();
        frame_with_border(&mut machine, 1);
        sink.publish(&machine);
        source.take();

        // Painting again while the consumer holds its frame does not touch it.
        frame_with_border(&mut machine, 6);
        sink.publish(&machine);
        assert_eq!(source.latest().pixel(0, 0), 1);
        assert_eq!(source.take().map(|f| f.pixel(0, 0)), Some(6));
    }

    /// The window's tape light. It has to come from the deck rather than from
    /// what the window last asked for, because the deck stops on its own.
    #[test]
    fn the_deck_reports_itself_with_each_frame() {
        let (mut sink, source) = channel();
        let mut machine = Spectrum::new();
        sink.publish(&machine);
        assert!(!source.tape_playing(), "nothing is mounted");

        machine.mount_tape(rkw_tape::Tap::builder().block(0xFF, &[0x00]).build());
        machine.tape(TapeButton::Play);
        sink.publish(&machine);
        assert!(source.tape_playing());

        machine.tape(TapeButton::Stop);
        sink.publish(&machine);
        assert!(!source.tape_playing());
    }

    #[test]
    fn a_published_frame_is_the_whole_visible_picture() {
        let (mut sink, mut source) = channel();
        let machine = Spectrum::new();
        sink.publish(&machine);
        let frame = source.take().expect("a frame");
        assert_eq!(frame.pixels().len(), WIDTH * HEIGHT);
    }
}
