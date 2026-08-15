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
# raxoft's suite is a Spectrum program on a tape, so it belongs with the other
# things that need a machine around them rather than with the CPU's own data.
tapes="$root/crates/rkw-spectrum/tests/fixtures/z80test"

fuse_base="https://raw.githubusercontent.com/FuseEmulator/fuse-emulator-svn/master/fuse/z80/tests"
zex_base="https://raw.githubusercontent.com/anotherlin/z80emu/master/testfiles"
z80test_url="https://github.com/raxoft/z80test/releases/download/v1.2a/z80test-1.2a.zip"

mkdir -p "$fixtures/fuse" "$fixtures/zex" "$tapes"

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

# raxoft's z80test, which is distributed as a release archive: the repository
# itself carries only the sjasmplus sources, and assembling them is a different
# exercise (ticket 0031) to running them. The archive is unpacked and thrown
# away rather than kept, so what lands in the tree is five tapes and no zip.
echo "fetching z80test-1.2a.zip"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp/z80test.zip" "$z80test_url"

# Checked rather than trusted, for the same reason the ROM's hash is: a
# different release of the suite has different expected CRCs baked into it, and
# a test that loaded one and reported against the other would be lying twice.
want_zip_sha256="7df0443d703e6b3114ea04b4cdef3e13b91421c62e37185c1036d06864cacbaf"
got="$(shasum -a 256 "$tmp/z80test.zip" | cut -d' ' -f1)"
if [ "$got" != "$want_zip_sha256" ]; then
    echo "z80test-1.2a.zip hashes to $got, expected $want_zip_sha256" >&2
    exit 1
fi

unzip -q -o -j "$tmp/z80test.zip" '*.tap' -d "$tapes"

echo
echo "Test data installed in $fixtures"
echo "  z80test: $(ls "$tapes" | tr '\n' ' ')"
echo "  fuse: $(grep -c '' "$fixtures/fuse/tests.in") lines in, \
$(grep -c '' "$fixtures/fuse/tests.expected") lines expected"
echo "  zex:  $(wc -c < "$fixtures/zex/zexdoc.com") / \
$(wc -c < "$fixtures/zex/zexall.com") bytes"
echo
echo "Run with:"
echo "  cargo test --test fuse"
echo "  cargo test --release --test zex -- --ignored --nocapture"
echo "  cargo test -p rkw-spectrum --test z80test -- --ignored --nocapture"
