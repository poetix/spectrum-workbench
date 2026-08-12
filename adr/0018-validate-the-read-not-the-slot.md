---
id: "ADR-0018"
title: Validate the read, not the slot
date: 2026-08-12
status: accepted
---

## Context

ADR-0007's event ring is single-producer, single-consumer, lossy and
non-blocking: the producer writes a slot and publishes an incremented index,
and it never reads the consumer's index, because a producer that waited for a
consumer would be a UI stalling the emulator. That leaves the consumer to
establish for itself that the record it copied out of a slot was not being
overwritten while it copied it.

The textbook answer is a sequence number per slot — a seqlock, where the writer
marks the slot odd, writes, and marks it even, and the reader retries if the
number changed. It is correct and it is well understood. It also means a slot
is no longer sixteen bytes: it is sixteen bytes plus a counter, plus the two
extra atomic operations per record on the producer's side, which is the side
that must not be slowed down.

The workspace forbids `unsafe_code`, so the alternative of publishing a
`[u8; 16]` behind a lock-free pointer swap is not available either.

## Decision

The slot carries no sequence number. The record is the two atomic words, and
the consumer validates the *read* rather than the slot:

1. Load the producer index with acquire.
2. If the producer is a whole lap or more ahead, skip forward, counting what
   was skipped as dropped.
3. Copy the two words out with relaxed loads.
4. **Acquire fence.**
5. Re-load the producer index. If it is now a whole lap or more ahead of the
   record just read, the producer was in that slot; discard and retry.

The fence in step 4 is the decision. An acquire *load* orders what follows it,
and what needs ordering here is what precedes it: without a fence, the relaxed
reads of the record are free to be answered after the re-read of the index, so
a stale record passes the very check that exists to catch it.

A record is treated as lost while the producer is a whole lap ahead of it,
rather than a lap and one record ahead, because the producer writes the slot
before publishing the index above it. A ring of sixteen slots therefore
delivers fifteen records before it starts counting drops. The lossless command
ring pays nothing for this: `try_push` refuses one record earlier, so that ring
never reaches the lap where the question arises.

## Consequences

**Positive:**
- A slot is exactly the record: sixteen bytes, no metadata, no third atomic.
- The producer's cost is unchanged — two relaxed stores and a release store —
  and it is the producer that is on the emulation thread.
- No `unsafe`, so the workspace-wide `forbid` stands.

**Negative:**
- The reasoning is subtle and the failure mode is silent: a missing fence
  yields records that are merely *stale*, which look entirely valid. The
  two-threaded test in `crates/rkw-debug/tests/ring.rs` is what makes it
  non-silent — every record carries its index twice so that a torn one is
  detectable, and the consumer asserts that what it reads is in order. It
  reproduced the missing-fence bug within a hundred records on an M3.
- One slot of pessimism on the lossy ring, and one on the lossless one. At the
  capacities involved this is not worth a sentence anywhere but here.
- A consumer racing a fast producer can retry several times before it gets a
  record out. It cannot livelock — the producer's progress is what invalidates
  the read, and the skip-forward that follows uses the newer index — but it is
  a spin, and it belongs on the consumer's thread rather than the emulator's.
