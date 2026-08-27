//! Reading a program without running it: bytes turned back into the
//! mnemonics and operands of chapter 9.

use crate::cpu::Cpu;

/// How an instruction spells its operand — the assembler's view of the
/// addressing modes, including the three the executor never needed a
/// name for: no operand at all, the accumulator, and a branch's offset.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Syntax {
    /// `TAX` — everything is in the opcode.
    Implied,
    /// `LSR A` — the accumulator, named.
    Accumulator,
    /// `LDA #$10`
    Immediate,
    /// `LDA $10`
    ZeroPage,
    /// `LDA $10,X`
    ZeroPageX,
    /// `LDX $10,Y`
    ZeroPageY,
    /// `LDA $1234`
    Absolute,
    /// `LDA $1234,X`
    AbsoluteX,
    /// `LDA $1234,Y`
    AbsoluteY,
    /// `JMP ($1234)` — the one instruction that jumps through a pointer.
    Indirect,
    /// `LDA ($10,X)`
    IndirectX,
    /// `LDA ($10),Y`
    IndirectY,
    /// `BNE $C010` — one signed byte of distance, shown as where it lands.
    Relative,
}

impl Syntax {
    /// Bytes after the opcode.
    pub fn operand_bytes(self) -> u16 {
        match self {
            Syntax::Implied | Syntax::Accumulator => 0,
            Syntax::Immediate
            | Syntax::ZeroPage
            | Syntax::ZeroPageX
            | Syntax::ZeroPageY
            | Syntax::IndirectX
            | Syntax::IndirectY
            | Syntax::Relative => 1,
            Syntax::Absolute | Syntax::AbsoluteX | Syntax::AbsoluteY | Syntax::Indirect => 2,
        }
    }
}

/// The spelling of every one of the 256 opcodes, laid out by mode: the
/// opcode map of chapter 9 read column by column, the unofficial rows
/// of chapter 32 included.
pub fn syntax(opcode: u8) -> Syntax {
    match opcode {
        // The shifts and rotates that work on A itself.
        0x0A | 0x2A | 0x4A | 0x6A => Syntax::Accumulator,

        // A value in the program: the column-9 loads and arithmetic,
        // the compares, the unofficial immediates.
        0x09 | 0x29 | 0x49 | 0x69 | 0xA9 | 0xC9 | 0xE9 | 0xA0 | 0xA2 | 0xC0 | 0xE0 | 0x0B
        | 0x2B | 0x4B | 0x6B | 0x80 | 0x82 | 0x89 | 0x8B | 0xAB | 0xC2 | 0xCB | 0xE2 | 0xEB => {
            Syntax::Immediate
        }

        // The zero page: columns 4 to 7 of the map.
        0x04 | 0x05 | 0x06 | 0x07 | 0x24 | 0x25 | 0x26 | 0x27 | 0x44 | 0x45 | 0x46 | 0x47
        | 0x64 | 0x65 | 0x66 | 0x67 | 0x84 | 0x85 | 0x86 | 0x87 | 0xA4 | 0xA5 | 0xA6 | 0xA7
        | 0xC4 | 0xC5 | 0xC6 | 0xC7 | 0xE4 | 0xE5 | 0xE6 | 0xE7 => Syntax::ZeroPage,

        // Zero page plus X: columns 4 to 7 again, one row down — except
        // the four that index by Y, because X is busy being the value.
        0x14 | 0x15 | 0x16 | 0x17 | 0x34 | 0x35 | 0x36 | 0x37 | 0x54 | 0x55 | 0x56 | 0x57
        | 0x74 | 0x75 | 0x76 | 0x77 | 0x94 | 0x95 | 0xB4 | 0xB5 | 0xD4 | 0xD5 | 0xD6 | 0xD7
        | 0xF4 | 0xF5 | 0xF6 | 0xF7 => Syntax::ZeroPageX,
        0x96 | 0x97 | 0xB6 | 0xB7 => Syntax::ZeroPageY,

        // Anywhere: columns C to F.
        0x0C | 0x0D | 0x0E | 0x0F | 0x20 | 0x2C | 0x2D | 0x2E | 0x2F | 0x4C | 0x4D | 0x4E
        | 0x4F | 0x6D | 0x6E | 0x6F | 0x8C | 0x8D | 0x8E | 0x8F | 0xAC | 0xAD | 0xAE | 0xAF
        | 0xCC | 0xCD | 0xCE | 0xCF | 0xEC | 0xED | 0xEE | 0xEF => Syntax::Absolute,
        0x6C => Syntax::Indirect,

        // Anywhere plus X, and the handful plus Y.
        0x1C | 0x1D | 0x1E | 0x1F | 0x3C | 0x3D | 0x3E | 0x3F | 0x5C | 0x5D | 0x5E | 0x5F
        | 0x7C | 0x7D | 0x7E | 0x7F | 0x9C | 0x9D | 0xBC | 0xBD | 0xDC | 0xDD | 0xDE | 0xDF
        | 0xFC | 0xFD | 0xFE | 0xFF => Syntax::AbsoluteX,
        0x19 | 0x1B | 0x39 | 0x3B | 0x59 | 0x5B | 0x79 | 0x7B | 0x99 | 0x9B | 0x9E | 0x9F
        | 0xB9 | 0xBB | 0xBE | 0xBF | 0xD9 | 0xDB | 0xF9 | 0xFB => Syntax::AbsoluteY,

        // Through a zero-page pointer: column 1 plus X, column 1 of the
        // next row plus Y, and their unofficial twins in column 3.
        0x01 | 0x03 | 0x21 | 0x23 | 0x41 | 0x43 | 0x61 | 0x63 | 0x81 | 0x83 | 0xA1 | 0xA3
        | 0xC1 | 0xC3 | 0xE1 | 0xE3 => Syntax::IndirectX,
        0x11 | 0x13 | 0x31 | 0x33 | 0x51 | 0x53 | 0x71 | 0x73 | 0x91 | 0x93 | 0xB1 | 0xB3
        | 0xD1 | 0xD3 | 0xF1 | 0xF3 => Syntax::IndirectY,

        // The eight branches, column 0 of the odd rows.
        0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xB0 | 0xD0 | 0xF0 => Syntax::Relative,

        // Everything else stands alone: the transfers, the flag
        // instructions, the stack, the returns, the NOPs — and the jams.
        _ => Syntax::Implied,
    }
}

