//! The picture chip — built here, piece by piece, starting with the
//! part every picture is made of: tiles.

/// Decode one tile: 16 bytes of CHR become 8x8 pixels of 0..=3.
///
/// A tile's first 8 bytes are plane 0 (every pixel's low bit, one byte
/// per row), its second 8 are plane 1 (the high bits). Bit 7 is the
/// leftmost pixel.
pub fn decode_tile(chr: &[u8], tile: usize) -> [[u8; 8]; 8] {
    let start = tile * 16;
    let mut pixels = [[0u8; 8]; 8];

    for row in 0..8 {
        let plane0 = chr[start + row];
        let plane1 = chr[start + row + 8];

        for col in 0..8 {
            let bit = 7 - col;
            let low = (plane0 >> bit) & 1;
            let high = (plane1 >> bit) & 1;
            pixels[row][col] = (high << 1) | low;
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_drawn_row_decodes() {
        // Row 0: plane 0 = $55, plane 1 = $33 — reading the bits down
        // the columns should give 0,1,2,3 repeating.
        let mut chr = vec![0u8; 16];
        chr[0] = 0x55;
        chr[8] = 0x33;

        let pixels = decode_tile(&chr, 0);
        assert_eq!(pixels[0], [0, 1, 2, 3, 0, 1, 2, 3]);
        assert_eq!(pixels[1], [0; 8]); // every other row stayed blank
    }

    #[test]
    fn tiles_are_sixteen_bytes_apart() {
        // A single low bit at the start of tile 1, nowhere near tile 0.
        let mut chr = vec![0u8; 32];
        chr[16] = 0b1000_0000;

        let pixels = decode_tile(&chr, 1);
        assert_eq!(pixels[0][0], 1);
        assert_eq!(decode_tile(&chr, 0)[0][0], 0);
    }
}
