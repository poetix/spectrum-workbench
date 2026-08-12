//! A single-producer single-consumer ring of 16-byte records.
//!
//! Both of ADR-0007's queues are this ring; what differs is what happens when
//! it is full. The outbound event queue overwrites the oldest record, because
//! emulation must never wait for a consumer that stopped reading. The inbound
//! command queue refuses the write, because a dropped command is a lost
//! keystroke.
//!
//! # Why records rather than values
//!
//! A slot is two `AtomicU64`, and a [`Record`] is anything that encodes into
//! that pair. Nothing here is generic over a payload it would have to move,
//! so the ring needs no `unsafe` to publish one — which matters, because the
//! workspace forbids it. The cost is that every message type writes its own
//! bit layout, which is 16 bytes either way and is at least checkable by a
//! round-trip test.
//!
//! # How the consumer avoids reading a record out from under itself
//!
//! The producer never reads the consumer's index when it is allowed to
//! overwrite, so it cannot be blocked; it writes a slot and then publishes an
//! incremented head. A consumer reading slot `tail` while the producer is
//! writing would see a half-written record, so it re-reads the head *after*
//! copying the record out and discards the record if the producer could have
//! been in the slot meanwhile.
//!
//! "Could have been" is the whole of it. The producer writes the slot before
//! it publishes the head that covers it, so a head one whole lap ahead of
//! `tail` means the slot at `tail` is being overwritten *now* — not that it is
//! about to be. The consumer therefore treats the record a lap behind the head
//! as already lost, which costs one slot's worth of pessimism: a ring of
//! sixteen delivers fifteen before it starts counting drops. The alternative
//! is a sequence number per slot, which is a third atomic and a slot bigger
//! than the record it holds, to recover one record out of a ring whose whole
//! purpose is that losing records is acceptable.
//!
//! The lossless side does not pay that: [`Producer::try_push`] refuses one
//! record earlier, so the ring never reaches the lap where the question
//! arises, and a command queue that reports room really has it.
//!
//! # Padding
//!
//! Head, tail and the drop counter are written by different threads and are
//! held in `CachePadded` (ADR-0010), so a write to one does not invalidate the
//! others' cache lines. The number is per target and comes from
//! `crossbeam_utils`, not from a measurement of this laptop.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_utils::CachePadded;

/// A message that fits in a ring slot: sixteen bytes, and no heap anywhere in
/// it.
pub trait Record: Copy {
    fn encode(self) -> [u64; 2];

    /// `None` for a word pair this type does not recognise. A consumer skips
    /// those rather than panicking: the ring is shared with another thread,
    /// and a decode that can fail loudly on garbage is one that cannot take
    /// the process down with it.
    fn decode(words: [u64; 2]) -> Option<Self>
    where
        Self: Sized;
}

struct Slot {
    lo: AtomicU64,
    hi: AtomicU64,
}

struct Ring<T> {
    slots: Box<[Slot]>,
    /// Capacity minus one; the capacity is a power of two so the index is a
    /// mask rather than a division.
    mask: u64,
    /// Records ever written. Only the producer stores to it.
    head: CachePadded<AtomicU64>,
    /// Records ever read. Only the consumer stores to it.
    tail: CachePadded<AtomicU64>,
    /// Records the producer overwrote before the consumer reached them. Only
    /// the consumer stores to it, because it is the consumer that notices.
    dropped: CachePadded<AtomicU64>,
    _payload: PhantomData<fn(T) -> T>,
}

/// The ring is full and the write was refused. Carries the record back, so a
/// caller that wants to retry does not have to have kept a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full<T>(pub T);

