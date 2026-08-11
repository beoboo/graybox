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

/// The Carry flag: the ninth bit of the last addition — and, for
/// subtraction and comparing, the "no borrow needed" signal.
pub const FLAG_CARRY: u8 = 0b0000_0001;

/// The Zero flag: switched on when the last value the CPU handled was zero.
pub const FLAG_ZERO: u8 = 0b0000_0010;

/// The Negative flag: a copy of the top bit — the sign bit — of the
/// last value the CPU handled.
pub const FLAG_NEGATIVE: u8 = 0b1000_0000;

/// The Overflow flag: the addition's answer landed on the wrong side of
/// zero — a signed-arithmetic accident the Carry can't see.
pub const FLAG_OVERFLOW: u8 = 0b0100_0000;

/// The ways an instruction can say WHERE its value lives.
/// (`Clone, Copy` lets us hand modes around as freely as numbers.)
#[derive(Clone, Copy)]
pub enum AddressingMode {
    /// The value sits right after the opcode, in the program itself.
    Immediate,
    /// One byte names an address in the first 256 — the "zero page".
    ZeroPage,
    /// Zero page, then add X.
    ZeroPageX,
    /// A full two-byte address. Anywhere in memory.
    Absolute,
    /// Absolute, then add X.
    AbsoluteX,
    /// Absolute, then add Y.
    AbsoluteY,
    /// ($xx,X): add X to a zero-page spot; a POINTER waits there.
    IndirectX,
    /// ($xx),Y: a pointer waits at a zero-page spot; add Y afterwards.
    IndirectY,
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

