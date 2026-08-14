#!/usr/bin/env bash
#
# Fetch the 48K Spectrum ROM.
#
# The ROM is Amstrad's, not ours, and it is not vendored into the repository for
# the same reason the conformance data is not (ADR-0005): it is a test *input*,
# nothing links against it, and keeping it out of the tree keeps the licensing
# question from arising. The tests that need it skip with a message when it is
# absent.
#
# Usage: scripts/fetch-rom.sh
#
# A ROM you already have works just as well: copy it to the path printed below,
# or point $RKW_48K_ROM at it.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="$root/crates/rkw-spectrum/tests/fixtures"
rom="$fixtures/48.rom"

# Fuse's copy, from the same mirror the CPU test data comes from.
url="https://raw.githubusercontent.com/FuseEmulator/fuse-emulator-svn/master/fuse/roms/48.rom"

# The 1982 ROM, the one every reference and every other emulator means by
# "48.rom". Checked rather than trusted: a 128K ROM pair, a Spanish 48K ROM or a
# diagnostic ROM would all load and boot to something subtly different, and the
# framebuffer hash in tests/boot.rs is only meaningful for this image.
want_sha256="d55daa439b673b0e3f5897f99ac37ecb45f974d1862b4dadb85dec34af99cb42"
want_size=16384

mkdir -p "$fixtures"
echo "fetching 48.rom"
curl -fsSL --retry 3 -o "$rom" "$url"

size="$(wc -c < "$rom" | tr -d ' ')"
if [ "$size" != "$want_size" ]; then
    echo "48.rom is $size bytes; the 48K map has room for $want_size" >&2
    exit 1
fi

if command -v shasum > /dev/null; then
    got="$(shasum -a 256 "$rom" | cut -d' ' -f1)"
elif command -v sha256sum > /dev/null; then
    got="$(sha256sum "$rom" | cut -d' ' -f1)"
else
    got="$want_sha256"
    echo "warning: no shasum or sha256sum; not verifying the image" >&2
fi
if [ "$got" != "$want_sha256" ]; then
    echo "48.rom is not the image these tests expect" >&2
    echo "  got  $got" >&2
    echo "  want $want_sha256" >&2
    exit 1
fi

echo
echo "ROM installed at $rom"
echo
echo "Run with:"
echo "  cargo test -p rkw-spectrum --test boot"
echo "  cargo run -p rkw-cli -- --rom $rom"
