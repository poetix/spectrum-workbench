---
id: "ADR-0005"
title: Fetch third-party conformance data rather than vendoring it
date: 2026-08-11
status: accepted
---

## Context

The CPU core is validated against two external suites: the Fuse Z80 test suite
and Frank Cringle's `zexdoc`/`zexall` exercisers. Both are excellent and
neither is ours.

Fuse is GPL-2.0-or-later; `zexall` is GPL-2.0. This project is MIT OR
Apache-2.0. The files in question are test *inputs* rather than code, nothing
links against them, and they are widely redistributed — but vendoring them
puts GPL material in the repository and creates a licensing question that does
not otherwise exist.

The same will apply to the 48K ROM, which is not redistributable at all.

## Decision

Do not vendor. `scripts/fetch-testdata.sh` downloads the files into a
gitignored `crates/z80/tests/fixtures/`, and the tests that need them skip with
an explanatory message when they are absent.

The README documents what is fetched and under what licence.

## Consequences

**Positive:**
- The repository is unambiguously MIT/Apache-2.0 with no GPL content.
- `cargo test` works on a fresh clone; the conformance tests skip rather than
  fail.
- The same mechanism handles the ROM later, where there is no alternative.

**Negative:**
- Tests are not hermetic. A fresh CI run needs network access, and an upstream
  repository moving or disappearing breaks the fetch.
- A skipped test is easy not to notice. The suites print a clear message, but
  nothing fails if they never run.

**Mitigation if this becomes a problem:** pin the upstream commit rather than
tracking `master`, and have CI assert that the fixtures were present rather
than accepting a skip.
