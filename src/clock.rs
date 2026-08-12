//! The clock — the metronome every chip in the machine marches to.

/// Where the beam is, counted the way the hardware counts: in dots.
/// 341 dots make a scanline, 262 scanlines make a frame, and the CPU
/// ticks once for every three dots.
pub struct Clock {
    /// 0-340 across the current scanline.
    pub dot: u16,

    /// 0-261 down the current frame: 0-239 are drawn, 240 is a rest,
    /// 241-260 are vblank, and 261 is the warm-up lap before the
    /// next picture.
    pub scanline: u16,

    /// Finished frames since power-on.
    pub frame: u64,
}

impl Clock {
    /// A clock at the top-left corner of frame zero.
    pub fn new() -> Clock {
        Clock { dot: 0, scanline: 0, frame: 0 }
    }

    /// One dot forward — wrapping dot into scanline into frame.
    pub fn tick(&mut self) {
        self.dot += 1;

        if self.dot == 341 {
            self.dot = 0;
            self.scanline += 1;

            if self.scanline == 262 {
                self.scanline = 0;
                self.frame += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scanline_is_341_dots() {
        let mut clock = Clock::new();
        for _ in 0..341 {
            clock.tick();
        }
        assert_eq!((clock.scanline, clock.dot), (1, 0));
    }

    #[test]
    fn a_frame_is_262_scanlines() {
        let mut clock = Clock::new();
        for _ in 0..341 * 262 {
            clock.tick();
        }
        assert_eq!((clock.frame, clock.scanline, clock.dot), (1, 0, 0));
    }
}