/// The instruction at `address`, as text, and how many bytes it takes —
/// so the next one can be found. Memory the listing must not touch, like
/// the picture chip's registers, disassembles as `??`.
pub fn disassemble(cpu: &Cpu, address: u16) -> (String, u16) {
    let opcode = match cpu.peek(address) {
        Some(opcode) => opcode,
        None => return ("??".to_string(), 1),
    };
    let name = match Cpu::opcode_name_and_length(opcode) {
        Some((name, _)) => name,
        None => return ("???".to_string(), 1),
    };
    let length = syntax(opcode).operand_bytes() + 1;

    let byte = |offset: u16| cpu.peek(address.wrapping_add(offset)).unwrap_or(0);
    let low = byte(1);
    let word = u16::from_le_bytes([byte(1), byte(2)]);

    let operand = match syntax(opcode) {
        Syntax::Implied => String::new(),
        Syntax::Accumulator => "A".to_string(),
        Syntax::Immediate => format!("#${low:02X}"),
        Syntax::ZeroPage => format!("${low:02X}"),
        Syntax::ZeroPageX => format!("${low:02X},X"),
        Syntax::ZeroPageY => format!("${low:02X},Y"),
        Syntax::Absolute => format!("${word:04X}"),
        Syntax::AbsoluteX => format!("${word:04X},X"),
        Syntax::AbsoluteY => format!("${word:04X},Y"),
        Syntax::Indirect => format!("(${word:04X})"),
        Syntax::IndirectX => format!("(${low:02X},X)"),
        Syntax::IndirectY => format!("(${low:02X}),Y"),
        // The byte is a signed distance from the instruction after
        // the branch; the reader wants the destination, not the trip.
        Syntax::Relative => {
            let target = address.wrapping_add(2).wrapping_add(low as i8 as u16);
            format!("${target:04X}")
        }
    };

    if operand.is_empty() {
        (name.to_string(), length)
    } else {
        (format!("{name} {operand}"), length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;

    /// A machine with a program at $8000 and nothing else to do.
    fn machine_with(program: &[u8]) -> Cpu {
        let mut prg = vec![0u8; 16 * 1024];
        prg[..program.len()].copy_from_slice(program);
        prg[0x3FFC] = 0x00;
        prg[0x3FFD] = 0x80;

        let mut rom = vec![b'N', b'E', b'S', 0x1A, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        rom.extend(prg);
        Cpu::new(Cartridge::load(&rom).unwrap())
    }

    /// The two tables were written separately, in different chapters,
    /// and must agree on every opcode's length.
    #[test]
    fn every_named_opcode_agrees_on_its_length() {
        for opcode in 0..=255u8 {
            if let Some((name, length)) = Cpu::opcode_name_and_length(opcode) {
                assert_eq!(
                    syntax(opcode).operand_bytes() + 1,
                    length,
                    "opcode {opcode:02X} ({name}) spells {:?}",
                    syntax(opcode)
                );
            }
        }
    }

    #[test]
    fn every_mode_has_its_spelling() {
        let cpu = machine_with(&[
            0xBD, 0x34, 0x12, // LDA $1234,X
            0x10, 0xFB, //       BPL $8000
            0x6C, 0x00, 0x80, // JMP ($8000)
            0x4A, //             LSR A
            0xB6, 0x10, //       LDX $10,Y
            0xB1, 0x20, //       LDA ($20),Y
            0xEA, //             NOP
        ]);
        let mut address = 0x8000;
        let mut listing = Vec::new();
        for _ in 0..7 {
            let (text, length) = disassemble(&cpu, address);
            listing.push(text);
            address += length;
        }
        assert_eq!(
            listing,
            [
                "LDA $1234,X",
                "BPL $8000",
                "JMP ($8000)",
                "LSR A",
                "LDX $10,Y",
                "LDA ($20),Y",
                "NOP"
            ]
        );
    }

    #[test]
    fn a_branch_shows_where_it_lands_not_how_far() {
        let cpu = machine_with(&[0xEA, 0xD0, 0x03]); // NOP; BNE +3
        assert_eq!(disassemble(&cpu, 0x8001).0, "BNE $8006");
    }

    #[test]
    fn register_space_is_not_guessed_at() {
        let cpu = machine_with(&[]);
        assert_eq!(disassemble(&cpu, 0x2002), ("??".to_string(), 1));
    }
}
