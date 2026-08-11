//! The controller — eight buttons on one wire, read a bit at a time.

use std::cell::Cell;

/// One NES controller. All eight buttons fit in one byte, in the
/// order the console shifts them out: A, B, Select, Start, Up, Down,
/// Left, Right — A in bit 0, Right in bit 7.
pub struct Controller {
    /// What the player is holding at this instant. The window loop
    /// keeps this fresh; the game never sees it directly.
    pub buttons: u8,

    /// The frozen copy the game actually reads, taken the moment the
    /// strobe drops.
    snapshot: u8,

    /// The "stare at the buttons" line. High: the pad watches the
    /// buttons live. Dropped: the snapshot freezes.
    strobe: bool,

    /// Which bit of the snapshot the next read hands out. A `Cell`,
    /// for the same reason as the PPU's address: reading moves it.
    index: Cell<u8>,
}

impl Controller {
    pub fn new() -> Controller {
        Controller {
            buttons: 0,
            snapshot: 0,
            strobe: false,
            index: Cell::new(0),
        }
    }

    /// A write to $4016. Only bit 0 matters: raising it makes the pad
    /// stare at the buttons; dropping it freezes what it saw and
    /// rewinds to the first button.
    pub fn write(&mut self, value: u8) {
        let high = value & 1 != 0;
        if self.strobe && !high {
            self.snapshot = self.buttons;
            self.index.set(0);
        }
        self.strobe = high;
    }

    /// A read from $4016: the next button of the snapshot, low bit of
    /// the answer. While the strobe is high there IS no "next" —
    /// every read reports A, live. And past the eighth button a real
    /// pad answers 1, which is how games tell a controller from an
    /// empty port.
    pub fn read(&self) -> u8 {
        if self.strobe {
            return self.buttons & 1;
        }
        let index = self.index.get();
        if index < 8 {
            self.index.set(index + 1);
            (self.snapshot >> index) & 1
        } else {
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A controller holding `buttons`, strobed the way games do it:
    /// stare, then freeze.
    fn frozen(buttons: u8) -> Controller {
        let mut controller = Controller::new();
        controller.buttons = buttons;
        controller.write(1);
        controller.write(0);
        controller
    }

    #[test]
    fn buttons_come_out_one_bit_at_a_time_a_first() {
        // Start is bit 3, Right is bit 7.
        let controller = frozen(0b1000_1000);
        let bits: Vec<u8> = (0..8).map(|_| controller.read()).collect();
        assert_eq!(bits, [0, 0, 0, 1, 0, 0, 0, 1]);
    }

    #[test]
    fn past_the_eighth_button_a_real_pad_answers_one() {
        let controller = frozen(0);
        for _ in 0..8 {
            controller.read();
        }
        assert_eq!(controller.read(), 1);
        assert_eq!(controller.read(), 1);
    }

    #[test]
    fn the_snapshot_freezes_when_the_strobe_drops() {
        let mut controller = frozen(0b0000_0001);
        // Released after the freeze — too late, the read still says A.
        controller.buttons = 0;
        assert_eq!(controller.read(), 1);
    }

    #[test]
    fn while_the_strobe_is_high_every_read_reports_a() {
        let mut controller = Controller::new();
        controller.buttons = 0b0000_0001;
        controller.write(1);
        assert_eq!(controller.read(), 1);
        // Still A. Staring is not stepping.
        assert_eq!(controller.read(), 1);
    }

    #[test]
    fn a_fresh_strobe_rewinds_to_the_first_button() {
        let mut controller = frozen(0b0000_0010);
        controller.read();
        controller.read();
        controller.write(1);
        controller.write(0);
        assert_eq!(controller.read(), 0); // A again...
        assert_eq!(controller.read(), 1); // ...then B.
    }
}