/// Make a ring holding `capacity` records, which must be a power of two.
pub fn channel<T: Record>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    assert!(
        capacity.is_power_of_two() && capacity >= 2,
        "ring capacity must be a power of two of at least 2, not {capacity}"
    );
    let slots = (0..capacity)
        .map(|_| Slot {
            lo: AtomicU64::new(0),
            hi: AtomicU64::new(0),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let ring = Arc::new(Ring {
        slots,
        mask: capacity as u64 - 1,
        head: CachePadded::new(AtomicU64::new(0)),
        tail: CachePadded::new(AtomicU64::new(0)),
        dropped: CachePadded::new(AtomicU64::new(0)),
        _payload: PhantomData,
    });
    (
        Producer {
            ring: Arc::clone(&ring),
            _payload: PhantomData,
        },
        Consumer {
            ring,
            _payload: PhantomData,
        },
    )
}

/// The writing end. Deliberately not `Clone`: the ring is single-producer, and
/// the only enforcement of that is that there is one of these.
pub struct Producer<T> {
    ring: Arc<Ring<T>>,
    _payload: PhantomData<fn(T) -> T>,
}

impl<T: Record> Producer<T> {
    /// Write a record, overwriting the oldest unread one if the ring is full.
    /// Never blocks, never fails, never allocates.
    pub fn push(&mut self, record: T) {
        let head = self.ring.head.load(Ordering::Relaxed);
        self.write(head, record);
        // Release: the slot's contents must be visible to a consumer that
        // acquires this head.
        self.ring
            .head
            .store(head.wrapping_add(1), Ordering::Release);
    }

    /// Write a record, refusing rather than overwriting when the ring is full.
    ///
    /// Full is one short of the slot count, so that the consumer never has to
    /// decide whether the record a lap behind the head is intact — for a
    /// lossless queue the answer has to be yes.
    pub fn try_push(&mut self, record: T) -> Result<(), Full<T>> {
        let head = self.ring.head.load(Ordering::Relaxed);
        let tail = self.ring.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.ring.mask {
            return Err(Full(record));
        }
        self.write(head, record);
        self.ring
            .head
            .store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    fn write(&mut self, head: u64, record: T) {
        let [lo, hi] = record.encode();
        let slot = &self.ring.slots[(head & self.ring.mask) as usize];
        slot.lo.store(lo, Ordering::Relaxed);
        slot.hi.store(hi, Ordering::Relaxed);
    }

    pub fn capacity(&self) -> usize {
        self.ring.slots.len()
    }
}

/// The reading end. Not `Clone`, for the same reason as [`Producer`].
pub struct Consumer<T> {
    ring: Arc<Ring<T>>,
    _payload: PhantomData<fn(T) -> T>,
}

impl<T: Record> Consumer<T> {
    /// The next record, or `None` when the ring is empty.
    ///
    /// Records the producer overwrote before this got to them are skipped and
    /// added to [`Consumer::dropped`]. The loop terminates because every turn
    /// through it either returns or advances past a record the producer has
    /// already replaced.
    pub fn pop(&mut self) -> Option<T> {
        let capacity = self.ring.mask + 1;
        loop {
            let head = self.ring.head.load(Ordering::Acquire);
            let mut tail = self.ring.tail.load(Ordering::Relaxed);
            if head == tail {
                return None;
            }
            // More has been written than fits: the oldest records are gone,
            // and so is the one the producer is standing on.
            let lag = head.wrapping_sub(tail);
            let missed = if lag >= capacity {
                let missed = lag - capacity + 1;
                tail = tail.wrapping_add(missed);
                missed
            } else {
                0
            };
            let slot = &self.ring.slots[(tail & self.ring.mask) as usize];
            let words = [
                slot.lo.load(Ordering::Relaxed),
                slot.hi.load(Ordering::Relaxed),
            ];
            // Those two loads are relaxed, and an acquire *load* below would
            // not stop them being reordered after it — acquire orders what
            // follows it, not what precedes it. Without this fence the check
            // can be answered by a head from before the overwrite while the
            // record read is one from after it, which is a stale record that
            // passes for a fresh one. An acquire fence is the seqlock reader's
            // half of the pattern, and is the whole reason this is sound
            // without a sequence number per slot.
            std::sync::atomic::fence(Ordering::Acquire);
            // Did the producer reach this slot while we were copying it out?
            // It writes the slot before publishing the head above it, so a
            // head a whole lap ahead means it is in there now.
            if self.ring.head.load(Ordering::Relaxed).wrapping_sub(tail) >= capacity {
                // Try again from wherever the producer has got to. Nothing is
                // counted yet, so a retry cannot count the same loss twice.
                continue;
            }
            if missed > 0 {
                self.ring.dropped.fetch_add(missed, Ordering::Relaxed);
            }
            self.ring
                .tail
                .store(tail.wrapping_add(1), Ordering::Release);
            match T::decode(words) {
                Some(record) => return Some(record),
                // Not a record this type knows. Nothing this crate writes
                // produces one; skipping rather than trusting it is the
                // difference between a wrong reading and a panic.
                None => continue,
            }
        }
    }

    /// How many records have been overwritten before being read, since the
    /// ring was made. A consumer that reports a trace has to report this
    /// alongside it: a gappy trace presented as a whole one is a lie.
    pub fn dropped(&self) -> u64 {
        self.ring.dropped.load(Ordering::Relaxed)
    }

    /// Records written but not yet read, capped at what is still readable —
    /// anything beyond that has been overwritten and is counted by
    /// [`Consumer::dropped`] when it is reached.
    pub fn len(&self) -> usize {
        let head = self.ring.head.load(Ordering::Acquire);
        let tail = self.ring.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail).min(self.ring.mask) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.ring.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record that is just its two words, so the ring can be tested without
    /// a message type's encoding in the way.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Pair(u64, u64);

    impl Record for Pair {
        fn encode(self) -> [u64; 2] {
            [self.0, self.1]
        }
        fn decode(words: [u64; 2]) -> Option<Self> {
            // Reserve zero as the unrecognised encoding, so the skip path has
            // something to be tested with.
            (words[0] != u64::MAX).then_some(Pair(words[0], words[1]))
        }
    }

    #[test]
    fn round_trips_in_order() {
        let (mut tx, mut rx) = channel::<Pair>(4);
        for i in 0..3 {
            tx.push(Pair(i, i * 2));
        }
        assert_eq!(rx.len(), 3);
        for i in 0..3 {
            assert_eq!(rx.pop(), Some(Pair(i, i * 2)));
        }
        assert_eq!(rx.pop(), None);
        assert_eq!(rx.dropped(), 0);
    }

    #[test]
    fn overwrites_oldest_and_counts_the_loss() {
        let (mut tx, mut rx) = channel::<Pair>(4);
        for i in 0..6 {
            tx.push(Pair(i, 0));
        }
        // Three of the four slots are readable — the fourth is where the
        // producer would write next — so the first three records are gone.
        assert_eq!(rx.pop(), Some(Pair(3, 0)));
        assert_eq!(rx.dropped(), 3);
        assert_eq!(rx.pop(), Some(Pair(4, 0)));
        assert_eq!(rx.pop(), Some(Pair(5, 0)));
        assert_eq!(rx.pop(), None);
        assert_eq!(rx.dropped(), 3);
    }

    #[test]
    fn a_ring_holds_one_fewer_record_than_it_has_slots() {
        let (mut tx, mut rx) = channel::<Pair>(4);
        for i in 0..3 {
            tx.push(Pair(i, 0));
        }
        assert_eq!(rx.len(), 3);
        assert_eq!(rx.dropped(), 0);
        tx.push(Pair(3, 0));
        assert_eq!(rx.len(), 3, "the fourth slot is the producer's next");
        assert_eq!(rx.pop(), Some(Pair(1, 0)));
        assert_eq!(rx.dropped(), 1);
    }

    #[test]
    fn try_push_refuses_when_full() {
        let (mut tx, mut rx) = channel::<Pair>(4);
        assert_eq!(tx.try_push(Pair(1, 0)), Ok(()));
        assert_eq!(tx.try_push(Pair(2, 0)), Ok(()));
        assert_eq!(tx.try_push(Pair(3, 0)), Ok(()));
        assert_eq!(tx.try_push(Pair(4, 0)), Err(Full(Pair(4, 0))));
        assert_eq!(rx.pop(), Some(Pair(1, 0)));
        // Reading one makes room for one.
        assert_eq!(tx.try_push(Pair(4, 0)), Ok(()));
        assert_eq!(rx.pop(), Some(Pair(2, 0)));
        assert_eq!(rx.pop(), Some(Pair(3, 0)));
        assert_eq!(rx.pop(), Some(Pair(4, 0)));
        // Nothing was overwritten on the way: that is the guarantee.
        assert_eq!(rx.dropped(), 0);
    }

    #[test]
    fn unrecognised_records_are_skipped_not_panicked_on() {
        let (mut tx, mut rx) = channel::<Pair>(4);
        tx.push(Pair(u64::MAX, 0));
        tx.push(Pair(7, 7));
        assert_eq!(rx.pop(), Some(Pair(7, 7)));
    }

    #[test]
    fn pushing_does_not_allocate_after_construction() {
        // Not the allocator test — that one is in tests/no_alloc.rs with a
        // counting allocator installed. This one just pins the shape: the
        // slots are made once, and nothing here can grow them.
        let (tx, _rx) = channel::<Pair>(8);
        assert_eq!(tx.capacity(), 8);
    }
}
