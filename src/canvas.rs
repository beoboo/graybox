//! The picture the window shows: the game's frame alone, or the game
//! beside the panels that watch it.

use crate::font::{Font, GLYPH_SIZE};

/// A rectangle of pixels, one `u32` each, in the window's own
/// 0x00RRGGBB.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl Canvas {
    /// A black canvas of the given size.
    pub fn new(width: usize, height: usize) -> Canvas {
        Canvas { width, height, pixels: vec![0; width * height] }
    }

    /// Paint one pixel. Off the edge is quietly ignored, so a panel
    /// drawn near the border needs no bounds arithmetic of its own.
    pub fn pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = color;
        }
    }

    /// Fill a rectangle.
    pub fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, color: u32) {
        for row in y..y + height {
            for column in x..x + width {
                self.pixel(column, row, color);
            }
        }
    }

    /// Copy a picture in, its top-left corner at (x, y). The source is
    /// `width` pixels wide and as tall as its length allows.
    pub fn blit(&mut self, x: usize, y: usize, width: usize, source: &[u32]) {
        for (index, &color) in source.iter().enumerate() {
            self.pixel(x + index % width, y + index / width, color);
        }
    }

    /// Print a string, one glyph per character, starting at (x, y). Only
    /// the lit bits are painted, so the text sits on whatever was there.
    pub fn text(&mut self, font: &Font, x: usize, y: usize, text: &str, color: u32) {
        for (column, c) in text.chars().enumerate() {
            for (row, &bits) in font.glyph(c).iter().enumerate() {
                for bit in 0..GLYPH_SIZE {
                    if bits & (1 << bit) != 0 {
                        self.pixel(x + column * GLYPH_SIZE + bit, y + row, color);
                    }
                }
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typeface whose 'A' is a single lit pixel in the top-left corner.
    fn dot_at_a() -> Font {
        let mut bytes = vec![0u8; 128 * GLYPH_SIZE];
        bytes[b'A' as usize * GLYPH_SIZE] = 0b0000_0001;
        Font::from_bytes(bytes).unwrap()
    }

    #[test]
    fn text_advances_one_glyph_per_character() {
        let mut canvas = Canvas::new(32, 8);
        canvas.text(&dot_at_a(), 4, 2, "AA", 0xFF_FFFF);
        assert_eq!(canvas.pixels[2 * 32 + 4], 0xFF_FFFF);
        assert_eq!(canvas.pixels[2 * 32 + 12], 0xFF_FFFF);
        assert_eq!(canvas.pixels.iter().filter(|&&p| p != 0).count(), 2);
    }

    #[test]
    fn a_blit_lands_where_it_is_told() {
        let mut canvas = Canvas::new(4, 4);
        canvas.blit(1, 2, 2, &[1, 2, 3, 4]);
        assert_eq!(canvas.pixels[2 * 4 + 1], 1);
        assert_eq!(canvas.pixels[2 * 4 + 2], 2);
        assert_eq!(canvas.pixels[3 * 4 + 1], 3);
        assert_eq!(canvas.pixels[3 * 4 + 2], 4);
    }

    #[test]
    fn painting_off_the_edge_is_ignored() {
        let mut canvas = Canvas::new(2, 2);
        canvas.pixel(2, 0, 7);
        canvas.fill(1, 1, 5, 5, 9);
        assert_eq!(canvas.pixels, [0, 0, 0, 9]);
    }
}