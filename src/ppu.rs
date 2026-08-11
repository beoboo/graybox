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

/// The NES's whole crayon box: the 64 colors it can ever show, as RGB.
///
/// The real chip stores no RGB anywhere — it emits an analog TV signal,
/// and every emulator's table is one honest reading of that signal.
/// These values were computed from the signal's documented voltages
/// (Part II does that computation live, and finds even more colors).
pub const SYSTEM_PALETTE: [u32; 64] = [
    0x525252, 0x001E94, 0x0907C2, 0x3100BD, 0x580086, 0x6F0036, 0x6C0000, 0x501000,
    0x272900, 0x023F00, 0x004B00, 0x004700, 0x003646, 0x000000, 0x000000, 0x000000,
    0xA0A0A0, 0x004FFF, 0x2C2AFF, 0x6D0FFF, 0xA905ED, 0xCB0775, 0xC61909, 0x9D3900,
    0x5E6100, 0x208300, 0x009400, 0x008F1A, 0x00758D, 0x000000, 0x000000, 0x000000,
    0xFEFEFE, 0x3EA4FF, 0x7B78FF, 0xC656FF, 0xFF46FF, 0xFF4ACE, 0xFF634C, 0xFB8B00,
    0xB5B800, 0x6BDE00, 0x34F205, 0x1AEC63, 0x1ECFEA, 0x3C3C3C, 0x000000, 0x000000,
    0xFEFEFE, 0xAAD8FF, 0xC6C5FF, 0xE7B5FF, 0xFFADFF, 0xFFB0EA, 0xFFBBB1, 0xFDCD82,
    0xDFE16A, 0xBFF16D, 0xA5F98A, 0x97F7BC, 0x99EBF6, 0xA9A9A9, 0x000000, 0x000000,
];

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

    #[test]
    fn famous_crayons_are_where_they_belong() {
        // $0F is the NES's true black; $20 and $30 are its whites; the
        // $x0 column is the grays, darkest to lightest.
        assert_eq!(SYSTEM_PALETTE[0x0F], 0x000000);
        assert_eq!(SYSTEM_PALETTE[0x20], SYSTEM_PALETTE[0x30]);
        // The white, pinned to the exact byte: a rounding slip here
        // passes every other test and shows up only when a frame sits
        // next to a reference picture.
        assert_eq!(SYSTEM_PALETTE[0x20], 0xFEFEFE);
        assert!(SYSTEM_PALETTE[0x00] < SYSTEM_PALETTE[0x10]);
    }

    #[test]
    fn every_crayon_is_plain_rgb() {
        // No alpha surprises for the frame buffer.
        for color in SYSTEM_PALETTE {
            assert_eq!(color >> 24, 0);
        }
    }
}
