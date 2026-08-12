//! The event ring with a real thread on each end.
//!
//! The unit tests in `src/ring.rs` check the policies — overwrite the oldest,
//! refuse when full, count what was lost — with one thread doing both jobs,
//! which is the only way to make the interesting cases happen on demand. What
//! they cannot check is the thing the ring exists for: that a producer running
//! flat out and a consumer that cannot keep up leave the consumer with whole
//! records and an honest count of the ones it missed.
//!
//! Every record carries its own index twice over, so a record assembled from
//! two different writes is detectable rather than merely unlikely.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use rkw_debug::event::Event;
use rkw_debug::ring;

const RECORDS: u64 = 500_000;

/// A record whose two words agree with each other, so tearing is visible: the
/// T-state and the address are the same number.
fn record(i: u64) -> Event {
    Event::Exec { t: i, pc: i as u16 }
}

#[test]
fn a_flat_out_producer_and_a_slow_consumer_agree_about_what_was_lost() {
    // Small enough that the consumer cannot possibly keep up, so the drop
    // path is what is being exercised.
    let (mut tx, mut rx) = ring::channel::<Event>(16);
    let done = Arc::new(AtomicBool::new(false));

    let producer = {
        let done = Arc::clone(&done);
        thread::spawn(move || {
            for i in 0..RECORDS {
                tx.push(record(i));
            }
            done.store(true, Ordering::Release);
        })
    };

    let mut received = 0u64;
    let mut last = None;
    loop {
        match rx.pop() {
            Some(Event::Exec { t, pc }) => {
                assert_eq!(
                    pc, t as u16,
                    "a record was assembled from two different writes"
                );
                if let Some(last) = last {
                    assert!(t > last, "records came back out of order: {last} then {t}");
                }
                last = Some(t);
                received += 1;
            }
            Some(other) => panic!("something else got into the ring: {other:?}"),
            // Nothing waiting. If the producer has finished and the ring is
            // still empty, so are we.
            None if done.load(Ordering::Acquire) && rx.is_empty() => break,
            None => thread::yield_now(),
        }
    }
    producer.join().expect("the producer panicked");

    assert_eq!(
        received + rx.dropped(),
        RECORDS,
        "every record was either read or counted as dropped"
    );
    assert_eq!(last, Some(RECORDS - 1), "the last record is never dropped");
}

#[test]
fn a_consumer_that_keeps_up_loses_nothing() {
    // The same run with room to work in: a ring sized for the burst drops
    // nothing, which is what makes the drop count worth reporting rather than
    // a number that is always non-zero.
    let (mut tx, mut rx) = ring::channel::<Event>(1 << 16);
    let sent = 1000;
    let producer = thread::spawn(move || {
        for i in 0..sent {
            tx.push(record(i));
        }
    });
    producer.join().expect("the producer panicked");

    let mut received = 0;
    while let Some(event) = rx.pop() {
        assert_eq!(event, record(received));
        received += 1;
    }
    assert_eq!(received, sent);
    assert_eq!(rx.dropped(), 0);
}
