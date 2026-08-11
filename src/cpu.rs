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
}