    /// Where does this instruction's value live? Every addressing mode
    /// answers differently. This function does the finding, and moves PC
    /// past whatever bytes the mode used up.
    fn operand_address(&mut self, mode: AddressingMode) -> u16 {
        match mode {
            // The value's address IS where PC points. Note it, step past.
            AddressingMode::Immediate => {
                let address = self.pc;
                self.pc = self.pc.wrapping_add(1);
                address
            }
            // One byte, naming one of the first 256 addresses.
            AddressingMode::ZeroPage => {
                let address = self.read(self.pc) as u16;
                self.pc = self.pc.wrapping_add(1);
                address
            }
            // Zero page plus X — and the sum wraps WITHIN the page:
            // $FF + 2 lands on $01, never on $101.
            AddressingMode::ZeroPageX => {
                let base = self.read(self.pc);
                self.pc = self.pc.wrapping_add(1);
                base.wrapping_add(self.x) as u16
            }
            // Two bytes, little end first — a full address.
            AddressingMode::Absolute => {
                let address = self.read_word(self.pc);
                self.pc = self.pc.wrapping_add(2);
                address
            }
            // Absolute plus X: perfect for "element X of a list".
            AddressingMode::AbsoluteX => {
                let base = self.read_word(self.pc);
                self.pc = self.pc.wrapping_add(2);
                base.wrapping_add(self.x as u16)
            }
            // The same, with Y.
            AddressingMode::AbsoluteY => {
                let base = self.read_word(self.pc);
                self.pc = self.pc.wrapping_add(2);
                base.wrapping_add(self.y as u16)
            }
            // ($xx,X): add X to the zero-page spot, THEN follow the
            // pointer stored there.
            AddressingMode::IndirectX => {
                let base = self.read(self.pc);
                self.pc = self.pc.wrapping_add(1);
                let pointer = base.wrapping_add(self.x) as u16;
                self.read_word(pointer)
            }
            // ($xx),Y: follow the pointer at the zero-page spot, THEN
            // add Y to wherever it led.
            AddressingMode::IndirectY => {
                let base = self.read(self.pc) as u16;
                self.pc = self.pc.wrapping_add(1);
                self.read_word(base).wrapping_add(self.y as u16)
            }
        }
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
            // LDA, in every flavor the 6502 sells it.
            0xA9 => self.lda(AddressingMode::Immediate),
            0xA5 => self.lda(AddressingMode::ZeroPage),
            0xB5 => self.lda(AddressingMode::ZeroPageX),
            0xAD => self.lda(AddressingMode::Absolute),
            0xBD => self.lda(AddressingMode::AbsoluteX),
            0xB9 => self.lda(AddressingMode::AbsoluteY),
            0xA1 => self.lda(AddressingMode::IndirectX),
            0xB1 => self.lda(AddressingMode::IndirectY),

            // STA — STore A into memory. Same flavors, minus Immediate:
            // "store into the program itself" is not a thing the 6502 sells.
            0x85 => self.sta(AddressingMode::ZeroPage),
            0x95 => self.sta(AddressingMode::ZeroPageX),
            0x8D => self.sta(AddressingMode::Absolute),
            0x9D => self.sta(AddressingMode::AbsoluteX),
            0x99 => self.sta(AddressingMode::AbsoluteY),
            0x81 => self.sta(AddressingMode::IndirectX),
            0x91 => self.sta(AddressingMode::IndirectY),

            // ADC — add, in all the flavors LDA taught us.
            0x69 => self.adc(AddressingMode::Immediate),
            0x65 => self.adc(AddressingMode::ZeroPage),
            0x75 => self.adc(AddressingMode::ZeroPageX),
            0x6D => self.adc(AddressingMode::Absolute),
            0x7D => self.adc(AddressingMode::AbsoluteX),
            0x79 => self.adc(AddressingMode::AbsoluteY),
            0x61 => self.adc(AddressingMode::IndirectX),
            0x71 => self.adc(AddressingMode::IndirectY),

            // CLC / SEC — CLear and SEt the Carry by hand. Every fresh
            // chained addition starts CLC; every subtraction, SEC.
            0x18 => self.status &= !FLAG_CARRY,
            0x38 => self.status |= FLAG_CARRY,

            // SBC — subtract, same flavors again.
            0xE9 => self.sbc(AddressingMode::Immediate),
            0xE5 => self.sbc(AddressingMode::ZeroPage),
            0xF5 => self.sbc(AddressingMode::ZeroPageX),
            0xED => self.sbc(AddressingMode::Absolute),
            0xFD => self.sbc(AddressingMode::AbsoluteX),
            0xF9 => self.sbc(AddressingMode::AbsoluteY),
            0xE1 => self.sbc(AddressingMode::IndirectX),
            0xF1 => self.sbc(AddressingMode::IndirectY),

            // CMP — compare with A...
            0xC9 => self.compare(AddressingMode::Immediate, self.a),
            0xC5 => self.compare(AddressingMode::ZeroPage, self.a),
            0xD5 => self.compare(AddressingMode::ZeroPageX, self.a),
            0xCD => self.compare(AddressingMode::Absolute, self.a),
            0xDD => self.compare(AddressingMode::AbsoluteX, self.a),
            0xD9 => self.compare(AddressingMode::AbsoluteY, self.a),
            0xC1 => self.compare(AddressingMode::IndirectX, self.a),
            0xD1 => self.compare(AddressingMode::IndirectY, self.a),

            // ...CPX, with X...
            0xE0 => self.compare(AddressingMode::Immediate, self.x),
            0xE4 => self.compare(AddressingMode::ZeroPage, self.x),
            0xEC => self.compare(AddressingMode::Absolute, self.x),

            // ...and CPY, with Y.
            0xC0 => self.compare(AddressingMode::Immediate, self.y),
            0xC4 => self.compare(AddressingMode::ZeroPage, self.y),
            0xCC => self.compare(AddressingMode::Absolute, self.y),

            // JMP — JuMP: put a new address straight into PC. That is
            // all "going somewhere else" ever was.
            0x4C => self.pc = self.operand_address(AddressingMode::Absolute),

            // The branch family — Branch if...: each one tests a single
            // flag, one way.
            0xD0 => self.branch_if(self.status & FLAG_ZERO == 0), // BNE - not equal
            0xF0 => self.branch_if(self.status & FLAG_ZERO != 0), // BEQ - equal
            0x90 => self.branch_if(self.status & FLAG_CARRY == 0), // BCC - carry clear
            0xB0 => self.branch_if(self.status & FLAG_CARRY != 0), // BCS - carry set
            0x10 => self.branch_if(self.status & FLAG_NEGATIVE == 0), // BPL - plus
            0x30 => self.branch_if(self.status & FLAG_NEGATIVE != 0), // BMI - minus
            0x50 => self.branch_if(self.status & FLAG_OVERFLOW == 0), // BVC
            0x70 => self.branch_if(self.status & FLAG_OVERFLOW != 0), // BVS

            // PHA / PLA — PusH A onto the stack, PuLl it back.
            0x48 => self.push(self.a),
            0x68 => {
                self.a = self.pull();
                self.update_zero_and_negative(self.a);
            }

            // PHP / PLP — PusH and PuLl the Processor status itself,
            // notes and all. Two ghost bits ride along: bits 4 and 5 exist
            // only on pushed copies — PHP pushes them as ones, and PLP
            // politely ignores them.
            0x08 => self.push(self.status | 0b0011_0000),
            0x28 => {
                let value = self.pull();
                self.status = (value | 0b0010_0000) & !0b0001_0000;
            }

            // JSR — Jump to SubRoutine: leave a return note on the
            // stack, then go. The 6502's quirk: the note is the address
            // of JSR's own LAST byte — one short of the next
            // instruction. RTS knows, and adds the 1 back.
            0x20 => {
                let target = self.operand_address(AddressingMode::Absolute);
                self.push_word(self.pc.wrapping_sub(1));
                self.pc = target;
            }

            // RTS — ReTurn from Subroutine: read the note, add the
            // missing 1, resume as if nothing happened.
            0x60 => {
                let return_address = self.pull_word();
                self.pc = return_address.wrapping_add(1);
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

    /// LDA, any flavor: find the value, load it into A, take notes.
    fn lda(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        self.a = self.read(address);
        self.update_zero_and_negative(self.a);
    }

    /// STA, any flavor: find the address, store A there.
    /// No flags — storing changes memory, not the CPU's mood.
    fn sta(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        self.write(address, self.a);
    }

    /// ADC — ADd with Carry: the 6502's one and only addition.
    fn adc(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        let value = self.read(address);
        self.add_to_a(value);
    }

    /// The shared heart of addition and subtraction: add a value into A
    /// and take all the notes.
    fn add_to_a(&mut self, value: u8) {
        // The Carry from last time joins the sum — that's what lets
        // additions chain into numbers bigger than one byte.
        let carry_in: u16 = if self.status & FLAG_CARRY != 0 { 1 } else { 0 };
        let sum = self.a as u16 + value as u16 + carry_in;

        // The answer's ninth bit doesn't fit an 8-bit pocket:
        // it lands in the Carry flag.
        if sum > 0xFF {
            self.status |= FLAG_CARRY;
        } else {
            self.status &= !FLAG_CARRY;
        }

        let result = sum as u8;

        // Overflow: both inputs sat on one side of zero and the answer
        // landed on the other. The top bit is the sign, so: if A and the
        // value agreed with each other and the result disagrees, that's
        // an accident worth a flag.
        if (self.a ^ result) & (value ^ result) & 0x80 != 0 {
            self.status |= FLAG_OVERFLOW;
        } else {
            self.status &= !FLAG_OVERFLOW;
        }

        self.a = result;
        self.update_zero_and_negative(self.a);
    }

    /// SBC — SuBtract with Carry. Subtraction is addition in disguise:
    /// flip every bit of the value and add it. The missing "+1" of the
    /// disguise comes in through the Carry — which is why every fresh
    /// subtraction starts with SEC.
    fn sbc(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        let value = self.read(address);
        self.add_to_a(value ^ 0xFF);
    }

    /// CMP — CoMPare A — with CPX and CPY for the other pockets: a
    /// subtraction that throws the answer away and keeps only the
    /// notes. Carry means "no borrow was needed" — in other words,
    /// register >= value.
    fn compare(&mut self, mode: AddressingMode, register: u8) {
        let address = self.operand_address(mode);
        let value = self.read(address);

        if register >= value {
            self.status |= FLAG_CARRY;
        } else {
            self.status &= !FLAG_CARRY;
        }
        self.update_zero_and_negative(register.wrapping_sub(value));
    }

    /// The heart of every branch: maybe move PC, by a SIGNED offset.
    /// (`as i8` reads the byte as -128..=127; the second `as` stretches
    /// it back to 16 bits, minus sign and all.)
    fn branch_if(&mut self, condition: bool) {
        let offset = self.read(self.pc) as i8;
        self.pc = self.pc.wrapping_add(1);

        if condition {
            self.pc = self.pc.wrapping_add(offset as u16);
        }
    }

    /// Push one byte onto the stack. The stack lives in page one,
    /// $0100-$01FF; SP points at the NEXT free slot; and the pile grows
    /// DOWNWARD - push writes, then steps down.
    fn push(&mut self, value: u8) {
        self.write(0x0100 + self.sp as u16, value);
        self.sp = self.sp.wrapping_sub(1);
    }

    /// Pull one byte back off the stack: step up, then read.
    /// (Most of the world says "pop"; the 6502 says "pull".)
    fn pull(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        self.read(0x0100 + self.sp as u16)
    }

    /// Push a two-byte value: big half first, so that in memory it lies
    /// little end first - the way the 6502 likes its addresses.
    fn push_word(&mut self, value: u16) {
        self.push((value >> 8) as u8);
        self.push(value as u8);
    }

    /// Pull a two-byte value pushed by `push_word`.
    fn pull_word(&mut self) -> u16 {
        let low = self.pull() as u16;
        let high = self.pull() as u16;
        (high << 8) | low
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

    /// Like `run_program`, but first plant some values in memory —
    /// the addressing modes need something to find.
    fn run_program_with(program: &[u8], plants: &[(u16, u8)]) -> Cpu {
        let mut cpu = Cpu::new();
        for (i, byte) in program.iter().enumerate() {
            cpu.write(0x8000 + i as u16, *byte);
        }
        for (address, value) in plants {
            cpu.write(*address, *value);
        }
        cpu.write(0xFFFC, 0x00);
        cpu.write(0xFFFD, 0x80);
        cpu.reset();

        while cpu.step() {}
        cpu
    }

    #[test]
    fn lda_zero_page_reads_from_the_first_page() {
        // LDA $10 — the byte planted at $0010 lands in A.
        let cpu = run_program_with(&[0xA5, 0x10, 0x00], &[(0x0010, 0x2A)]);
        assert_eq!(cpu.a, 0x2A);
    }

    #[test]
    fn lda_zero_page_x_wraps_inside_the_page() {
        // X = 2 (via TAX), then LDA $FF,X — $FF + 2 wraps to $01.
        let cpu = run_program_with(&[0xA9, 0x02, 0xAA, 0xB5, 0xFF, 0x00], &[(0x0001, 0x77)]);
        assert_eq!(cpu.a, 0x77);
    }

    #[test]
    fn lda_absolute_reads_anywhere() {
        // LDA $0234.
        let cpu = run_program_with(&[0xAD, 0x34, 0x02, 0x00], &[(0x0234, 0x55)]);
        assert_eq!(cpu.a, 0x55);
    }

    #[test]
    fn lda_absolute_x_adds_x() {
        // A = X = 5 via TAX, then LDA $0200,X reads $0205.
        let cpu = run_program_with(
            &[0xA9, 0x05, 0xAA, 0xBD, 0x00, 0x02, 0x00],
            &[(0x0205, 0x11)],
        );
        assert_eq!(cpu.a, 0x11);
    }

    #[test]
    fn lda_absolute_y_adds_y() {
        // The test sets Y by hand — robots may reach anywhere.
        let mut cpu = Cpu::new();
        for (i, byte) in [0xB9, 0x00, 0x02, 0x00].iter().enumerate() {
            cpu.write(0x8000 + i as u16, *byte);
        }
        cpu.write(0x0205, 0x66);
        cpu.write(0xFFFC, 0x00);
        cpu.write(0xFFFD, 0x80);
        cpu.reset();
        cpu.y = 5;
        while cpu.step() {}
        assert_eq!(cpu.a, 0x66);
    }

    #[test]
    fn lda_indirect_x_adds_x_then_follows_the_pointer() {
        // X = 4 via TAX; ($1C,X) means the pointer waits at $20,
        // and the pointer says $0300.
        let cpu = run_program_with(
            &[0xA9, 0x04, 0xAA, 0xA1, 0x1C, 0x00],
            &[(0x0020, 0x00), (0x0021, 0x03), (0x0300, 0x88)],
        );
        assert_eq!(cpu.a, 0x88);
    }

    #[test]
    fn lda_indirect_y_follows_the_pointer_then_adds_y() {
        // The pointer at $20/$21 says $0300; Y adds 4; $0304 holds the prize.
        let mut cpu = Cpu::new();
        for (i, byte) in [0xB1, 0x20, 0x00].iter().enumerate() {
            cpu.write(0x8000 + i as u16, *byte);
        }
        cpu.write(0x0020, 0x00); // pointer, little end
        cpu.write(0x0021, 0x03); // pointer, big end
        cpu.write(0x0304, 0x99);
        cpu.write(0xFFFC, 0x00);
        cpu.write(0xFFFD, 0x80);
        cpu.reset();
        cpu.y = 4;
        while cpu.step() {}
        assert_eq!(cpu.a, 0x99);
    }

    #[test]
    fn sta_stores_a_into_memory() {
        // LDA #$42, STA $0250.
        let cpu = run_program(&[0xA9, 0x42, 0x8D, 0x50, 0x02, 0x00]);
        assert_eq!(cpu.read(0x0250), 0x42);
    }

    #[test]
    fn adc_adds() {
        // LDA #2, ADC #3. Carry starts clear after reset.
        let cpu = run_program(&[0xA9, 0x02, 0x69, 0x03, 0x00]);
        assert_eq!(cpu.a, 5);
        assert!(cpu.status & FLAG_CARRY == 0);
    }

    #[test]
    fn adc_catches_the_ninth_bit() {
        // LDA #$FF, ADC #1 — the answer is 256: too big for the pocket.
        let cpu = run_program(&[0xA9, 0xFF, 0x69, 0x01, 0x00]);
        assert_eq!(cpu.a, 0);
        assert!(cpu.status & FLAG_CARRY != 0);
        assert!(cpu.status & FLAG_ZERO != 0);
    }

    #[test]
    fn adc_lets_the_carry_back_in() {
        // SEC first: 2 + 3 + 1 = 6.
        let cpu = run_program(&[0x38, 0xA9, 0x02, 0x69, 0x03, 0x00]);
        assert_eq!(cpu.a, 6);
    }

    #[test]
    fn adc_flags_a_signed_accident() {
        // 80 + 80 = 160 — but as SIGNED bytes, 80 + 80 can't be -96.
        let cpu = run_program(&[0xA9, 0x50, 0x69, 0x50, 0x00]);
        assert_eq!(cpu.a, 0xA0);
        assert!(cpu.status & FLAG_OVERFLOW != 0);
    }

    #[test]
    fn sbc_subtracts() {
        // SEC, LDA #9, SBC #4. Carry still set: no borrow was needed.
        let cpu = run_program(&[0x38, 0xA9, 0x09, 0xE9, 0x04, 0x00]);
        assert_eq!(cpu.a, 5);
        assert!(cpu.status & FLAG_CARRY != 0);
    }

    #[test]
    fn sbc_below_zero_wraps_and_drops_the_carry() {
        // SEC, LDA #4, SBC #9: 4 - 9 = -5, which a pocket shows as $FB.
        let cpu = run_program(&[0x38, 0xA9, 0x04, 0xE9, 0x09, 0x00]);
        assert_eq!(cpu.a, 0xFB);
        assert!(cpu.status & FLAG_CARRY == 0); // a borrow was needed
        assert!(cpu.status & FLAG_NEGATIVE != 0);
    }

    #[test]
    fn cmp_equal_sets_zero_and_carry() {
        // LDA #7, CMP #7.
        let cpu = run_program(&[0xA9, 0x07, 0xC9, 0x07, 0x00]);
        assert!(cpu.status & FLAG_ZERO != 0);
        assert!(cpu.status & FLAG_CARRY != 0);
        assert_eq!(cpu.a, 7); // comparing never changes A
    }

    #[test]
    fn cmp_smaller_clears_the_carry() {
        // LDA #4, CMP #9 — a borrow would be needed.
        let cpu = run_program(&[0xA9, 0x04, 0xC9, 0x09, 0x00]);
        assert!(cpu.status & FLAG_CARRY == 0);
    }

    #[test]
    fn jmp_skips_what_it_jumps_over() {
        // JMP $8005 leaps over LDA #$63; A stays 0.
        let cpu = run_program(&[0x4C, 0x05, 0x80, 0xA9, 0x63, 0x00]);
        assert_eq!(cpu.a, 0);
    }

    #[test]
    fn beq_branches_when_zero_is_set() {
        // LDA #0 sets Z; BEQ +2 leaps over LDA #$63.
        let cpu = run_program(&[0xA9, 0x00, 0xF0, 0x02, 0xA9, 0x63, 0x00]);
        assert_eq!(cpu.a, 0);
    }

    #[test]
    fn a_loop_counts_to_three() {
        // A loop: INX / CPX #3 / BNE, round and round until X hits 3.
        let cpu = run_program(&[0xA9, 0x00, 0xAA, 0xE8, 0xE0, 0x03, 0xD0, 0xFB, 0x00]);
        assert_eq!(cpu.x, 3);
    }

    #[test]
    fn pha_pla_round_trip() {
        // LDA #7, PHA, LDA #0, PLA — the 7 survives the trip.
        let cpu = run_program(&[0xA9, 0x07, 0x48, 0xA9, 0x00, 0x68, 0x00]);
        assert_eq!(cpu.a, 7);
    }

    #[test]
    fn the_stack_is_last_in_first_out() {
        // Push 1, push 2 — then the pulls give 2 first, 1 second.
        let cpu = run_program(&[0xA9, 0x01, 0x48, 0xA9, 0x02, 0x48, 0x68, 0xAA, 0x68, 0x00]);
        assert_eq!(cpu.x, 2); // first pull (parked in X)
        assert_eq!(cpu.a, 1); // second pull
    }

    #[test]
    fn the_stack_lives_in_page_one() {
        // One push: the byte lands at $0100 + SP's old value ($FD),
        // and SP steps down to $FC.
        let cpu = run_program(&[0xA9, 0x07, 0x48, 0x00]);
        assert_eq!(cpu.read(0x01FD), 7);
        assert_eq!(cpu.sp, 0xFC);
    }

    #[test]
    fn jsr_notes_the_address_of_its_own_last_byte() {
        // JSR $8004 jumps to a BRK. The pushed note reads $8002 —
        // one short of the next instruction at $8003. RTS compensates.
        let cpu = run_program(&[0x20, 0x04, 0x80, 0x00, 0x00]);
        assert_eq!(cpu.read(0x01FD), 0x80); // big half
        assert_eq!(cpu.read(0x01FC), 0x02); // little half
    }

    #[test]
    fn jsr_and_rts_come_back_twice() {
        // A helper that adds 10, called twice — each RTS finds its own
        // way home.
        let cpu = run_program(&[
            0xA9, 0x05, 0x20, 0x09, 0x80, 0x20, 0x09, 0x80, 0x00, 0x18, 0x69, 0x0A, 0x60,
        ]);
        assert_eq!(cpu.a, 25);
    }

    #[test]
    fn php_and_plp_carry_the_notes_across() {
        // LDA #0 raises Zero; PHP saves the notes; LDA #1 clears it;
        // PLP brings the saved notes back.
        let cpu = run_program(&[0xA9, 0x00, 0x08, 0xA9, 0x01, 0x28, 0x00]);
        assert!(cpu.status & FLAG_ZERO != 0);
    }

    #[test]
    fn php_pushes_the_two_ghost_bits_as_ones() {
        // The pushed copy of status always shows bits 4 and 5 set.
        let cpu = run_program(&[0x08, 0x00]);
        assert_eq!(cpu.read(0x01FD) & 0b0011_0000, 0b0011_0000);
    }
}
