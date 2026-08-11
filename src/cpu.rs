//! The 6502 CPU — the brain of the NES.

use crate::bus::Bus;
use crate::cartridge::Cartridge;

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

    /// The bus: everything the CPU can reach lives on its far side.
    pub bus: Bus,
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

/// The Interrupt-disable flag: the CPU's "do not disturb" sign.
pub const FLAG_INTERRUPT_DISABLE: u8 = 0b0000_0100;

/// The Decimal flag. On most 6502s it switches ADC and SBC into
/// base-ten mode. The NES's chip lets you set it — and ignores it.
/// The flag flips; the math never changes. We are exactly that honest.
pub const FLAG_DECIMAL: u8 = 0b0000_1000;

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
    /// Zero page, then add Y.
    ZeroPageY,
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
    /// A CPU wired to a cartridge: pockets empty, RAM blank, the
    /// program waiting at the far end of the bus.
    pub fn new(cartridge: Cartridge) -> Cpu {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            pc: 0,
            sp: 0,
            status: 0,
            bus: Bus::new(cartridge),
        }
    }

    /// Read one byte — by asking the bus.
    pub fn read(&self, address: u16) -> u8 {
        self.bus.read(address)
    }

    /// Store one byte — by telling the bus.
    pub fn write(&mut self, address: u16, value: u8) {
        self.bus.write(address, value)
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
            // The Y twin — wrapping inside the page, same as X.
            AddressingMode::ZeroPageY => {
                let base = self.read(self.pc);
                self.pc = self.pc.wrapping_add(1);
                base.wrapping_add(self.y) as u16
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
            // pointer stored there. BOTH pointer bytes come from the
            // zero page: a pointer starting at $FF takes its second
            // half from $00, never $0100. nestest taught us this one.
            AddressingMode::IndirectX => {
                let base = self.read(self.pc);
                self.pc = self.pc.wrapping_add(1);
                let pointer = base.wrapping_add(self.x);
                let low = self.read(pointer as u16) as u16;
                let high = self.read(pointer.wrapping_add(1) as u16) as u16;
                (high << 8) | low
            }
            // ($xx),Y: follow the pointer at the zero-page spot — same
            // wrap — THEN add Y to wherever it led.
            AddressingMode::IndirectY => {
                let base = self.read(self.pc);
                self.pc = self.pc.wrapping_add(1);
                let low = self.read(base as u16) as u16;
                let high = self.read(base.wrapping_add(1) as u16) as u16;
                ((high << 8) | low).wrapping_add(self.y as u16)
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

            // RTI — ReTurn from Interrupt: pull the status notes back
            // (ghost-bit manners, as with PLP), then pull the address —
            // EXACTLY as saved, no +1. JSR pushes one short and RTS
            // compensates; an interrupt pushes the true address, so RTI
            // adds nothing. Two callers, two manners.
            0x40 => {
                let value = self.pull();
                self.status = (value | 0b0010_0000) & !0b0001_0000;
                self.pc = self.pull_word();
            }

            // LDX and LDY — every flavor each of them is sold in.
            0xA2 => self.ldx(AddressingMode::Immediate),
            0xA6 => self.ldx(AddressingMode::ZeroPage),
            0xB6 => self.ldx(AddressingMode::ZeroPageY),
            0xAE => self.ldx(AddressingMode::Absolute),
            0xBE => self.ldx(AddressingMode::AbsoluteY),

            0xA0 => self.ldy(AddressingMode::Immediate),
            0xA4 => self.ldy(AddressingMode::ZeroPage),
            0xB4 => self.ldy(AddressingMode::ZeroPageX),
            0xAC => self.ldy(AddressingMode::Absolute),
            0xBC => self.ldy(AddressingMode::AbsoluteX),

            // STX and STY.
            0x86 => self.stx(AddressingMode::ZeroPage),
            0x96 => self.stx(AddressingMode::ZeroPageY),
            0x8E => self.stx(AddressingMode::Absolute),

            0x84 => self.sty(AddressingMode::ZeroPage),
            0x94 => self.sty(AddressingMode::ZeroPageX),
            0x8C => self.sty(AddressingMode::Absolute),

            // TAY / TYA / TXA — the name spells the route: Transfer A
            // to Y, Transfer Y to A, Transfer X to A.
            0xA8 => {
                self.y = self.a;
                self.update_zero_and_negative(self.y);
            }
            0x98 => {
                self.a = self.y;
                self.update_zero_and_negative(self.a);
            }
            0x8A => {
                self.a = self.x;
                self.update_zero_and_negative(self.a);
            }

            // INC / DEC — counting directly in memory.
            0xE6 => self.modify(AddressingMode::ZeroPage, Cpu::inc_value),
            0xF6 => self.modify(AddressingMode::ZeroPageX, Cpu::inc_value),
            0xEE => self.modify(AddressingMode::Absolute, Cpu::inc_value),
            0xFE => self.modify(AddressingMode::AbsoluteX, Cpu::inc_value),

            0xC6 => self.modify(AddressingMode::ZeroPage, Cpu::dec_value),
            0xD6 => self.modify(AddressingMode::ZeroPageX, Cpu::dec_value),
            0xCE => self.modify(AddressingMode::Absolute, Cpu::dec_value),
            0xDE => self.modify(AddressingMode::AbsoluteX, Cpu::dec_value),

            // INY, DEY, DEX — INcrement and DEcrement: the index pockets
            // count both ways.
            0xC8 => {
                self.y = self.y.wrapping_add(1);
                self.update_zero_and_negative(self.y);
            }
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                self.update_zero_and_negative(self.y);
            }
            0xCA => {
                self.x = self.x.wrapping_sub(1);
                self.update_zero_and_negative(self.x);
            }

            // AND / ORA / EOR — the bit tools, promoted to instructions.
            0x29 => self.and(AddressingMode::Immediate),
            0x25 => self.and(AddressingMode::ZeroPage),
            0x35 => self.and(AddressingMode::ZeroPageX),
            0x2D => self.and(AddressingMode::Absolute),
            0x3D => self.and(AddressingMode::AbsoluteX),
            0x39 => self.and(AddressingMode::AbsoluteY),
            0x21 => self.and(AddressingMode::IndirectX),
            0x31 => self.and(AddressingMode::IndirectY),

            0x09 => self.ora(AddressingMode::Immediate),
            0x05 => self.ora(AddressingMode::ZeroPage),
            0x15 => self.ora(AddressingMode::ZeroPageX),
            0x0D => self.ora(AddressingMode::Absolute),
            0x1D => self.ora(AddressingMode::AbsoluteX),
            0x19 => self.ora(AddressingMode::AbsoluteY),
            0x01 => self.ora(AddressingMode::IndirectX),
            0x11 => self.ora(AddressingMode::IndirectY),

            0x49 => self.eor(AddressingMode::Immediate),
            0x45 => self.eor(AddressingMode::ZeroPage),
            0x55 => self.eor(AddressingMode::ZeroPageX),
            0x4D => self.eor(AddressingMode::Absolute),
            0x5D => self.eor(AddressingMode::AbsoluteX),
            0x59 => self.eor(AddressingMode::AbsoluteY),
            0x41 => self.eor(AddressingMode::IndirectX),
            0x51 => self.eor(AddressingMode::IndirectY),

            // BIT — the tester.
            0x24 => self.bit(AddressingMode::ZeroPage),
            0x2C => self.bit(AddressingMode::Absolute),

            // The shifts, on A directly...
            0x0A => self.a = self.asl_value(self.a),
            0x4A => self.a = self.lsr_value(self.a),
            0x2A => self.a = self.rol_value(self.a),
            0x6A => self.a = self.ror_value(self.a),

            // ...and on memory, through `modify`.
            0x06 => self.modify(AddressingMode::ZeroPage, Cpu::asl_value),
            0x16 => self.modify(AddressingMode::ZeroPageX, Cpu::asl_value),
            0x0E => self.modify(AddressingMode::Absolute, Cpu::asl_value),
            0x1E => self.modify(AddressingMode::AbsoluteX, Cpu::asl_value),

            0x46 => self.modify(AddressingMode::ZeroPage, Cpu::lsr_value),
            0x56 => self.modify(AddressingMode::ZeroPageX, Cpu::lsr_value),
            0x4E => self.modify(AddressingMode::Absolute, Cpu::lsr_value),
            0x5E => self.modify(AddressingMode::AbsoluteX, Cpu::lsr_value),

            0x26 => self.modify(AddressingMode::ZeroPage, Cpu::rol_value),
            0x36 => self.modify(AddressingMode::ZeroPageX, Cpu::rol_value),
            0x2E => self.modify(AddressingMode::Absolute, Cpu::rol_value),
            0x3E => self.modify(AddressingMode::AbsoluteX, Cpu::rol_value),

            0x66 => self.modify(AddressingMode::ZeroPage, Cpu::ror_value),
            0x76 => self.modify(AddressingMode::ZeroPageX, Cpu::ror_value),
            0x6E => self.modify(AddressingMode::Absolute, Cpu::ror_value),
            0x7E => self.modify(AddressingMode::AbsoluteX, Cpu::ror_value),

            // The rest of the flag switches — SEt or CLear, plus a
            // letter: SEI/CLI for Interrupt-disable, SED/CLD for
            // Decimal (the flag the NES ignores), CLV for oVerflow —
            // which nothing sets directly.
            0x78 => self.status |= FLAG_INTERRUPT_DISABLE,
            0x58 => self.status &= !FLAG_INTERRUPT_DISABLE,
            0xF8 => self.status |= FLAG_DECIMAL,
            0xD8 => self.status &= !FLAG_DECIMAL,
            0xB8 => self.status &= !FLAG_OVERFLOW,

            // TXS — Transfer X to the Stack pointer. The ONE transfer
            // that takes no notes: SP is plumbing, not data. A classic
            // trap.
            0x9A => self.sp = self.x,

            // TSX — Transfer the Stack pointer to X, notes and all.
            0xBA => {
                self.x = self.sp;
                self.update_zero_and_negative(self.x);
            }

            // JMP (indirect) — jump to wherever the pointer points.
            0x6C => self.jmp_indirect(),

            // NOP — No OPeration: do nothing, beautifully. Real programs
            // use it to fill space and to wait for exactly one moment.
            0xEA => {}

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

    /// LDX — LoaD X: LDA's twin for the X pocket.
    fn ldx(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        self.x = self.read(address);
        self.update_zero_and_negative(self.x);
    }

    /// LDY — LoaD Y: the same once more, for Y.
    fn ldy(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        self.y = self.read(address);
        self.update_zero_and_negative(self.y);
    }

    /// STX — STore X. Like STA, it takes no notes.
    fn stx(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        self.write(address, self.x);
    }

    /// STY — STore Y.
    fn sty(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        self.write(address, self.y);
    }

    /// Read-modify-write: fetch a value from memory, hand it to a worker
    /// function, store the worker's answer back where the value came
    /// from. The worker is a PARAMETER — in Rust, a plain function's
    /// name is a value you can pass around, like any other.
    fn modify(&mut self, mode: AddressingMode, worker: fn(&mut Cpu, u8) -> u8) {
        let address = self.operand_address(mode);
        let value = self.read(address);
        let result = worker(self, value);
        self.write(address, result);
    }

    /// INC's worker: one more, clock rules.
    fn inc_value(&mut self, value: u8) -> u8 {
        let result = value.wrapping_add(1);
        self.update_zero_and_negative(result);
        result
    }

    /// DEC's worker: one less, clock rules — 0 rolls back to 255.
    fn dec_value(&mut self, value: u8) -> u8 {
        let result = value.wrapping_sub(1);
        self.update_zero_and_negative(result);
        result
    }

    /// AND — keep only the bits A and the value share.
    fn and(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        let value = self.read(address);
        self.a &= value;
        self.update_zero_and_negative(self.a);
    }

    /// ORA — switch on every bit the value has on.
    fn ora(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        let value = self.read(address);
        self.a |= value;
        self.update_zero_and_negative(self.a);
    }

    /// EOR — exclusive or: flip every bit the value has on.
    fn eor(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        let value = self.read(address);
        self.a ^= value;
        self.update_zero_and_negative(self.a);
    }

    /// BIT — test BITs without touching A. Zero reports whether A and
    /// the value share any set bits; N and V become plain copies of the
    /// value's top two bits — which is why the flag masks fit.
    fn bit(&mut self, mode: AddressingMode) {
        let address = self.operand_address(mode);
        let value = self.read(address);

        if self.a & value == 0 {
            self.status |= FLAG_ZERO;
        } else {
            self.status &= !FLAG_ZERO;
        }
        if value & FLAG_NEGATIVE != 0 {
            self.status |= FLAG_NEGATIVE;
        } else {
            self.status &= !FLAG_NEGATIVE;
        }
        if value & FLAG_OVERFLOW != 0 {
            self.status |= FLAG_OVERFLOW;
        } else {
            self.status &= !FLAG_OVERFLOW;
        }
    }

    /// ASL's worker — Arithmetic Shift Left: every bit one slot up,
    /// a 0 in at the bottom, the old top bit out into Carry.
    /// Numerically: times two, ninth bit caught.
    fn asl_value(&mut self, value: u8) -> u8 {
        if value & 0x80 != 0 {
            self.status |= FLAG_CARRY;
        } else {
            self.status &= !FLAG_CARRY;
        }
        let result = value << 1;
        self.update_zero_and_negative(result);
        result
    }

    /// LSR's worker — Logical Shift Right: divide by two, the old
    /// bottom bit out into Carry.
    fn lsr_value(&mut self, value: u8) -> u8 {
        if value & 0x01 != 0 {
            self.status |= FLAG_CARRY;
        } else {
            self.status &= !FLAG_CARRY;
        }
        let result = value >> 1;
        self.update_zero_and_negative(result);
        result
    }

    /// ROL's worker — ROtate Left: like ASL, but the OLD Carry comes
    /// in at the bottom. Nine bits going around in a circle.
    fn rol_value(&mut self, value: u8) -> u8 {
        let old_carry = self.status & FLAG_CARRY != 0;
        if value & 0x80 != 0 {
            self.status |= FLAG_CARRY;
        } else {
            self.status &= !FLAG_CARRY;
        }
        let mut result = value << 1;
        if old_carry {
            result |= 0x01;
        }
        self.update_zero_and_negative(result);
        result
    }

    /// ROR's worker — the same circle, turning the other way.
    fn ror_value(&mut self, value: u8) -> u8 {
        let old_carry = self.status & FLAG_CARRY != 0;
        if value & 0x01 != 0 {
            self.status |= FLAG_CARRY;
        } else {
            self.status &= !FLAG_CARRY;
        }
        let mut result = value >> 1;
        if old_carry {
            result |= 0x80;
        }
        self.update_zero_and_negative(result);
        result
    }

    /// JMP's indirect flavor: follow a full 16-bit pointer — with the
    /// 6502's famous flaw. If the pointer starts at the END of a page
    /// ($xxFF), the chip fetches the second byte from the START of that
    /// same page, not from the next one. A mistake printed into millions
    /// of chips stops being a mistake: games rely on it, so it's the
    /// law, and we obey the law.
    fn jmp_indirect(&mut self) {
        let pointer = self.operand_address(AddressingMode::Absolute);

        let low = self.read(pointer) as u16;
        let high_address = if pointer & 0x00FF == 0x00FF {
            pointer & 0xFF00 // wrap to the page's own start
        } else {
            pointer + 1
        };
        let high = self.read(high_address) as u16;

        self.pc = (high << 8) | low;
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

    /// Every official opcode's name and total length in bytes — the
    /// decoder ring for the diary. `None` means unofficial.
    pub fn opcode_name_and_length(opcode: u8) -> Option<(&'static str, u16)> {
        Some(match opcode {
            0xA9 | 0xA5 | 0xB5 | 0xA1 | 0xB1 => ("LDA", 2),
            0xAD | 0xBD | 0xB9 => ("LDA", 3),
            0xA2 | 0xA6 | 0xB6 => ("LDX", 2),
            0xAE | 0xBE => ("LDX", 3),
            0xA0 | 0xA4 | 0xB4 => ("LDY", 2),
            0xAC | 0xBC => ("LDY", 3),

            0x85 | 0x95 | 0x81 | 0x91 => ("STA", 2),
            0x8D | 0x9D | 0x99 => ("STA", 3),
            0x86 | 0x96 => ("STX", 2),
            0x8E => ("STX", 3),
            0x84 | 0x94 => ("STY", 2),
            0x8C => ("STY", 3),

            0xAA => ("TAX", 1),
            0xA8 => ("TAY", 1),
            0x8A => ("TXA", 1),
            0x98 => ("TYA", 1),
            0x9A => ("TXS", 1),
            0xBA => ("TSX", 1),

            0x69 | 0x65 | 0x75 | 0x61 | 0x71 => ("ADC", 2),
            0x6D | 0x7D | 0x79 => ("ADC", 3),
            0xE9 | 0xE5 | 0xF5 | 0xE1 | 0xF1 => ("SBC", 2),
            0xED | 0xFD | 0xF9 => ("SBC", 3),

            0xC9 | 0xC5 | 0xD5 | 0xC1 | 0xD1 => ("CMP", 2),
            0xCD | 0xDD | 0xD9 => ("CMP", 3),
            0xE0 | 0xE4 => ("CPX", 2),
            0xEC => ("CPX", 3),
            0xC0 | 0xC4 => ("CPY", 2),
            0xCC => ("CPY", 3),

            0xE6 | 0xF6 => ("INC", 2),
            0xEE | 0xFE => ("INC", 3),
            0xC6 | 0xD6 => ("DEC", 2),
            0xCE | 0xDE => ("DEC", 3),
            0xE8 => ("INX", 1),
            0xC8 => ("INY", 1),
            0xCA => ("DEX", 1),
            0x88 => ("DEY", 1),

            0x29 | 0x25 | 0x35 | 0x21 | 0x31 => ("AND", 2),
            0x2D | 0x3D | 0x39 => ("AND", 3),
            0x09 | 0x05 | 0x15 | 0x01 | 0x11 => ("ORA", 2),
            0x0D | 0x1D | 0x19 => ("ORA", 3),
            0x49 | 0x45 | 0x55 | 0x41 | 0x51 => ("EOR", 2),
            0x4D | 0x5D | 0x59 => ("EOR", 3),
            0x24 => ("BIT", 2),
            0x2C => ("BIT", 3),

            0x0A => ("ASL", 1),
            0x06 | 0x16 => ("ASL", 2),
            0x0E | 0x1E => ("ASL", 3),
            0x4A => ("LSR", 1),
            0x46 | 0x56 => ("LSR", 2),
            0x4E | 0x5E => ("LSR", 3),
            0x2A => ("ROL", 1),
            0x26 | 0x36 => ("ROL", 2),
            0x2E | 0x3E => ("ROL", 3),
            0x6A => ("ROR", 1),
            0x66 | 0x76 => ("ROR", 2),
            0x6E | 0x7E => ("ROR", 3),

            0x4C | 0x6C => ("JMP", 3),
            0x20 => ("JSR", 3),
            0x60 => ("RTS", 1),
            0x40 => ("RTI", 1),

            0xD0 => ("BNE", 2),
            0xF0 => ("BEQ", 2),
            0x90 => ("BCC", 2),
            0xB0 => ("BCS", 2),
            0x10 => ("BPL", 2),
            0x30 => ("BMI", 2),
            0x50 => ("BVC", 2),
            0x70 => ("BVS", 2),

            0x48 => ("PHA", 1),
            0x68 => ("PLA", 1),
            0x08 => ("PHP", 1),
            0x28 => ("PLP", 1),

            0x18 => ("CLC", 1),
            0x38 => ("SEC", 1),
            0x58 => ("CLI", 1),
            0x78 => ("SEI", 1),
            0xD8 => ("CLD", 1),
            0xF8 => ("SED", 1),
            0xB8 => ("CLV", 1),

            0xEA => ("NOP", 1),
            0x00 => ("BRK", 1),

            _ => return None,
        })
    }

    /// One line of diary: where the CPU is, the instruction it is about
    /// to run, and the state of every pocket — before the deed.
    pub fn trace(&self) -> Option<String> {
        let opcode = self.read(self.pc);
        let (name, length) = Cpu::opcode_name_and_length(opcode)?;

        // The instruction's raw bytes, as many as it has.
        let mut bytes = String::new();
        for offset in 0..length {
            let byte = self.read(self.pc.wrapping_add(offset));
            bytes.push_str(&format!("{byte:02X} "));
        }

        Some(format!(
            "{:04X}  {:<9} {}  A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X}",
            self.pc, bytes, name, self.a, self.x, self.y, self.status, self.sp
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway cartridge for the robots: the program at $8000, the
    /// reset vector pointing at it.
    fn test_cartridge(program: &[u8]) -> Cartridge {
        let mut prg = vec![0u8; 16 * 1024];
        prg[..program.len()].copy_from_slice(program);
        prg[0x3FFC] = 0x00; // the reset vector: $8000, little end first
        prg[0x3FFD] = 0x80;
        Cartridge {
            prg_rom: prg,
            chr_rom: Vec::new(),
            mapper: 0,
        }
    }

    #[test]
    fn what_you_write_is_what_you_read() {
        let mut cpu = Cpu::new(test_cartridge(&[]));
        cpu.write(0x0200, 42);
        assert_eq!(cpu.read(0x0200), 42);
    }

    #[test]
    fn words_are_stored_little_end_first() {
        let mut cpu = Cpu::new(test_cartridge(&[]));
        cpu.write(0x0200, 0x34); // the little end comes first...
        cpu.write(0x0201, 0x12); // ...the big end second.
        assert_eq!(cpu.read_word(0x0200), 0x1234);
    }

    #[test]
    fn reset_starts_at_the_reset_vector() {
        // The vector is baked into the cartridge now — as in real life.
        let mut cpu = Cpu::new(test_cartridge(&[]));
        cpu.reset();

        assert_eq!(cpu.pc, 0x8000);
        assert_eq!(cpu.a, 0);
        assert_eq!(cpu.x, 0);
        assert_eq!(cpu.y, 0);
        assert_eq!(cpu.sp, 0xFD);
        assert_eq!(cpu.status, 0b0010_0100);
    }

    #[test]
    fn ram_echoes_every_two_kilobytes() {
        // One write, four addresses: the 2 KiB of RAM answers again at
        // $0800, $1000, and $1800.
        let mut cpu = Cpu::new(test_cartridge(&[]));
        cpu.write(0x0042, 0x99);
        assert_eq!(cpu.read(0x0042), 0x99);
        assert_eq!(cpu.read(0x0842), 0x99);
        assert_eq!(cpu.read(0x1042), 0x99);
        assert_eq!(cpu.read(0x1842), 0x99);
    }

    /// Run a program from a proper cartridge until BRK, and hand the
    /// CPU back for inspection.
    fn run_program(program: &[u8]) -> Cpu {
        let mut cpu = Cpu::new(test_cartridge(program));
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

    /// Like `run_program`, but first plant some values in RAM — the
    /// addressing modes need something to find.
    fn run_program_with(program: &[u8], plants: &[(u16, u8)]) -> Cpu {
        let mut cpu = Cpu::new(test_cartridge(program));
        for (address, value) in plants {
            cpu.write(*address, *value);
        }
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
        let mut cpu = Cpu::new(test_cartridge(&[0xB9, 0x00, 0x02, 0x00]));
        cpu.write(0x0205, 0x66);
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
        let mut cpu = Cpu::new(test_cartridge(&[0xB1, 0x20, 0x00]));
        cpu.write(0x0020, 0x00); // pointer, little end
        cpu.write(0x0021, 0x03); // pointer, big end
        cpu.write(0x0304, 0x99);
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

    #[test]
    fn rti_pulls_status_and_the_exact_address() {
        // Hand-build an interrupt's stack frame — PC $800A, then a
        // status byte — and RTI must land exactly there, notes
        // restored, no +1. The BRK at $800A catches the landing.
        let cpu = run_program(&[
            0xA9, 0x80, 0x48, // LDA #$80, PHA — address, big half
            0xA9, 0x0A, 0x48, // LDA #$0A, PHA — address, little half
            0xA9, 0x02, 0x48, // LDA #$02, PHA — a status with Z raised
            0x40, // RTI
            0x00, // $800A: BRK
        ]);
        assert_eq!(cpu.pc, 0x800B); // the BRK at $800A fetched, stopped
        assert!(cpu.status & FLAG_ZERO != 0); // the pulled Z came back
    }

    #[test]
    fn nestest_matches_the_golden_log() {
        // Runs only when both files sit in roms/ — the robots skip
        // politely on machines without them. (`let ... else`: destructure
        // or bail.)
        let (Ok(bytes), Ok(golden)) = (
            std::fs::read("roms/nestest.nes"),
            std::fs::read_to_string("roms/nestest.log"),
        ) else {
            return;
        };

        let mut cpu = Cpu::new(Cartridge::load(&bytes).unwrap());
        cpu.reset();
        cpu.pc = 0xC000;

        let mut matched = 0;
        for line in golden.lines() {
            let ours = format!(
                "{:04X} A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X}",
                cpu.pc, cpu.a, cpu.x, cpu.y, cpu.status, cpu.sp
            );
            let theirs = format!("{} {}", &line[0..4], &line[48..73]);
            assert_eq!(ours, theirs, "diverged after {matched} matching lines");
            matched += 1;

            if Cpu::opcode_name_and_length(cpu.read(cpu.pc)).is_none() {
                break; // the unofficial frontier — Part II territory
            }
            cpu.step();
        }
        assert!(
            matched >= 5004,
            "expected the whole official log, got {matched}"
        );
    }

    #[test]
    fn ldx_speaks_zero_page_y() {
        // LDY #2, then LDX $0E,Y reads from $0010.
        let cpu = run_program_with(&[0xA0, 0x02, 0xB6, 0x0E, 0x00], &[(0x0010, 0x33)]);
        assert_eq!(cpu.x, 0x33);
    }

    #[test]
    fn stx_and_sty_store_their_pockets() {
        // LDX #7, STX $10; LDY #8, STY $11.
        let cpu = run_program(&[0xA2, 0x07, 0x86, 0x10, 0xA0, 0x08, 0x84, 0x11, 0x00]);
        assert_eq!(cpu.read(0x0010), 7);
        assert_eq!(cpu.read(0x0011), 8);
    }

    #[test]
    fn tay_and_tya_round_trip() {
        // LDA #9, TAY, LDA #0, TYA — the 9 comes home through Y.
        let cpu = run_program(&[0xA9, 0x09, 0xA8, 0xA9, 0x00, 0x98, 0x00]);
        assert_eq!(cpu.a, 9);
    }

    #[test]
    fn dex_wraps_zero_around_to_255() {
        // LDX #0, DEX — clock rules, backwards.
        let cpu = run_program(&[0xA2, 0x00, 0xCA, 0x00]);
        assert_eq!(cpu.x, 0xFF);
        assert!(cpu.status & FLAG_NEGATIVE != 0);
    }

    #[test]
    fn inc_and_dec_count_in_memory() {
        // $10 starts at 41: two INCs and a DEC leave the answer.
        let cpu = run_program_with(&[0xE6, 0x10, 0xE6, 0x10, 0xC6, 0x10, 0x00], &[(0x0010, 41)]);
        assert_eq!(cpu.read(0x0010), 42);
    }

    #[test]
    fn and_ora_eor_do_bit_arithmetic() {
        // ((0x0C AND 0x0A) OR 0x01) EOR 0xFF = 0xF6.
        let cpu = run_program(&[0xA9, 0x0C, 0x29, 0x0A, 0x09, 0x01, 0x49, 0xFF, 0x00]);
        assert_eq!(cpu.a, 0xF6);
        assert!(cpu.status & FLAG_NEGATIVE != 0);
    }

    #[test]
    fn bit_reports_without_touching_a() {
        // A = 0 shares no bits with $C0; N and V copy the value's top bits.
        let cpu = run_program_with(&[0xA9, 0x00, 0x24, 0x10, 0x00], &[(0x0010, 0xC0)]);
        assert_eq!(cpu.a, 0); // untouched
        assert!(cpu.status & FLAG_ZERO != 0);
        assert!(cpu.status & FLAG_NEGATIVE != 0);
        assert!(cpu.status & FLAG_OVERFLOW != 0);
    }

    #[test]
    fn asl_doubles_and_catches_the_top_bit() {
        // LDA #$81, ASL A: 0x81 doubled is 0x02 with the ninth bit in Carry.
        let cpu = run_program(&[0xA9, 0x81, 0x0A, 0x00]);
        assert_eq!(cpu.a, 0x02);
        assert!(cpu.status & FLAG_CARRY != 0);
    }

    #[test]
    fn lsr_halves_and_catches_the_bottom_bit() {
        // LDA #5, LSR A: 2, and the odd bit lands in Carry.
        let cpu = run_program(&[0xA9, 0x05, 0x4A, 0x00]);
        assert_eq!(cpu.a, 0x02);
        assert!(cpu.status & FLAG_CARRY != 0);
    }

    #[test]
    fn rol_and_ror_turn_the_nine_bit_circle() {
        // SEC, LDA #0, ROL: the old Carry rolls in at the bottom.
        let cpu = run_program(&[0x38, 0xA9, 0x00, 0x2A, 0x00]);
        assert_eq!(cpu.a, 0x01);

        // SEC, LDA #0, ROR: same circle, other direction.
        let cpu = run_program(&[0x38, 0xA9, 0x00, 0x6A, 0x00]);
        assert_eq!(cpu.a, 0x80);
        assert!(cpu.status & FLAG_NEGATIVE != 0);
    }

    #[test]
    fn shifts_work_on_memory_too() {
        // ASL $10 twice: 1 becomes 4, in place.
        let cpu = run_program_with(&[0x06, 0x10, 0x06, 0x10, 0x00], &[(0x0010, 0x01)]);
        assert_eq!(cpu.read(0x0010), 0x04);
    }

    #[test]
    fn txs_moves_x_to_sp_and_tsx_back() {
        // LDX #5, TXS, LDX #0, TSX — SP remembers, X gets it back.
        let cpu = run_program(&[0xA2, 0x05, 0x9A, 0xA2, 0x00, 0xBA, 0x00]);
        assert_eq!(cpu.x, 5);
    }

    #[test]
    fn sei_sed_set_their_flags_and_clv_clears_overflow() {
        // SEI, SED — then stage an overflow and CLV it away.
        let cpu = run_program(&[0x78, 0xF8, 0xA9, 0x50, 0x69, 0x50, 0xB8, 0x00]);
        assert!(cpu.status & FLAG_INTERRUPT_DISABLE != 0);
        assert!(cpu.status & FLAG_DECIMAL != 0);
        assert!(cpu.status & FLAG_OVERFLOW == 0);
    }

    #[test]
    fn jmp_indirect_follows_the_pointer() {
        // The pointer at $0210 says $8005, where BRK waits; the LDA
        // at $8003 gets jumped over.
        let cpu = run_program_with(
            &[0x6C, 0x10, 0x02, 0xA9, 0x63, 0x00],
            &[(0x0210, 0x05), (0x0211, 0x80)],
        );
        assert_eq!(cpu.a, 0);
    }

    #[test]
    fn indirect_y_pointer_at_ff_wraps_inside_the_zero_page() {
        // The pointer's low half sits at $FF; its high half comes from
        // $00 — never $0100. A decoy waits on the wrong path.
        let cpu = run_program_with(
            &[0xB1, 0xFF, 0x00],
            &[
                (0x00FF, 0x00), // pointer, little half
                (0x0000, 0x03), // pointer, big half — the LAWFUL one
                (0x0100, 0x07), // the wrong big half, lying in wait
                (0x0300, 0x2A), // the prize, where the law leads
            ],
        );
        assert_eq!(cpu.a, 0x2A);
    }

    #[test]
    fn indirect_x_wraps_the_same_way() {
        // X = 1 via TAX; base $FE + 1 = pointer $FF: same wrap.
        let cpu = run_program_with(
            &[0xA9, 0x01, 0xAA, 0xA1, 0xFE, 0x00],
            &[(0x00FF, 0x00), (0x0000, 0x03), (0x0300, 0x55)],
        );
        assert_eq!(cpu.a, 0x55);
    }
    #[test]
    fn jmp_indirect_obeys_the_page_edge_law() {
        // Pointer at $02FF: low byte from $02FF, high byte from $0200 —
        // NOT $0300. The decoy now lives where only ROM can put it:
        // baked into the cartridge, at the address a wrong answer names.
        let mut cartridge = test_cartridge(&[0x6C, 0xFF, 0x02, 0xA9, 0x63, 0x00]);
        cartridge.prg_rom[0x1905] = 0xA9; // $9905: the decoy, LDA #$42...
        cartridge.prg_rom[0x1906] = 0x42; // ...which tells on a wrong jump.

        let mut cpu = Cpu::new(cartridge);
        cpu.write(0x02FF, 0x05);
        cpu.write(0x0200, 0x80);
        cpu.write(0x0300, 0x99); // the wrong high byte, lying in wait
        cpu.reset();
        while cpu.step() {}
        assert_eq!(cpu.a, 0);
    }
}
