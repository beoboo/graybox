//! The APU — the Audio Processing Unit. Five voices on the real chip;
//! here are the two that carry every melody: the pulse channels.

use std::cell::Cell;

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

    /// The 11-bit countdown that sets the pitch: the chip walks one
    /// step of the shape every 2 x (timer + 1) CPU cycles.
    timer: u16,

    /// Where in the eight-step cycle the voice is, as a fraction
    /// 0 to 1 — the one field that moves while a note is held.
    phase: f32,

    /// The note's alarm clock.
    length: LengthCounter,

    /// The note's fade.
    envelope: Envelope,

    /// The note's bend.
    sweep: Sweep,
}

impl Pulse {
    fn new(extra_step: bool) -> Pulse {
        Pulse {
            duty: 0,
            timer: 0,
            phase: 0.0,
            length: LengthCounter::new(),
            envelope: Envelope::new(),
            sweep: Sweep::new(extra_step),
        }
    }

    /// $4000 / $4004 — shape, envelope, and the halt bit. Chapter
    /// 19 read the low bits as a bare loudness; they were the
    /// envelope's settings all along.
    fn write_control(&mut self, value: u8) {
        self.duty = (value >> 6) as usize;
        self.envelope.write(value);
        self.length.set_halt(value & 0b0010_0000 != 0);
    }

    /// $4002 / $4006 — the timer's low eight bits. Melodies ride this
    /// register: small nudges here bend the pitch between notes.
    fn write_timer_low(&mut self, value: u8) {
        self.timer = (self.timer & 0x0700) | value as u16;
    }

    /// The half-frame beat: the sweep may bend the pitch, and the
    /// alarm clock counts the note down.
    fn half_frame(&mut self) {
        if let Some(timer) = self.sweep.tick(self.timer) {
            self.timer = timer;
        }
        self.length.tick();
    }

    /// $4003 / $4007 — the timer's top three bits, and a note-on:
    /// the length counter loads, the envelope restarts, the shape
    /// restarts from step zero.
    fn write_timer_high(&mut self, value: u8) {
        self.timer = (self.timer & 0x00FF) | (((value & 0x07) as u16) << 8);
        self.length.load(value);
        self.envelope.start = true;
        self.phase = 0.0;
    }

    /// The note being sung, in hertz: sixteen timer-loads of the
    /// timer per full eight-step cycle.
    fn frequency(&self) -> f32 {
        CPU_HZ / (16.0 * (self.timer as f32 + 1.0))
    }

    /// Sing one sample: the envelope's volume, shaped by the duty
    /// row — gated by the alarm clock and the sweep's mute rule
    /// (which swallows chapter 19's timer-under-8 check whole).
    fn sample(&mut self, sample_rate: f32) -> u8 {
        if !self.length.active() || self.sweep.mutes(self.timer) {
            return 0;
        }

        let step = (self.phase * 8.0) as usize & 7;
        self.phase = (self.phase + self.frequency() / sample_rate).fract();
        DUTY[self.duty][step] * self.envelope.volume()
    }
}

/// The thirty-two note lengths a channel can be told to hold, in
/// half-frames. The order looks shuffled because the five register
/// bits are really two smaller fields grown together in hardware.
const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

/// The note's alarm clock: loaded when a note starts, counted down at
/// half-frame rate, and the voice is cut the moment it hits zero — an
/// automatic note-off no program has to remember to send.
struct LengthCounter {
    counter: u8,

    /// The halt flag: notes that never end (and, on the pulse
    /// channels, the same bit loops the envelope).
    halt: bool,

    /// The $4015 enable bit. Disabled channels force their counter to
    /// zero and refuse loads.
    enabled: bool,

    /// Whether the next APU cycle is one that clocks this counter. A
    /// register write lands between two cycles, so "a write landing
    /// exactly on the clock" is, from here, a write landing while
    /// this flag is up — and two hardware rules hang on that
    /// coincidence, below.
    clock_imminent: bool,

    /// A halt flag written while the clock was imminent: it applies
    /// after that clock, not before it.
    pending_halt: Option<bool>,

    /// A reload accepted on the imminent clock (only possible with
    /// the counter at zero). That clock decided against the zero it
    /// found, so it must not eat the value just loaded.
    reloaded_on_the_clock: bool,
}

impl LengthCounter {
    fn new() -> LengthCounter {
        LengthCounter {
            counter: 0,
            halt: false,
            enabled: false,
            clock_imminent: false,
            pending_halt: None,
            reloaded_on_the_clock: false,
        }
    }

    /// Close one CPU cycle: apply what the clock was holding up, then
    /// note whether the *next* cycle carries the clock.
    fn end_cycle(&mut self, clock_imminent: bool) {
        if let Some(halt) = self.pending_halt.take() {
            self.halt = halt;
        }
        if !self.clock_imminent {
            self.reloaded_on_the_clock = false;
        }
        self.clock_imminent = clock_imminent;
    }

    /// A $4003-style write: look the five top bits up in the table.
    /// A reload landing on the very cycle the counter is clocked is
    /// ignored — unless the counter had already run out, in which
    /// case it goes through and the clock leaves it alone.
    fn load(&mut self, value: u8) {
        if self.clock_imminent {
            if self.counter > 0 {
                return;
            }
            self.reloaded_on_the_clock = true;
        }
        if self.enabled {
            self.counter = LENGTH_TABLE[(value >> 3) as usize];
        }
    }

    /// The halt bit from a control write. On the clock's own cycle it
    /// applies after the clock: the counter is counted one last time
    /// either way.
    fn set_halt(&mut self, halt: bool) {
        if self.clock_imminent {
            self.pending_halt = Some(halt);
        } else {
            self.halt = halt;
        }
    }

    /// The channel's $4015 bit. Disabling silences the note *now*.
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.counter = 0;
        }
    }

    /// The half-frame clock.
    fn tick(&mut self) {
        if self.counter > 0 && !self.halt && !self.reloaded_on_the_clock {
            self.counter -= 1;
        }
        self.reloaded_on_the_clock = false;
    }

    /// Is the note still sounding? Also the channel's status bit.
    fn active(&self) -> bool {
        self.counter > 0
    }
}

