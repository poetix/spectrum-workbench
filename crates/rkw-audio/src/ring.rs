//! A single-producer single-consumer ring of samples.
//!
//! The emulation thread makes 20 ms of sound at a time, in a lump, at whatever
//! rate it happens to be running; the device asks for a few milliseconds at a
//! time, on the dot, on a thread that must never block. This is what sits
//! between them.
//!
//! # It refuses rather than overwriting, which makes it simpler as well as
//! # better
//!
//! `rkw-debug`'s event ring overwrites its oldest record when it is full,
//! because emulation must not wait for a debugger that stopped reading, and
//! the price of that is the fence-and-retry dance in ADR-0018: a consumer
//! reading a slot the producer might be in the middle of has to notice
//! afterwards and throw the record away.
//!
//! Audio has the opposite requirement and therefore gets the easier
//! implementation. An overwritten sample in the middle of the buffer is not a
//! lost sample, it is a discontinuity and a fragment of the wrong moment in
//! the wrong place — a bang, and then everything after it out of order. A
//! *refused* sample is a gap at the end, which is a thing the producer can
//! count and the consumer never hears, because the consumer is behind. So this
//! ring refuses, and because it refuses the producer never touches a slot the
//! consumer might be reading, and plain acquire/release on the two indices is
//! the whole of the synchronisation.
//!
//! # Being full is the normal state, and is the pacing signal
//!
//! The emulation core runs at about 360 times real time (ADR-0007). Left
//! alone it fills this in a few milliseconds and then drops everything, which
//! is not a failure but the mechanism: how full the ring is, is how far ahead
//! of the speaker the machine has got, and running frames until it is nearly
//! full and then waiting is how the front end (ticket 0019) should pace
//! itself. An emulator paced by its audio buffer needs no other clock and
//! never has to correct for drift.
//!
//! # No `unsafe`, and no `unsafe`-shaped trick either
//!
//! A slot is an `AtomicU32` holding `f32::to_bits`, so publishing a sample is
//! an ordinary atomic store and the workspace's ban on `unsafe` costs nothing
//! at all here. The transfer is in bulk — a slice in, a slice out — because
//! the consumer is a device callback that wants a buffer and not an iterator.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Cache-line padding, so that the producer's index and the consumer's index
/// are not on the same line and each thread's writes do not invalidate the
/// other's reads.
///
/// ADR-0010 says this number is per target and should come from
/// `crossbeam_utils` rather than from a measurement of somebody's laptop. This
/// crate has no dependencies on purpose, so it takes the pessimistic constant
/// instead: 128 covers Apple Silicon's 128-byte lines and x86's adjacent-line
/// prefetch, and being too generous costs a few bytes once.
#[repr(align(128))]
struct Padded<T>(T);

struct Ring {
    slots: Box<[AtomicU32]>,
    /// Capacity minus one. The capacity is a power of two, so wrapping is a
    /// mask rather than a division.
    mask: u64,
    /// Samples ever written. Only the producer stores to it.
    head: Padded<AtomicU64>,
    /// Samples ever read. Only the consumer stores to it.
    tail: Padded<AtomicU64>,
    /// Samples the producer had nowhere to put. Only the producer stores to
    /// it, because it is the producer that is refused.
    dropped: Padded<AtomicU64>,
    /// Samples the consumer asked for and did not get. Only the consumer
    /// stores to it.
    starved: Padded<AtomicU64>,
}

