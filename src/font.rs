//! The debugger's typeface: 128 characters, eight bytes each.

/// A glyph is a square of pixels, GLYPH_SIZE on a side: one byte per row,
/// one bit per pixel, bit 0 the leftmost.
pub const GLYPH_SIZE: usize = 8;

/// How many characters the file holds: the ASCII range, code 0 to 127.
const GLYPHS: usize = 128;

/// A typeface read from `font8x8.bin`.
pub struct Font {
    /// Every glyph in code order: 'A' is code 65, so its rows start at
    /// byte 65 * 8.
    glyphs: Vec<u8>,
}

impl Font {
    /// Read the typeface from a file. A file of the wrong size is refused:
    /// a half-downloaded font would print garbage without a word of
    /// complaint, and garbage in a debugger is worse than no text at all.
    pub fn load(path: &str) -> Result<Font, String> {
        let glyphs =
            std::fs::read(path).map_err(|why| format!("could not read {path}: {why}"))?;
        Font::from_bytes(glyphs)
    }

    /// A typeface from bytes already in hand — the way the tests build one.
    pub fn from_bytes(glyphs: Vec<u8>) -> Result<Font, String> {
        let expected = GLYPHS * GLYPH_SIZE;
        if glyphs.len() != expected {
            return Err(format!("a font is {expected} bytes, this one is {}", glyphs.len()));
        }
        Ok(Font { glyphs })
    }

    /// The rows of one character. Anything beyond ASCII — an accent, an
    /// emoji — prints as '?', which is at least honest about it.
    pub fn glyph(&self, c: char) -> &[u8] {
        let code = if (c as usize) < GLYPHS { c as usize } else { b'?' as usize };
        &self.glyphs[code * GLYPH_SIZE..(code + 1) * GLYPH_SIZE]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typeface with one recognizable glyph — a solid block at 'A' —
    /// and nothing anywhere else.
    fn block_at_a() -> Font {
        let mut bytes = vec![0u8; GLYPHS * GLYPH_SIZE];
        let a = b'A' as usize * GLYPH_SIZE;
        bytes[a..a + GLYPH_SIZE].fill(0xFF);
        Font::from_bytes(bytes).unwrap()
    }

    #[test]
    fn a_glyph_is_its_eight_rows() {
        let font = block_at_a();
        assert_eq!(font.glyph('A'), &[0xFF; 8]);
        assert_eq!(font.glyph('B'), &[0x00; 8]);
    }

    #[test]
    fn characters_beyond_ascii_print_as_a_question_mark() {
        let mut bytes = vec![0u8; GLYPHS * GLYPH_SIZE];
        bytes[b'?' as usize * GLYPH_SIZE] = 0x3C;
        let font = Font::from_bytes(bytes).unwrap();
        assert_eq!(font.glyph('é')[0], 0x3C);
    }

    #[test]
    fn a_font_of_the_wrong_size_is_refused() {
        assert!(Font::from_bytes(vec![0; 1000]).is_err());
    }
}