/// The envelope: hardware decay. Started by a note-on, it walks a
/// volume from 15 down to 0 at a programmed rate — or loops back to
/// 15, or is bypassed entirely for constant volume.
struct Envelope {
    /// Set by a note-on; the next quarter-frame restarts the decay.
    start: bool,

    /// The divider that slows quarter-frames down to the decay rate.
    divider: u8,

    /// The decaying volume itself, 15 down to 0.
    decay: u8,

    /// Loop (shared with the length counter's halt bit): at zero,
    /// wrap back to 15 instead of staying silent.
    loops: bool,

    /// Constant-volume mode: ignore the decay, play `period` as the
    /// volume — chapter 19's whole model, revealed as one mode of
    /// this machinery.
    constant: bool,

    /// Doubles as the decay rate and the constant volume.
    period: u8,
}

impl Envelope {
    fn new() -> Envelope {
        Envelope {
            start: false,
            divider: 0,
            decay: 0,
            loops: false,
            constant: false,
            period: 0,
        }
    }

    /// The low six bits of a $4000-style control write.
    fn write(&mut self, value: u8) {
        self.loops = value & 0b0010_0000 != 0;
        self.constant = value & 0b0001_0000 != 0;
        self.period = value & 0b0000_1111;
    }

    /// The quarter-frame clock.
    fn tick(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.period;
        } else if self.divider == 0 {
            self.divider = self.period;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.loops {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    /// What the channel should play at, right now.
    fn volume(&self) -> u8 {
        if self.constant {
            self.period
        } else {
            self.decay
        }
    }
}

/// The sweep: a pitch bender. Every few half-frames it shifts the
/// pulse's own period and adds or subtracts the result — small
/// periods slide fast, large ones slowly, which is why one circuit
/// makes both siren wails and gentle dives.
struct Sweep {
    enabled: bool,
    period: u8,
    negate: bool,
    shift: u8,

    /// The divider that slows half-frames to the programmed rate.
    divider: u8,

    /// A write restarts the divider at the next half-frame.
    reload: bool,

    /// Pulse 1 negates with one extra step down — the two channels'
    /// adders are wired differently, and games can hear it.
    extra_step: bool,
}

impl Sweep {
    fn new(extra_step: bool) -> Sweep {
        Sweep {
            enabled: false,
            period: 0,
            negate: false,
            shift: 0,
            divider: 0,
            reload: false,
            extra_step,
        }
    }

    /// A $4001-style write.
    fn write(&mut self, value: u8) {
        self.enabled = value & 0b1000_0000 != 0;
        self.period = (value >> 4) & 0b111;
        self.negate = value & 0b0000_1000 != 0;
        self.shift = value & 0b111;
        self.reload = true;
    }

    /// Where the bend is headed from `timer`.
    fn target(&self, timer: u16) -> u16 {
        let change = timer >> self.shift;
        if self.negate {
            let down = timer.saturating_sub(change);
            if self.extra_step {
                down.saturating_sub(1)
            } else {
                down
            }
        } else {
            timer.wrapping_add(change)
        }
    }

    /// The mute rule: the sweep silences its channel whenever the
    /// period is under 8 or the *target* overflows eleven bits — even
    /// with the sweep disabled. The comparison never sleeps.
    fn mutes(&self, timer: u16) -> bool {
        timer < 8 || self.target(timer) > 0x7FF
    }

    /// The half-frame clock: maybe hand back a new period.
    fn tick(&mut self, timer: u16) -> Option<u16> {
        let mut moved = None;
        if self.divider == 0 && self.enabled && self.shift > 0 && !self.mutes(timer) {
            moved = Some(self.target(timer));
        }
        if self.divider == 0 || self.reload {
            self.divider = self.period;
            self.reload = false;
        } else {
            self.divider -= 1;
        }
        moved
    }
}

/// The sound chip: four singing voices, and the conductor that cues
/// their counters.
pub struct Apu {
    pulse1: Pulse,
    pulse2: Pulse,
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,
    filter: OutputFilter,
    frame: FrameCounter,
}

impl Apu {
    pub fn new() -> Apu {
        Apu {
            pulse1: Pulse::new(true),
            pulse2: Pulse::new(false),
            triangle: Triangle::new(),
            noise: Noise::new(),
            dmc: Dmc::new(),
            filter: OutputFilter::new(),
            frame: FrameCounter::new(),
        }
    }

    /// A write to any sound register. Every voice answers now; only
    /// the sample channel's registers still fall through, for one
    /// more chapter.
    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0x4000 => self.pulse1.write_control(value),
            0x4001 => self.pulse1.sweep.write(value),
            0x4002 => self.pulse1.write_timer_low(value),
            0x4003 => self.pulse1.write_timer_high(value),
            0x4004 => self.pulse2.write_control(value),
            0x4005 => self.pulse2.sweep.write(value),
            0x4006 => self.pulse2.write_timer_low(value),
            0x4007 => self.pulse2.write_timer_high(value),
            0x4008 => self.triangle.write_control(value),
            0x400A => self.triangle.write_timer_low(value),
            0x400B => self.triangle.write_timer_high(value),
            0x400C => self.noise.write_control(value),
            0x400E => self.noise.write_mode(value),
            0x400F => self.noise.write_length(value),
            0x4010 => self.dmc.write_control(value),
            0x4011 => self.dmc.write_level(value),
            0x4012 => self.dmc.write_address(value),
            0x4013 => self.dmc.write_length(value),
            0x4015 => {
                self.pulse1.length.set_enabled(value & 0b0001 != 0);
                self.pulse2.length.set_enabled(value & 0b0010 != 0);
                self.triangle.length.set_enabled(value & 0b0100 != 0);
                self.noise.length.set_enabled(value & 0b1000 != 0);
                self.dmc
                    .set_enabled(value & 0b1_0000 != 0, self.frame.apu_cycle);
                self.dmc.irq_pending.set(false);
            }
            0x4017 => {
                if self.frame.write(value) {
                    self.quarter_frame();
                    self.half_frame();
                }
            }
            _ => {}
        }
    }