/// Make a ring holding `capacity` samples, which must be a power of two.
///
/// At 48 kHz, 4096 samples is 85 ms and 8192 is 171 ms — a latency budget and
/// a safety margin respectively.
pub fn channel(capacity: usize) -> (SampleTx, SampleRx) {
    assert!(
        capacity.is_power_of_two() && capacity >= 2,
        "ring capacity must be a power of two of at least 2, not {capacity}"
    );
    let ring = Arc::new(Ring {
        slots: (0..capacity)
            .map(|_| AtomicU32::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        mask: capacity as u64 - 1,
        head: Padded(AtomicU64::new(0)),
        tail: Padded(AtomicU64::new(0)),
        dropped: Padded(AtomicU64::new(0)),
        starved: Padded(AtomicU64::new(0)),
    });
    (
        SampleTx {
            ring: Arc::clone(&ring),
        },
        SampleRx { ring },
    )
}

/// The emulation thread's end.
pub struct SampleTx {
    ring: Arc<Ring>,
}

impl SampleTx {
    /// Write as much of `samples` as there is room for, and count the rest.
    ///
    /// Returns how many were taken. A short return is ordinary — see the
    /// module note on pacing — and what was refused is the *tail* of the
    /// slice, so what was accepted is always a contiguous run.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let head = self.ring.head.0.load(Ordering::Relaxed);
        let tail = self.ring.tail.0.load(Ordering::Acquire);
        let free = (self.ring.slots.len() as u64 - (head - tail)) as usize;

        let taken = samples.len().min(free);
        for (i, &sample) in samples[..taken].iter().enumerate() {
            let slot = ((head + i as u64) & self.ring.mask) as usize;
            self.ring.slots[slot].store(sample.to_bits(), Ordering::Relaxed);
        }
        // Publish the samples only once every one of them is in place.
        self.ring.head.0.store(head + taken as u64, Ordering::Release);

        if taken < samples.len() {
            let missed = (samples.len() - taken) as u64;
            let total = self.ring.dropped.0.load(Ordering::Relaxed) + missed;
            self.ring.dropped.0.store(total, Ordering::Relaxed);
        }
        taken
    }

    /// Samples waiting to be read.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Whether the consumer has read everything written.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Room for that many more.
    pub fn free(&self) -> usize {
        self.ring.slots.len() - self.len()
    }

    /// How full the ring is, from 0.0 to 1.0 — the pacing signal.
    pub fn fill(&self) -> f32 {
        self.len() as f32 / self.ring.slots.len() as f32
    }

    /// Samples there was no room for, over the life of the stream.
    pub fn dropped(&self) -> u64 {
        self.ring.dropped.0.load(Ordering::Relaxed)
    }
}

/// The device's end.
pub struct SampleRx {
    ring: Arc<Ring>,
}

impl SampleRx {
    /// Fill as much of `out` as there are samples for, and return how many
    /// that was. A short return is an underrun, and is counted.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        let tail = self.ring.tail.0.load(Ordering::Relaxed);
        let head = self.ring.head.0.load(Ordering::Acquire);
        let ready = (head - tail) as usize;

        let taken = out.len().min(ready);
        for (i, slot) in out[..taken].iter_mut().enumerate() {
            let index = ((tail + i as u64) & self.ring.mask) as usize;
            *slot = f32::from_bits(self.ring.slots[index].load(Ordering::Relaxed));
        }
        // Release the slots only once every sample is out of them.
        self.ring.tail.0.store(tail + taken as u64, Ordering::Release);

        if taken < out.len() {
            let missed = (out.len() - taken) as u64;
            let total = self.ring.starved.0.load(Ordering::Relaxed) + missed;
            self.ring.starved.0.store(total, Ordering::Relaxed);
        }
        taken
    }

    /// Samples waiting to be read.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Whether there is nothing to read.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Samples asked for and not there, over the life of the stream.
    pub fn starved(&self) -> u64 {
        self.ring.starved.0.load(Ordering::Relaxed)
    }

    /// Samples the producer had no room for, over the life of the stream.
    pub fn dropped(&self) -> u64 {
        self.ring.dropped.0.load(Ordering::Relaxed)
    }
}

