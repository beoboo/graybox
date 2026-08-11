//! The 6502 CPU — the brain of the NES.

/// The CPU: a handful of small "pockets" (registers) and the memory it
/// reads and writes.
pub struct Cpu {
    /// A, the accumulator: the pocket where answers happen.
    pub a: u8,

    /// X, an index register: a helper pocket, good at counting.
    pub x: u8,

    /// Y, the other index register.
    pub y: u8,

    /// The program counter: the ADDRESS of the next instruction.
    /// The only 16-bit pocket, because addresses are 16 bits.
    pub pc: u16,

    /// The stack pointer — where in page one the next push lands.
    pub sp: u8,

    /// The status byte: eight tiny yes/no flags packed into one byte.
    pub status: u8,

    /// 64 KiB of memory — every address from $0000 to $FFFF.
    pub memory: [u8; 65536],
}

/// The Zero flag: switched on when the last value the CPU handled was zero.
pub const FLAG_ZERO: u8 = 0b0000_0010;

/// The Negative flag: a copy of the top bit — the sign bit — of the
/// last value the CPU handled.
pub const FLAG_NEGATIVE: u8 = 0b1000_0000;

impl Cpu {
    /// A brand-new CPU: every pocket empty, every byte of memory zero.
    pub fn new() -> Cpu {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            pc: 0,
            sp: 0,
            status: 0,
            memory: [0; 65536],
        }
    }

    /// Read the byte stored at an address.
    pub fn read(&self, address: u16) -> u8 {
        self.memory[address as usize]
    }

    /// Store a byte at an address.
    pub fn write(&mut self, address: u16, value: u8) {
        self.memory[address as usize] = value;
    }

    /// Read a two-byte number. The 6502 stores the LITTLE end first:
    /// the number $8000 sits in memory as the bytes $00, $80.
    pub fn read_word(&self, address: u16) -> u16 {
        let low = self.read(address) as u16;
        let high = self.read(address.wrapping_add(1)) as u16;

        // Slide the big half 8 bits to the left, then glue the halves.
        (high << 8) | low
    }

    /// Press the reset button.
    pub fn reset(&mut self) {
        self.a = 0;
        self.x = 0;
        self.y = 0;

        // The 6502's documented wake-up values, exactly as the real
        // chip's reset sequence leaves them.
        self.sp = 0xFD;
        self.status = 0b0010_0100;

        // The reset vector: the address stored AT $FFFC tells the CPU
        // where its program begins. The CPU's first act is to read it.
        self.pc = self.read_word(0xFFFC);
    }

    /// Run ONE instruction: fetch it, decode it, execute it.
    /// Returns `false` when the program says stop, `true` otherwise.
    pub fn step(&mut self) -> bool {
        // FETCH: read the next instruction's number (its "opcode")
        // and move the program counter past it.
        let opcode = self.read(self.pc);
        self.pc = self.pc.wrapping_add(1);

        // DECODE and EXECUTE: recognize the opcode, do what it says.
        match opcode {
            // LDA #value — LoaD A with the byte right after the opcode.
            0xA9 => {
                let value = self.read(self.pc);
                self.pc = self.pc.wrapping_add(1);
                self.a = value;
                self.update_zero_and_negative(self.a);
            }

            // TAX — Transfer A to X. A keeps its value; X gets a copy.
            0xAA => {
                self.x = self.a;
                self.update_zero_and_negative(self.x);
            }

            // INX — INcrement X: add 1. A pocket holds 0..=255, so
            // "wrapping" means 255 + 1 = 0, the way a clock's minutes
            // roll from 59 back to 00.
            0xE8 => {
                self.x = self.x.wrapping_add(1);
                self.update_zero_and_negative(self.x);
            }

            // BRK — simplified into a stop sign until interrupts arrive
            // (chapter 18). The real thing is more interesting.
            0x00 => return false,

            // An opcode we don't implement. Stopping loudly beats
            // carrying on wrongly.
            _ => panic!("I don't know opcode {opcode:#04X} yet!"),
        }

        true
    }

    /// Almost every instruction ends the same way: the CPU looks at the
    /// value it just produced and updates two flags about it.
    fn update_zero_and_negative(&mut self, value: u8) {
        if value == 0 {
            self.status |= FLAG_ZERO; // switch the bit on
        } else {
            self.status &= !FLAG_ZERO; // switch the bit off
        }

        if value & FLAG_NEGATIVE != 0 {
            self.status |= FLAG_NEGATIVE;
        } else {
            self.status &= !FLAG_NEGATIVE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_you_write_is_what_you_read() {
        let mut cpu = Cpu::new();
        cpu.write(0x0200, 42);
        assert_eq!(cpu.read(0x0200), 42);
    }

    #[test]
    fn words_are_stored_little_end_first() {
        let mut cpu = Cpu::new();
        cpu.write(0x0200, 0x34); // the little end comes first...
        cpu.write(0x0201, 0x12); // ...the big end second.
        assert_eq!(cpu.read_word(0x0200), 0x1234);
    }

    #[test]
    fn reset_starts_at_the_reset_vector() {
        let mut cpu = Cpu::new();
        cpu.write(0xFFFC, 0x00);
        cpu.write(0xFFFD, 0x80);

        cpu.reset();

        assert_eq!(cpu.pc, 0x8000);
        assert_eq!(cpu.a, 0);
        assert_eq!(cpu.x, 0);
        assert_eq!(cpu.y, 0);
        assert_eq!(cpu.sp, 0xFD);
        assert_eq!(cpu.status, 0b0010_0100);
    }

    /// Put a little program at $8000, point the reset vector at it,
    /// run it until BRK, and hand the CPU back for inspection.
    fn run_program(program: &[u8]) -> Cpu {
        let mut cpu = Cpu::new();
        for (i, byte) in program.iter().enumerate() {
            cpu.write(0x8000 + i as u16, *byte);
        }
        cpu.write(0xFFFC, 0x00);
        cpu.write(0xFFFD, 0x80);
        cpu.reset();

        while cpu.step() {}
        cpu
    }

    #[test]
    fn lda_loads_a_number_into_a() {
        let cpu = run_program(&[0xA9, 0x07, 0x00]);
        assert_eq!(cpu.a, 0x07);
    }

    #[test]
    fn lda_zero_switches_the_zero_flag_on() {
        let cpu = run_program(&[0xA9, 0x00, 0x00]);
        assert!(cpu.status & FLAG_ZERO != 0);
    }

    #[test]
    fn lda_top_bit_switches_the_negative_flag_on() {
        let cpu = run_program(&[0xA9, 0x80, 0x00]);
        assert!(cpu.status & FLAG_NEGATIVE != 0);
    }

    #[test]
    fn tax_copies_a_into_x() {
        let cpu = run_program(&[0xA9, 0x0A, 0xAA, 0x00]);
        assert_eq!(cpu.x, 0x0A);
        assert_eq!(cpu.a, 0x0A); // A keeps its copy
    }

    #[test]
    fn inx_adds_one_to_x() {
        let cpu = run_program(&[0xA9, 0x0A, 0xAA, 0xE8, 0x00]);
        assert_eq!(cpu.x, 0x0B);
    }

    #[test]
    fn inx_wraps_255_around_to_zero() {
        let cpu = run_program(&[0xA9, 0xFF, 0xAA, 0xE8, 0x00]);
        assert_eq!(cpu.x, 0);
        assert!(cpu.status & FLAG_ZERO != 0);
    }
}