    /// One CPU cycle for the whole chip: the conductor beats, the
    /// counters follow, and every length counter closes the cycle
    /// knowing whether the next one carries its clock.
    pub fn tick(&mut self) {
        self.dmc.tick();
        let (quarter, half) = self.frame.tick();
        if quarter {
            self.quarter_frame();
        }
        if half {
            self.half_frame();
        }

        let imminent = self.frame.clocks_length_next();
        self.pulse1.length.end_cycle(imminent);
        self.pulse2.length.end_cycle(imminent);
        self.triangle.length.end_cycle(imminent);
        self.noise.length.end_cycle(imminent);
    }

    /// The quarter-frame beat: envelopes and the linear counter.
    fn quarter_frame(&mut self) {
        self.pulse1.envelope.tick();
        self.pulse2.envelope.tick();
        self.triangle.quarter_frame();
        self.noise.envelope.tick();
    }

    /// The half-frame beat: sweeps and length counters.
    fn half_frame(&mut self) {
        self.pulse1.half_frame();
        self.pulse2.half_frame();
        self.triangle.length.tick();
        self.noise.length.tick();
    }
    /// A read of $4015: which notes still sound, and whether the
    /// conductor's IRQ is up. Reading clears the IRQ flag — one more
    /// register where looking changes what you see.
    pub fn read_status(&self) -> u8 {
        let mut status = 0;
        if self.pulse1.length.active() {
            status |= 0b0001;
        }
        if self.pulse2.length.active() {
            status |= 0b0010;
        }
        if self.triangle.length.active() {
            status |= 0b0100;
        }
        if self.noise.length.active() {
            status |= 0b1000;
        }
        if self.dmc.bytes_remaining > 0 {
            status |= 0b1_0000;
        }
        if self.dmc.irq_pending.get() {
            status |= 0b1000_0000;
        }
        if self.frame.take_irq() {
            status |= 0b0100_0000;
        }
        status
    }

    /// Whether the chip is pulling the CPU's interrupt line: the
    /// conductor's flag or the sampler's — live, both of them. The
    /// one-cycle lag the poll needs lives in the CPU now, where it
    /// can serve every line at once.
    pub fn irq_pending(&self) -> bool {
        self.frame.irq_pending.get() || self.dmc.irq_pending.get()
    }

    /// The sampler's fetch request, passed through for the CPU.
    pub fn dmc_fetch(&mut self) -> Option<u16> {
        self.dmc.take_fetch()
    }

    /// The DMA's answer, passed back in.
    pub fn dmc_supply(&mut self, value: u8) {
        self.dmc.supply(value);
    }

    /// One sample of the chip's output: all four voices, mixed the
    /// way the silicon mixes them. The $4015 gates of chapter 19
    /// live inside the length counters now.
    pub fn sample(&mut self, sample_rate: f32) -> f32 {
        let pulse1 = self.pulse1.sample(sample_rate);
        let pulse2 = self.pulse2.sample(sample_rate);
        let triangle = self.triangle.sample(sample_rate);
        let noise = self.noise.sample(sample_rate);

        let mixed = mix(pulse1, pulse2, triangle, noise, self.dmc.level);
        self.filter.process(mixed, sample_rate)
    }
}

/// The chip's two mixing curves, one per resistor ladder: the pulse
/// pair on one, triangle-noise-sample on the other — the sample
/// channel's name on that ladder finally cashed in. Neither curve
/// is a plain sum: loud voices crowd each other instead of clipping.
fn mix(pulse1: u8, pulse2: u8, triangle: u8, noise: u8, dmc: u8) -> f32 {
    let pulses = (pulse1 + pulse2) as f32;
    let pulse_out = if pulses == 0.0 {
        0.0
    } else {
        95.88 / (8128.0 / pulses + 100.0)
    };

    let tnd = triangle as f32 / 8227.0 + noise as f32 / 12241.0 + dmc as f32 / 22638.0;
    let tnd_out = if tnd == 0.0 {
        0.0
    } else {
        159.79 / (1.0 / tnd + 100.0)
    };

    pulse_out + tnd_out
}

/// The console's last inch of wire: before the mixed signal reaches
/// the speaker it crosses three little analogue stages — capacitors
/// and resistors, not logic. Two high-passes (90 Hz and 440 Hz) pass
/// CHANGE and block anything steady; one low-pass (14 kHz) rounds the
/// harshest corners off the squares. This matters more than it looks:
/// the mixer's silence is 0.0, not a midpoint, so an unfiltered note
/// PARKS the speaker cone off-center — every start thumps, every DMC
/// step clicks. The high-passes un-park it.
struct OutputFilter {
    hp90: (f32, f32),
    hp440: (f32, f32),
    lp14k: f32,
}

impl OutputFilter {
    fn new() -> OutputFilter {
        OutputFilter { hp90: (0.0, 0.0), hp440: (0.0, 0.0), lp14k: 0.0 }
    }

    fn process(&mut self, sample: f32, sample_rate: f32) -> f32 {
        let sample = high_pass(&mut self.hp90, sample, 90.0, sample_rate);
        let sample = high_pass(&mut self.hp440, sample, 440.0, sample_rate);
        low_pass(&mut self.lp14k, sample, 14_000.0, sample_rate)
    }
}

/// One resistor-capacitor stage, high-pass wiring: the output follows
/// change in the input and decays toward zero when nothing changes.
/// `state` is (previous input, previous output).
fn high_pass(state: &mut (f32, f32), input: f32, cutoff: f32, sample_rate: f32) -> f32 {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    let dt = 1.0 / sample_rate;
    let alpha = rc / (rc + dt);
    let output = alpha * (state.1 + input - state.0);
    *state = (input, output);
    output
}

