//! The APU — the Audio Processing Unit. Five voices on the real chip;
//! here are the two that carry every melody: the pulse channels.

/// The CPU's clock in hertz, which is also the sound chip's metronome:
/// pitch on this console is measured in CPU cycles per wobble.
const CPU_HZ: f32 = 1_789_773.0;

/// The four shapes a pulse can sing, as one repeating cycle of eight
/// steps. Row 2 is the fat square — up half the time, the classic
/// video-game voice. Row 0 is a thin reedy spike. Row 3 is row 1
/// upside down, which ears can barely tell apart.
const DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];

/// One pulse voice: a shape, a loudness, a pitch — and where in its
/// cycle it currently stands.
pub struct Pulse {
    /// Which DUTY row to sing.
    duty: usize,

    /// Loudness, 0 to 15, straight from the register.
    volume: u8,

    /// The 11-bit countdown that sets the pitch: the chip walks one
    /// step of the shape every 2 x (timer + 1) CPU cycles.
    timer: u16,

    /// Where in the eight-step cycle the voice is, as a fraction
    /// 0 to 1 — the one field that moves while a note is held.
    phase: f32,
}

impl Pulse {
    fn new() -> Pulse {
        Pulse {
            duty: 0,
            volume: 0,
            timer: 0,
            phase: 0.0,
        }
    }

    /// $4000 / $4004 — shape and loudness. (The middle bits select
    /// the envelope and length-counter behaviors; both of our games
    /// keep them pinned to "constant volume, never cut me off", so
    /// constant volume is what we build.)
    fn write_control(&mut self, value: u8) {
        self.duty = (value >> 6) as usize;
        self.volume = value & 0x0F;
    }

    /// $4002 / $4006 — the timer's low eight bits. Melodies ride this
    /// register: small nudges here bend the pitch between notes.
    fn write_timer_low(&mut self, value: u8) {
        self.timer = (self.timer & 0x0700) | value as u16;
    }

    /// $4003 / $4007 — the timer's top three bits. Writing here also
    /// restarts the shape from step zero, so games save this register
    /// for genuine pitch jumps.
    fn write_timer_high(&mut self, value: u8) {
        self.timer = (self.timer & 0x00FF) | (((value & 0x07) as u16) << 8);
        self.phase = 0.0;
    }

    /// The note being sung, in hertz: sixteen timer-loads of the
    /// timer per full eight-step cycle.
    fn frequency(&self) -> f32 {
        CPU_HZ / (16.0 * (self.timer as f32 + 1.0))
    }

    /// Sing one sample: the volume, or nothing, says the shape. A
    /// timer under 8 would whistle above 14 kHz — the real chip mutes
    /// it there, and so do we.
    fn sample(&mut self, sample_rate: f32) -> u8 {
        if self.volume == 0 || self.timer < 8 {
            return 0;
        }

        let step = (self.phase * 8.0) as usize & 7;
        self.phase = (self.phase + self.frequency() / sample_rate).fract();
        DUTY[self.duty][step] * self.volume
    }
}

/// The sound chip: its two pulse voices and the on/off switchboard.
pub struct Apu {
    pulse1: Pulse,
    pulse2: Pulse,

    /// $4015 — one enable bit per voice; bits 0 and 1 are the pulses.
    enabled: u8,
}

impl Apu {
    pub fn new() -> Apu {
        Apu {
            pulse1: Pulse::new(),
            pulse2: Pulse::new(),
            enabled: 0,
        }
    }

    /// A write to any sound register. The two pulses answer; the
    /// other three voices — triangle bass, noise drums, sample
    /// playback — are Part II's, and their registers fall through
    /// politely until then.
    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0x4000 => self.pulse1.write_control(value),
            0x4002 => self.pulse1.write_timer_low(value),
            0x4003 => self.pulse1.write_timer_high(value),
            0x4004 => self.pulse2.write_control(value),
            0x4006 => self.pulse2.write_timer_low(value),
            0x4007 => self.pulse2.write_timer_high(value),
            0x4015 => self.enabled = value,
            _ => {}
        }
    }

    /// One sample of the chip's output: both voices, mixed the way
    /// the silicon mixes them.
    pub fn sample(&mut self, sample_rate: f32) -> f32 {
        let pulse1 = if self.enabled & 0b01 != 0 {
            self.pulse1.sample(sample_rate)
        } else {
            0
        };
        let pulse2 = if self.enabled & 0b10 != 0 {
            self.pulse2.sample(sample_rate)
        } else {
            0
        };

        mix(pulse1, pulse2)
    }
}

/// The pulse pair's mixing curve, from the chip's own resistor
/// ladder. It is NOT a plain sum: two loud voices together come out
/// quieter than twice one voice, which keeps the mix from clipping.
fn mix(pulse1: u8, pulse2: u8) -> f32 {
    let sum = (pulse1 + pulse2) as f32;
    if sum == 0.0 {
        return 0.0;
    }

    95.88 / (8128.0 / sum + 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pulse mid-note, built the way the bus would build it.
    fn singing(control: u8, timer: u16) -> Pulse {
        let mut pulse = Pulse::new();
        pulse.write_control(control);
        pulse.write_timer_low(timer as u8);
        pulse.write_timer_high((timer >> 8) as u8);
        pulse
    }

    /// Count how many times a second of output rises from silence —
    /// which is the frequency, measured the way a tuner measures it.
    fn risings_per_second(pulse: &mut Pulse) -> usize {
        let mut count = 0;
        let mut previous = 0;
        for _ in 0..44_100 {
            let sample = pulse.sample(44_100.0);
            if previous == 0 && sample > 0 {
                count += 1;
            }
            previous = sample;
        }
        count
    }

    #[test]
    fn a_timer_of_253_sings_the_tuning_a() {
        // 1,789,773 / (16 x 254) = 440.4 Hz — the A orchestras tune to.
        let mut pulse = singing(0b0111_1111, 253);
        let risings = risings_per_second(&mut pulse);
        assert!((438..=442).contains(&risings), "sang {risings} Hz");
    }

    #[test]
    fn the_fat_square_is_up_half_the_time() {
        let mut pulse = singing(0b1000_1111, 253);
        let loud = (0..44_100)
            .filter(|_| pulse.sample(44_100.0) > 0)
            .count();
        let ratio = loud as f32 / 44_100.0;
        assert!((0.45..=0.55).contains(&ratio), "up {ratio} of the time");
    }

    #[test]
    fn volume_zero_and_tiny_timers_are_silence() {
        let mut muted = singing(0b0111_0000, 253); // volume 0
        let mut shrill = singing(0b0111_1111, 7); // timer under 8
        for _ in 0..1000 {
            assert_eq!(muted.sample(44_100.0), 0);
            assert_eq!(shrill.sample(44_100.0), 0);
        }
    }

    #[test]
    fn the_switchboard_silences_a_disabled_voice() {
        let mut apu = Apu::new();
        apu.write(0x4000, 0b0111_1111);
        apu.write(0x4002, 253);
        apu.write(0x4015, 0b0000_0010); // pulse 2 on — pulse 1 OFF
        for _ in 0..1000 {
            assert_eq!(apu.sample(44_100.0), 0.0);
        }
    }

    #[test]
    fn the_mixer_squashes_instead_of_clipping() {
        // Both voices at full blast: the mix stays shy of 0.26, the
        // resistor ladder's ceiling — not 2 x anything.
        assert!(mix(15, 15) < 0.26);
        assert!(mix(15, 15) < 2.0 * mix(15, 0));
        assert_eq!(mix(0, 0), 0.0);
    }
}
