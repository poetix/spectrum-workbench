//! The sample ring under two threads, which is the only way it is ever used.
//!
//! The unit tests in `src/ring.rs` drive both ends from one thread, which
//! proves the index arithmetic and proves nothing at all about the memory
//! ordering. This runs a producer and a consumer against each other for long
//! enough to shake out a missing `Acquire`, and asserts the property that
//! matters: what comes out is a *prefix* of what went in, in order, with no
//! sample invented, duplicated or transposed.
//!
//! A refused sample is a gap at the end of a push and never a hole in the
//! middle, so the producer advances its counter only by what was accepted and
//! the stream the consumer sees is exactly `0, 1, 2, …`. Anything else is a
//! bug, and the counter is carried in the sample value so that the assertion
//! can name which one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use rkw_audio::ring;

/// Samples to get through before the test is satisfied. Twelve seconds of
/// audio at 48 kHz, which takes well under a second of wall clock.
const TARGET: u32 = 600_000;

#[test]
fn a_producer_and_a_consumer_agree_on_every_sample() {
    let (mut tx, mut rx) = ring::channel(1_024);
    let done = Arc::new(AtomicBool::new(false));

    let producer_done = Arc::clone(&done);
    let producer = thread::spawn(move || {
        let mut sent = 0u32;
        // Uneven lumps, so that pushes land in every alignment against the
        // wrap rather than marching in step with it.
        let mut lump = 1usize;
        while sent < TARGET {
            let batch: Vec<f32> = (0..lump).map(|i| (sent + i as u32) as f32).collect();
            sent += tx.push(&batch) as u32;
            lump = lump % 373 + 1;
        }
        producer_done.store(true, Ordering::Release);
        (sent, tx.dropped())
    });

    let mut got = 0u32;
    let mut buf = [0.0f32; 512];
    let mut asked = 1usize;
    loop {
        let n = rx.pop(&mut buf[..asked]);
        for &sample in &buf[..n] {
            assert_eq!(
                sample, got as f32,
                "sample {got} came back as {sample}: the stream is not in order"
            );
            got += 1;
        }
        asked = asked % 512 + 1;

        if n == 0 && done.load(Ordering::Acquire) && rx.is_empty() {
            break;
        }
    }

    let (sent, dropped) = producer.join().expect("producer panicked");
    assert_eq!(got, sent, "the consumer saw {got} of the {sent} accepted");
    assert!(sent >= TARGET, "the producer stopped early at {sent}");
    // The ring is a thousandth of the traffic, so it must have filled and
    // refused; a run that never dropped anything did not exercise the case
    // this test exists for.
    assert!(dropped > 0, "the ring never filled, so nothing was tested");
}

#[test]
fn a_consumer_that_outruns_the_producer_is_starved_and_not_confused() {
    let (mut tx, mut rx) = ring::channel(64);
    let done = Arc::new(AtomicBool::new(false));

    // A producer far slower than the consumer, so the ring is empty most of
    // the time and almost every pop is short.
    let producer_done = Arc::clone(&done);
    let producer = thread::spawn(move || {
        let mut sent = 0u32;
        while sent < 20_000 {
            let batch = [sent as f32];
            sent += tx.push(&batch) as u32;
            std::hint::spin_loop();
        }
        producer_done.store(true, Ordering::Release);
        sent
    });

    let mut got = 0u32;
    let mut buf = [0.0f32; 128];
    loop {
        let n = rx.pop(&mut buf);
        for &sample in &buf[..n] {
            assert_eq!(sample, got as f32, "sample {got} out of order under starvation");
            got += 1;
        }
        if n == 0 && done.load(Ordering::Acquire) && rx.is_empty() {
            break;
        }
    }

    let sent = producer.join().expect("producer panicked");
    assert_eq!(got, sent);
    assert!(rx.starved() > 0, "the consumer should have run dry repeatedly");
}