/// The same stage, wired the other way: the output chases the input
/// but can only move so fast, which sands the edges off a square.
fn low_pass(state: &mut f32, input: f32, cutoff: f32, sample_rate: f32) -> f32 {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    let dt = 1.0 / sample_rate;
    let alpha = dt / (rc + dt);
    *state += alpha * (input - *state);
    *state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pulse mid-note, built the way the bus would build it. The
    /// note-on only lands if the channel is enabled first — the same
    /// order every real program follows.
    fn singing(control: u8, timer: u16) -> Pulse {
        let mut pulse = Pulse::new(true);
        pulse.length.set_enabled(true);
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
    fn the_fat_square_is_up_half_the_time() {
        // Same test as chapter 19 wrote it, one bit different: bit 4
        // now asks for constant volume, which is what "volume 15"
        // always secretly was.
        let mut pulse = singing(0b1001_1111, 253);
        let loud = (0..44_100).filter(|_| pulse.sample(44_100.0) > 0).count();
        let ratio = loud as f32 / 44_100.0;
        assert!((0.45..=0.55).contains(&ratio), "up {ratio} of the time");
    }

    #[test]
    fn the_mixer_squashes_instead_of_clipping() {
        // Both pulses at full blast: the mix stays shy of 0.26, the
        // resistor ladder's ceiling — not 2 x anything.
        assert!(mix(15, 15, 0, 0, 0) < 0.26);
        assert!(mix(15, 15, 0, 0, 0) < 2.0 * mix(15, 0, 0, 0, 0));
        assert_eq!(mix(0, 0, 0, 0, 0), 0.0);
    }

    #[test]
    fn the_second_ladder_mixes_the_new_voices() {
        // The triangle alone registers louder than the noise alone at
        // equal levels — the resistor ratios say so — and both crowd
        // rather than sum. The sampler rides the same ladder, at the
        // quietest tap of the three.
        assert!(mix(0, 0, 15, 0, 0) > mix(0, 0, 0, 15, 0));
        assert!(mix(0, 0, 0, 15, 0) > mix(0, 0, 0, 0, 15));
        let both = mix(0, 0, 15, 15, 0);
        assert!(both < mix(0, 0, 15, 0, 0) + mix(0, 0, 0, 15, 0));
        assert!(both > 0.0);
    }

    /// Tick the whole chip for a stretch of CPU cycles.
    fn run(apu: &mut Apu, cycles: u64) {
        for _ in 0..cycles {
            apu.tick();
        }
    }

    #[test]
    fn the_conductor_beats_four_times_a_sequence() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0b0001);
        apu.write(0x4000, 0b0001_1111); // halt off, constant volume
        apu.write(0x4003, 0); // length index 0: ten half-frames

        // Ten half-frames arrive over five sequences of the 4-step
        // beat. One cycle short of the fifth sequence's last clock,
        // the note still sounds; one cycle later it is cut.
        run(&mut apu, 4 * 29830 + 29828);
        assert_ne!(apu.read_status() & 0b0001, 0, "cut too early");
        run(&mut apu, 1);
        assert_eq!(apu.read_status() & 0b0001, 0, "still on at ten");
    }

    #[test]
    fn the_halt_flag_holds_a_note_forever() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0b0001);
        apu.write(0x4000, 0b0011_1111); // bit 5: halt ON
        apu.write(0x4003, 0);

        run(&mut apu, 29830 * 20);
        assert_ne!(apu.read_status() & 0b0001, 0, "halted note ended");
    }

    #[test]
    fn disabling_a_channel_cuts_its_note_now() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0b0001);
        apu.write(0x4003, 0);
        assert_ne!(apu.read_status() & 0b0001, 0);

        apu.write(0x4015, 0);
        assert_eq!(apu.read_status() & 0b0001, 0, "disable must silence");

        // And a note-on while disabled must not stick.
        apu.write(0x4003, 0);
        assert_eq!(apu.read_status() & 0b0001, 0, "loaded while disabled");
    }

    #[test]
    fn the_envelope_decays_and_the_constant_bit_bypasses_it() {
        let mut envelope = Envelope::new();
        envelope.write(0b0000_0000); // decay mode, fastest rate
        envelope.start = true;
        envelope.tick();
        assert_eq!(envelope.volume(), 15, "decay starts from the top");
        for _ in 0..15 {
            envelope.tick();
        }
        assert_eq!(envelope.volume(), 0, "and walks to silence");

        envelope.write(0b0001_0101); // constant volume 5
        assert_eq!(envelope.volume(), 5, "constant mode reads the period");
    }

    #[test]
    fn the_sweep_bends_and_its_mute_rule_never_sleeps() {
        let mut pulse = singing(0b0011_1111, 100);

        // Sweep up, shift 2: 100 + 25 = 125 at the first half-frame.
        pulse.sweep.write(0b1000_0010);
        pulse.half_frame();
        assert_eq!(pulse.timer, 125, "the bend moved the pitch");

        // A target past $7FF mutes the voice even between beats:
        // $700 + ($700 >> 2) overflows the eleven bits.
        pulse.timer = 0x700;
        assert!(pulse.sweep.mutes(pulse.timer));
        assert_eq!(pulse.sample(44_100.0), 0);
    }

    #[test]
    fn pulse_one_negates_one_deeper_than_pulse_two() {
        let mut one = Sweep::new(true);
        let mut two = Sweep::new(false);
        one.write(0b1000_1010); // negate, shift 2
        two.write(0b1000_1010);
        assert_eq!(one.target(100), 74, "ones' complement: 100 - 25 - 1");
        assert_eq!(two.target(100), 75, "two's complement: 100 - 25");
    }

    #[test]
    fn the_triangle_needs_both_locks_open() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0b0100);
        apu.write(0x4008, 0x81); // hold bit + linear reload 1
        apu.write(0x400A, 70); // the 788 Hz lead from Lan Master
        apu.write(0x400B, 0); // note-on

        // Before any quarter-frame the linear counter is still zero:
        // the length lock is open, the linear lock is not.
        assert_eq!(apu.triangle.sample(44_100.0), 0, "linear lock closed");
        run(&mut apu, 7457); // the first quarter-frame beat
        let mut sang = false;
        for _ in 0..100 {
            sang |= apu.triangle.sample(44_100.0) > 0;
        }
        assert!(sang, "both locks open, still silent");

        // A note-off the FamiTone way: reload value zero.
        apu.write(0x4008, 0x80);
        run(&mut apu, 29830);
        assert_eq!(apu.triangle.sample(44_100.0), 0, "note-off ignored");
    }

    #[test]
    fn the_triangle_walks_all_thirty_two_steps() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0b0100);
        apu.write(0x4008, 0xFF);
        apu.write(0x400A, 70);
        apu.write(0x400B, 0);
        run(&mut apu, 7457);

        let mut seen = [false; 16];
        for _ in 0..1000 {
            seen[apu.triangle.sample(44_100.0) as usize] = true;
        }
        assert!(seen.iter().all(|s| *s), "some staircase level missing");
    }

    #[test]
    fn the_noise_register_ticks_like_the_hardware() {
        let mut noise = Noise::new();
        // Mode 1 turns the register into a 93-step loop; walk one
        // full loop and land back at the seed.
        noise.mode = true;
        let seed = noise.lfsr;
        for _ in 0..93 {
            noise.shift();
        }
        assert_eq!(noise.lfsr, seed, "mode 1 is a 93-step pattern");

        // Mode 0 must NOT close after 93 — its period is 32,767.
        noise.mode = false;
        noise.lfsr = 1;
        for _ in 0..93 {
            noise.shift();
        }
        assert_ne!(noise.lfsr, 1, "mode 0 closed far too soon");
    }

    #[test]
    fn reading_status_clears_the_conductors_irq() {
        let mut apu = Apu::new();
        run(&mut apu, 29830);
        let first = apu.read_status();
        assert_ne!(first & 0b0100_0000, 0, "the sequence's end raises it");
        let second = apu.read_status();
        assert_eq!(second & 0b0100_0000, 0, "and reading clears it");
    }

    #[test]
    fn the_inhibit_bit_silences_the_conductor() {
        let mut apu = Apu::new();
        apu.write(0x4017, 0b0100_0000);
        run(&mut apu, 29830 * 3);
        assert_eq!(apu.read_status() & 0b0100_0000, 0, "inhibited IRQ fired");
    }

    #[test]
    fn a_4017_write_restarts_the_sequence_a_moment_later() {
        let mut apu = Apu::new();
        // Get near the first beat, then restart: the sequence starts
        // over — three cycles late, because the write landed on an
        // APU cycle (four, had it landed between two).
        run(&mut apu, 7000);
        apu.write(0x4017, 0);
        run(&mut apu, 1000);
        assert_eq!(apu.frame.cycle, 997, "restart was not deferred by 3");
    }

    #[test]
    fn the_five_step_bit_clocks_everything_immediately() {
        let mut apu = Apu::new();
        apu.write(0x4015, 0b0001);
        apu.write(0x4000, 0b0001_1111); // halt off
        apu.write(0x4003, 0b1111_1000); // length index 31: thirty

        // No conductor beat has happened — but the 5-step bit brings
        // its own: one write, and thirty is already twenty-nine.
        apu.write(0x4017, 0b1000_0000);
        assert_eq!(apu.pulse1.length.counter, 29, "no immediate clock");
    }
    #[test]
    fn the_sampler_asks_the_moment_its_buffer_is_empty() {
        let mut apu = Apu::new();
        apu.write(0x4012, 2); // sample at $C000 + 2 x 64
        apu.write(0x4013, 1); // 17 bytes
        apu.write(0x4015, 0b1_0000);

        // The request surfaces a couple of cycles later — parity
        // manners — and names the sample's first address.
        assert_eq!(apu.dmc_fetch(), None, "asked before the delay ran out");
        run(&mut apu, 3);
        assert_eq!(apu.dmc_fetch(), Some(0xC080));
    }

    #[test]
    fn each_bit_nudges_the_level_by_two() {
        let mut apu = Apu::new();
        apu.write(0x4012, 0);
        apu.write(0x4013, 0); // "zero" still means one byte
        apu.write(0x4015, 0b1_0000);
        run(&mut apu, 3);
        apu.dmc_fetch();
        apu.dmc_supply(0b1111_1111); // eight steps up

        // The output unit must first shift out the eight silent bits
        // it woke up with; only then does the byte load and nudge the
        // level up 2 x 8 = 16 over the next eight periods.
        run(&mut apu, 428 * 18);
        assert_eq!(apu.dmc.level, 16);
    }

    #[test]
    fn the_level_never_leaves_its_seven_bits() {
        let mut apu = Apu::new();
        apu.write(0x4011, 0x7E); // near the top
        apu.write(0x4012, 0);
        apu.write(0x4013, 0);
        apu.write(0x4015, 0b1_0000);
        run(&mut apu, 3);
        apu.dmc_fetch();
        apu.dmc_supply(0xFF); // eight more ups
        run(&mut apu, 428 * 18);
        assert!(apu.dmc.level <= 0x7F, "the DAC is seven bits wide");
    }

    #[test]
    fn a_finished_sample_raises_the_irq_and_4015_reports_it() {
        let mut apu = Apu::new();
        apu.write(0x4010, 0b1000_0000); // IRQ on, no loop
        apu.write(0x4012, 0);
        apu.write(0x4013, 0); // one byte
        apu.write(0x4015, 0b1_0000);
        run(&mut apu, 3);
        let address = apu.dmc_fetch().unwrap();
        apu.dmc_supply(0x00);
        assert_eq!(address, 0xC000);
        assert_ne!(apu.read_status() & 0b1000_0000, 0, "the sample ended");

        // Writing $4015 clears the sampler's IRQ — that is the
        // acknowledgement games use.
        apu.write(0x4015, 0);
        assert_eq!(apu.read_status() & 0b1000_0000, 0);
    }

    #[test]
    fn a_byte_for_a_cancelled_sample_changes_nothing() {
        let mut apu = Apu::new();
        apu.write(0x4012, 0);
        apu.write(0x4013, 1);
        apu.write(0x4015, 0b1_0000);
        run(&mut apu, 3);
        apu.dmc_fetch();

        // The game calls the sample off while the DMA is in flight.
        apu.write(0x4015, 0);
        apu.dmc_supply(0xAA);

        // The late byte must not resurrect it: a $4015 read that
        // says "still playing" here would say it forever.
        assert_eq!(apu.read_status() & 0b1_0000, 0, "a ghost sample plays on");
    }

    #[test]
    fn the_filter_unparks_the_speaker() {
        // A steady level is exactly what the mixer emits mid-note.
        // The high-passes must walk it back to zero — no parked cone,
        // no thump.
        let mut filter = OutputFilter::new();
        let mut last = 1.0;
        for _ in 0..44_100 {
            last = filter.process(0.5, 44_100.0);
        }
        assert!(last.abs() < 0.001, "DC survived: {last}");
    }

    #[test]
    fn the_filter_lets_the_music_through() {
        // A 1 kHz wobble sits squarely between the corners and must
        // keep most of its size.
        let mut filter = OutputFilter::new();
        let mut peak: f32 = 0.0;
        for n in 0..44_100 {
            let t = n as f32 / 44_100.0;
            let out = filter.process((2.0 * std::f32::consts::PI * 1000.0 * t).sin(), 44_100.0);
            if n > 22_050 {
                peak = peak.max(out.abs());
            }
        }
        assert!(peak > 0.8, "the tone was crushed to {peak}");
    }
}

