#!/usr/bin/env bash
#
# Fetch third-party CPU conformance test data.
#
# These files are not vendored into the repository because they come from
# projects under different licences to this one (Fuse is GPL-2.0-or-later).
# They are test *inputs*, not code, and nothing in rkw links against them, but
# keeping them out of the tree keeps the licensing question from arising at
# all. The tests that use them skip with a message when they are absent.
#
# Usage: scripts/fetch-testdata.sh

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$root/crates/z80/tests/fixtures"

fuse_base="https://raw.githubusercontent.com/FuseEmulator/fuse-emulator-svn/master/fuse/z80/tests"
zex_base="https://raw.githubusercontent.com/anotherlin/z80emu/master/testfiles"

mkdir -p "$fixtures/fuse" "$fixtures/zex"

for f in tests.in tests.expected; do
    echo "fetching $f"
    curl -fsSL --retry 3 -o "$fixtures/fuse/$f" "$fuse_base/$f"
done

# Frank Cringle's exerciser, as CP/M .com images. zexdoc checks the documented
# flags only; zexall additionally checks the undocumented bits 3 and 5.
for f in zexdoc.com zexall.com; do
    echo "fetching $f"
    curl -fsSL --retry 3 -o "$fixtures/zex/$f" "$zex_base/$f"
done

echo
echo "Test data installed in $fixtures"
echo "  fuse: $(grep -c '' "$fixtures/fuse/tests.in") lines in, \
$(grep -c '' "$fixtures/fuse/tests.expected") lines expected"
echo "  zex:  $(wc -c < "$fixtures/zex/zexdoc.com") / \
$(wc -c < "$fixtures/zex/zexall.com") bytes"
echo
echo "Run with:"
echo "  cargo test --test fuse"
echo "  cargo test --release --test zex -- --ignored --nocapture"
