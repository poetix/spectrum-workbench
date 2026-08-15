//! A PNG encoder in a hundred lines, so that looking at a frame costs no
//! dependency.
//!
//! The compressed stream is deflate *stored* blocks: legal, universally
//! readable, and about 1.02x the size of the raw pixels. Nothing here is on a
//! path that runs more than a few dozen times, and a real compressor would be
//! a dependency to make a screenshot smaller than a screenshot needs to be.

/// An 8-bit RGB PNG. `rgb` is `width * height * 3` bytes, row-major.
pub fn encode_rgb(width: usize, height: usize, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgb.len(),
        width * height * 3,
        "pixels do not fill the image"
    );

    let mut out = Vec::with_capacity(rgb.len() + rgb.len() / 64 + 1024);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, truecolour, no interlace
    chunk(&mut out, b"IHDR", &ihdr);

    // Each row is prefixed with its filter type, which is 0: none.
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for row in rgb.chunks_exact(width * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));

    chunk(&mut out, b"IEND", &[]);
    out
}

/// Length, type, payload, CRC of the type and payload.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(payload);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// A zlib stream of stored deflate blocks: the two-byte header, the blocks,
/// and the Adler-32 of what went in.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 16);
    out.extend_from_slice(&[0x78, 0x01]); // deflate, 32K window, no preset dict

    let mut blocks = data.chunks(0xFFFF).peekable();
    // An empty input still needs one block, and it has to be the final one.
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        out.push(u8::from(last));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the most bytes that can be summed before b can overflow.
    for block in data.chunks(5552) {
        for &byte in block {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

struct Crc(u32);

impl Crc {
    fn new() -> Crc {
        Crc(0xFFFF_FFFF)
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let mut c = (self.0 ^ u32::from(byte)) & 0xFF;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            self.0 = c ^ (self.0 >> 8);
        }
    }

    fn finish(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_pixel_image_has_the_signature_and_the_four_chunks() {
        let png = encode_rgb(1, 1, &[0x12, 0x34, 0x56]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn adler_and_crc_agree_with_their_specifications() {
        // Both are worked examples that can be checked by hand or against zlib.
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        let mut crc = Crc::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
    }

    #[test]
    fn the_stored_stream_holds_every_byte_it_was_given() {
        // Two blocks' worth, so the boundary is exercised.
        let data: Vec<u8> = (0..70_000u32).map(|i| i as u8).collect();
        let stream = zlib_stored(&data);
        // Header, then per block a 5-byte header and the block itself.
        assert_eq!(stream.len(), 2 + 5 + 0xFFFF + 5 + (70_000 - 0xFFFF) + 4);
        assert_eq!(&stream[7..7 + 8], &data[..8]);
    }
}