/// The 4-step conductor's beat, in CPU cycles from the sequence's
/// start: which cycles clock the quarter-frame units, and which of
/// those also clock the half-frame units (`true`).
const FOUR_STEP: [(u64, bool); 4] = [(7457, false), (14913, true), (22371, false), (29829, true)];

/// One cycle past the 4-step sequence's last clock: where it wraps.
const FOUR_STEP_WRAP: u64 = 29830;

/// The 5-step beat: same first three clocks, a longer tail, no IRQ.
const FIVE_STEP: [(u64, bool); 4] = [(7457, false), (14913, true), (22371, false), (37281, true)];

/// Where the 5-step sequence wraps.
const FIVE_STEP_WRAP: u64 = 37282;

/// The frame counter — the chip's conductor. A free-running divider
/// off the CPU clock that cues the envelopes, sweeps, linear and
/// length counters at fixed points in a repeating sequence. Nothing
/// to do with video frames, despite the name.
struct FrameCounter {
    /// $4017 bit 7: the 5-step sequence instead of the 4-step.
    five_step: bool,

    /// CPU cycles since the sequence last restarted.
    cycle: u64,

    /// $4017 bit 6: never raise the frame IRQ, and drop any pending.
    irq_inhibit: bool,

    /// The IRQ flag, reported by $4015 bit 6 and cleared by reading
    /// it — a read that changes state, so a `Cell`, like $2002's.
    irq_pending: Cell<bool>,