impl Ring {
    fn len(&self) -> usize {
        let head = self.head.0.load(Ordering::Acquire);
        let tail = self.tail.0.load(Ordering::Acquire);
        (head - tail) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_goes_in_comes_out_in_the_order_it_went_in() {
        let (mut tx, mut rx) = channel(16);
        let sent: Vec<f32> = (0..10).map(|i| i as f32).collect();
        assert_eq!(tx.push(&sent), 10);

        let mut got = [0.0f32; 10];
        assert_eq!(rx.pop(&mut got), 10);
        assert_eq!(got.to_vec(), sent);
        assert!(rx.is_empty());
    }

    #[test]
    fn a_full_ring_refuses_the_tail_and_counts_it() {
        let (mut tx, mut rx) = channel(8);
        let sent: Vec<f32> = (0..20).map(|i| i as f32).collect();

        assert_eq!(tx.push(&sent), 8);
        assert_eq!(tx.dropped(), 12);
        assert_eq!(tx.free(), 0);

        // What it took is the *front* of the slice, contiguous, in order.
        let mut got = [0.0f32; 8];
        assert_eq!(rx.pop(&mut got), 8);
        assert_eq!(got.to_vec(), sent[..8].to_vec());
    }

    #[test]
    fn an_empty_ring_gives_what_it_has_and_counts_the_shortfall() {
        let (mut tx, mut rx) = channel(8);
        tx.push(&[1.0, 2.0, 3.0]);

        let mut got = [0.0f32; 8];
        assert_eq!(rx.pop(&mut got), 3);
        assert_eq!(got[..3].to_vec(), vec![1.0, 2.0, 3.0]);
        assert_eq!(rx.starved(), 5);

        assert_eq!(rx.pop(&mut got), 0);
        assert_eq!(rx.starved(), 13);
    }

    #[test]
    fn the_ring_wraps_without_losing_or_reordering_anything() {
        // Round the buffer many times in uneven lumps, so that pushes and pops
        // straddle the wrap in every alignment. The producer sends a counter
        // and only advances it by what was accepted, so the accepted stream is
        // contiguous and the consumer must see exactly 0, 1, 2, ... whatever
        // was refused along the way.
        let (mut tx, mut rx) = channel(8);
        let mut sent = 0u32;
        let mut got = 0u32;
        let mut buf = [0.0f32; 5];

        for round in 0..200 {
            let lump: Vec<f32> = (0..3).map(|i| (sent + i) as f32).collect();
            sent += tx.push(&lump) as u32;

            // Draining slower than filling, so the ring spends time full and
            // the refusal path is exercised rather than merely present.
            let asked = (round % 3) + 1;
            let n = rx.pop(&mut buf[..asked]);
            for &sample in &buf[..n] {
                assert_eq!(sample, got as f32, "sample {got} came back out of order");
                got += 1;
            }
        }

        assert!(got > 300, "the test should have moved samples, not {got}");
        assert_eq!(sent - got, rx.len() as u32, "the ring holds the difference");
        assert!(tx.dropped() > 0, "the ring should have filled at some point");
    }

    #[test]
    fn a_pushed_run_is_readable_in_pieces() {
        let (mut tx, mut rx) = channel(16);
        let sent: Vec<f32> = (0..12).map(|i| i as f32 * 0.25).collect();
        tx.push(&sent);

        let mut all = Vec::new();
        let mut buf = [0.0f32; 5];
        loop {
            let n = rx.pop(&mut buf);
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all, sent);
    }

    #[test]
    fn fill_reports_how_far_ahead_of_the_speaker_the_machine_is() {
        let (mut tx, mut rx) = channel(8);
        assert_eq!(tx.fill(), 0.0);

        tx.push(&[0.0; 4]);
        assert_eq!(tx.fill(), 0.5);

        tx.push(&[0.0; 4]);
        assert_eq!(tx.fill(), 1.0);
        assert_eq!(tx.free(), 0);

        let mut got = [0.0f32; 8];
        rx.pop(&mut got);
        assert_eq!(tx.fill(), 0.0);
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn a_capacity_that_is_not_a_power_of_two_is_a_mistake() {
        channel(100);
    }
}