    /// A $4017 write restarts the sequence — but not immediately:
    /// three cycles later if the write lands on an APU cycle, four if
    /// it lands between two.
    pending_reset: Option<u8>,

    /// Which half of the divide-by-two the last CPU cycle fell on.
    /// Free-running: a $4017 write does not restart *this*.
    apu_cycle: bool,
}

impl FrameCounter {
    fn new() -> FrameCounter {
        FrameCounter {
            five_step: false,
            cycle: 0,
            irq_inhibit: false,
            irq_pending: Cell::new(false),
            pending_reset: None,
            apu_cycle: true,
        }
    }

    fn sequence(&self) -> (&'static [(u64, bool)], u64) {
        if self.five_step {
            (&FIVE_STEP, FIVE_STEP_WRAP)
        } else {
            (&FOUR_STEP, FOUR_STEP_WRAP)
        }
    }

    /// Advance one CPU cycle. Returns (quarter, half): what to clock.
    fn tick(&mut self) -> (bool, bool) {
        if let Some(delay) = self.pending_reset {
            if delay == 0 {
                self.pending_reset = None;
                self.cycle = 0;
            } else {
                self.pending_reset = Some(delay - 1);
            }
        }

        self.cycle += 1;
        self.apu_cycle = !self.apu_cycle;

        let (sequence, wrap) = self.sequence();
        let clock = sequence
            .iter()
            .find(|(cycle, _)| *cycle == self.cycle)
            .map(|(_, half)| (true, *half))
            .unwrap_or((false, false));

        // The 4-step sequence holds its IRQ up across the last THREE
        // cycles — 29828 through 29830 — not one. A program that
        // reads $4015 inside the window clears the flag and finds it
        // set again on the very next cycle.
        if !self.five_step && !self.irq_inhibit && (29828..=29830).contains(&self.cycle) {
            self.irq_pending.set(true);
        }

        if self.cycle == wrap {
            self.cycle = 0;
        }

        clock
    }

    /// A $4017 write. Returns whether to clock quarter AND half
    /// frames immediately — the 5-step bit does, which is how music
    /// drivers force a known starting state.
    fn write(&mut self, value: u8) -> bool {
        self.five_step = value & 0b1000_0000 != 0;
        self.irq_inhibit = value & 0b0100_0000 != 0;
        if self.irq_inhibit {
            self.irq_pending.set(false);
        }
        self.pending_reset = Some(if self.apu_cycle { 3 } else { 4 });
        self.five_step
    }

    /// Whether the *next* tick clocks the length counters — the
    /// question `LengthCounter::end_cycle` needs answered ahead.
    fn clocks_length_next(&self) -> bool {
        if matches!(self.pending_reset, Some(0)) {
            return false;
        }
        let (sequence, _) = self.sequence();
        let next = self.cycle + 1;
        sequence.iter().any(|(cycle, half)| *cycle == next && *half)
    }

    /// Read-and-clear, as reading $4015 does.
    fn take_irq(&self) -> bool {
        let pending = self.irq_pending.get();
        self.irq_pending.set(false);
        pending
    }
}
/// The triangle's staircase, thirty-two steps down then up. No
/// volume knob anywhere: the wave plays at one loudness or not at
/// all.
const TRIANGLE_STEPS: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

/// The third voice: the triangle, the console's bass. Its timer runs
/// at CPU rate — twice the pulses' — and its note-off has two locks:
/// the shared length counter and a private "linear" counter with
/// finer, quarter-frame resolution.
pub struct Triangle {
    timer: u16,
    phase: f32,

    /// The second lock: counted down every quarter-frame.
    linear: u8,

    /// What the linear counter reloads to ($4008's low seven bits).
    linear_reload: u8,

    /// $4008 bit 7 — hold the note: reload the linear counter every
    /// quarter-frame instead of counting it down. Doubles as the
    /// length counter's halt, same as the pulses' envelope-loop bit.
    linear_halt: bool,

    /// Set by a note-on; the next quarter-frame reloads the linear
    /// counter instead of counting it.
    reload_flag: bool,

    length: LengthCounter,
}

impl Triangle {
    fn new() -> Triangle {
        Triangle {
            timer: 0,
            phase: 0.0,
            linear: 0,
            linear_reload: 0,
            linear_halt: false,
            reload_flag: false,
            length: LengthCounter::new(),
        }
    }

    /// $4008 — the linear counter's reload value and the hold bit.
    fn write_control(&mut self, value: u8) {
        self.linear_reload = value & 0b0111_1111;
        self.linear_halt = value & 0b1000_0000 != 0;
        self.length.set_halt(self.linear_halt);
    }

    /// $400A — the timer's low eight bits.
    fn write_timer_low(&mut self, value: u8) {
        self.timer = (self.timer & 0x0700) | value as u16;
    }

    /// $400B — timer high, and a note-on: length loads, and the
    /// linear counter is flagged to reload at the next quarter-frame.
    fn write_timer_high(&mut self, value: u8) {
        self.timer = (self.timer & 0x00FF) | (((value & 0x07) as u16) << 8);
        self.length.load(value);
        self.reload_flag = true;
    }

    /// The quarter-frame clock: the linear counter's turn.
    fn quarter_frame(&mut self) {
        if self.reload_flag {
            self.linear = self.linear_reload;
        } else if self.linear > 0 {
            self.linear -= 1;
        }
        // With the hold bit set the reload flag stays up, so the
        // counter reloads every beat and the note sustains forever.
        if !self.linear_halt {
            self.reload_flag = false;
        }
    }

    /// The staircase's frequency: thirty-two steps per cycle, timer
    /// clocked at CPU rate.
    fn frequency(&self) -> f32 {
        CPU_HZ / (32.0 * (self.timer as f32 + 1.0))
    }

    /// Sing one sample. Both locks must be open; a timer under 2
    /// would sing far above hearing, so it is treated as silence.
    fn sample(&mut self, sample_rate: f32) -> u8 {
        if !self.length.active() || self.linear == 0 || self.timer < 2 {
            return 0;
        }

        let step = (self.phase * 32.0) as usize & 31;
        self.phase = (self.phase + self.frequency() / sample_rate).fract();
        TRIANGLE_STEPS[step]
    }
}

/// How many CPU cycles between shifts of the noise register, for
/// each of the sixteen speeds a game can pick: fast is hiss, slow
/// is rumble.
const NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

/// The drummer: a fifteen-bit shift register playing pseudo-random
/// bits as sound. One XOR is the entire instrument.
pub struct Noise {
    /// The shift register. Never zero — seeded with 1, and the
    /// feedback keeps it alive forever after.
    lfsr: u16,

    /// Mode flag ($400E bit 7): feed back from bit 6 instead of
    /// bit 1, shortening the pattern from 32,767 steps to 93 — less
    /// hiss, more buzz.
    mode: bool,

    /// CPU cycles between shifts, from NOISE_PERIODS.
    period: u16,

    /// Fractional shifts owed to the register, carried between
    /// samples.
    owed: f32,

    length: LengthCounter,
    envelope: Envelope,
}

impl Noise {
    fn new() -> Noise {
        Noise {
            lfsr: 1,
            mode: false,
            period: NOISE_PERIODS[0],
            owed: 0.0,
            length: LengthCounter::new(),
            envelope: Envelope::new(),
        }
    }

    /// $400C — envelope and halt, exactly like a pulse's control.
    fn write_control(&mut self, value: u8) {
        self.envelope.write(value);
        self.length.set_halt(value & 0b0010_0000 != 0);
    }

    /// $400E — the mode bit and the speed.
    fn write_mode(&mut self, value: u8) {
        self.mode = value & 0b1000_0000 != 0;
        self.period = NOISE_PERIODS[(value & 0b1111) as usize];
    }

    /// $400F — a note-on: length loads, envelope restarts.
    fn write_length(&mut self, value: u8) {
        self.length.load(value);
        self.envelope.start = true;
    }

    /// One shift: bit 0 XOR bit 1 (or bit 6) becomes the new bit 14.
    fn shift(&mut self) {
        let tap = if self.mode { 6 } else { 1 };
        let feedback = (self.lfsr & 1) ^ ((self.lfsr >> tap) & 1);
        self.lfsr = (self.lfsr >> 1) | (feedback << 14);
    }

    /// Sing one sample: run the register as many shifts as this
    /// slice of time owes it, then let bit 0 gate the volume.
    fn sample(&mut self, sample_rate: f32) -> u8 {
        if !self.length.active() {
            return 0;
        }

        self.owed += CPU_HZ / self.period as f32 / sample_rate;
        while self.owed >= 1.0 {
            self.shift();
            self.owed -= 1.0;
        }

        if self.lfsr & 1 == 0 {
            self.envelope.volume()
        } else {
            0
        }
    }
}

/// How many CPU cycles between bits of a sample, for each of the
/// sixteen playback rates.
const DMC_RATES: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];

/// The fifth voice: the DMC, a sampler one bit at a time. Each bit
/// nudges a 7-bit level up or down by two — sound as a diary of
/// changes rather than of values.
pub struct Dmc {
    /// $4010 bit 7: raise an IRQ when the sample ends.
    irq_enabled: bool,

    /// $4010 bit 6: start the sample over instead of stopping.
    loop_flag: bool,

    /// The 7-bit level the channel presents to the mixer.
    level: u8,

    /// The one-byte buffer between the memory reader and the output
    /// unit. Hardware really has exactly one byte here.
    buffer: Option<u8>,

    /// The output unit: eight bits shifting out of one byte.
    shift: u8,
    bits_remaining: u8,

    /// Silent means the level holds still, not that it drops.
    silence: bool,

    /// Where the next sample byte lives, and how many remain.
    current_address: u16,
    bytes_remaining: u16,

    /// $4012/$4013 raw values, kept to restart from.
    sample_address: u8,
    sample_length: u8,

    /// CPU cycles between output bits, from DMC_RATES.
    timer: u16,
    timer_value: u16,

    /// A byte the channel wants fetched. It cannot read memory
    /// itself — the fetch is a DMA the CPU must perform.
    pending_fetch: Option<u16>,

    /// Cycles before a $4015-started fetch may surface: two or
    /// three by write parity, so every start lands on one parity.
    start_delay: u8,

    /// Set when a sample ends with the IRQ enabled.
    irq_pending: Cell<bool>,
}

impl Dmc {
    fn new() -> Dmc {
        Dmc {
            irq_enabled: false,
            loop_flag: false,
            level: 0,
            buffer: None,
            shift: 0,
            bits_remaining: 8,
            silence: true,
            current_address: 0,
            bytes_remaining: 0,
            sample_address: 0,
            sample_length: 0,
            timer: DMC_RATES[0],
            timer_value: 1,
            pending_fetch: None,
            start_delay: 0,
            irq_pending: Cell::new(false),
        }
    }

    /// $4010 — rate, loop, and the IRQ switch. Switching the IRQ off
    /// also drops one already raised.
    fn write_control(&mut self, value: u8) {
        self.irq_enabled = value & 0b1000_0000 != 0;
        self.loop_flag = value & 0b0100_0000 != 0;
        self.timer = DMC_RATES[(value & 0b1111) as usize];
        if !self.irq_enabled {
            self.irq_pending.set(false);
        }
    }

    /// $4011 — write the level directly: PCM by hand.
    fn write_level(&mut self, value: u8) {
        self.level = value & 0b0111_1111;
    }

    /// $4012 — where the sample starts: $C000 plus 64-byte steps.
    fn write_address(&mut self, value: u8) {
        self.sample_address = value;
    }

    /// $4013 — how long it is: 16-byte steps, plus one.
    fn write_length(&mut self, value: u8) {
        self.sample_length = value;
    }

    /// Point the reader back at the sample's start.
    fn restart(&mut self) {
        self.current_address = 0xC000 | ((self.sample_address as u16) << 6);
        self.bytes_remaining = ((self.sample_length as u16) << 4) | 1;
    }
    /// The $4015 bit: enabling starts the sample (unless one is
    /// mid-flight); disabling stops it now. The parity decides the
    /// start-up delay — see `start_delay`.
    fn set_enabled(&mut self, enabled: bool, on_even_cycle: bool) {
        if enabled {
            if self.bytes_remaining == 0 {
                self.restart();
            }
            if self.buffer.is_none() && self.bytes_remaining > 0 && self.pending_fetch.is_none() {
                self.start_delay = if on_even_cycle { 3 } else { 2 };
            }
        } else {
            self.bytes_remaining = 0;
        }
    }

    /// One CPU cycle. The timer and output unit free-run from
    /// power-on; $4015 gates only the memory reader.
    fn tick(&mut self) {
        if self.start_delay > 0 {
            self.start_delay -= 1;
        }

        // The reader asks the moment the buffer is empty and bytes
        // remain — independently of the output unit, which is what
        // makes the buffer a buffer.
        if self.start_delay == 0
            && self.buffer.is_none()
            && self.bytes_remaining > 0
            && self.pending_fetch.is_none()
        {
            self.pending_fetch = Some(self.current_address);
        }

        if self.timer_value == 0 {
            // One less than the period: acting at zero and reloading
            // with the full value would stretch every rate by one.
            self.timer_value = self.timer.saturating_sub(1);

            if !self.silence {
                if self.shift & 1 != 0 {
                    if self.level <= 0x7D {
                        self.level += 2;
                    }
                } else if self.level >= 0x02 {
                    self.level -= 2;
                }
                self.shift >>= 1;
            }

            self.bits_remaining = self.bits_remaining.saturating_sub(1);
            if self.bits_remaining == 0 {
                self.bits_remaining = 8;
                match self.buffer.take() {
                    Some(byte) => {
                        self.silence = false;
                        self.shift = byte;
                        // Ask for the refill in this same cycle: the
                        // stall's length hangs on the halt's parity,
                        // and the timer's even period keeps every
                        // refill on one side of it.
                        if self.bytes_remaining > 0 && self.pending_fetch.is_none() {
                            self.pending_fetch = Some(self.current_address);
                        }
                    }
                    None => self.silence = true,
                }
            }
        } else {
            self.timer_value -= 1;
        }
    }

    /// The address the channel wants read, if any. Clears the ask.
    fn take_fetch(&mut self) -> Option<u16> {
        self.pending_fetch.take()
    }

    /// The DMA's answer. A byte for a sample that was called off in
    /// the meantime changes nothing — there is no sample for it to
    /// belong to.
    fn supply(&mut self, value: u8) {
        if self.bytes_remaining == 0 {
            return;
        }
        self.buffer = Some(value);
        self.current_address = if self.current_address == 0xFFFF {
            0x8000
        } else {
            self.current_address + 1
        };
        self.bytes_remaining -= 1;
        if self.bytes_remaining == 0 {
            if self.loop_flag {
                self.restart();
            } else if self.irq_enabled {
                self.irq_pending.set(true);
            }
        }
    }

}
