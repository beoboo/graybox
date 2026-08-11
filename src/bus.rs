//! The bus — the wiring that decides which chip answers each address.

use crate::cartridge::Cartridge;

/// Everything on the far side of the CPU's read and write lines.
pub struct Bus {
    /// The console's own RAM: two kibibytes. All of it. It was 1983.
    pub ram: [u8; 2048],

    /// The game, occupying the top half of the address space.
    pub cartridge: Cartridge,
}

impl Bus {
    /// Wire a bus around a cartridge.
    pub fn new(cartridge: Cartridge) -> Bus {
        Bus {
            ram: [0; 2048],
            cartridge,
        }
    }

    /// One address in, one byte out — from whichever chip lives there.
    /// (A `..=` pattern matches a whole range of values at once.)
    pub fn read(&self, address: u16) -> u8 {
        match address {
            // The console's RAM — and its three echoes. The board only
            // decodes enough address wires to tell 2 KiB apart, so the
            // same bytes answer again at $0800, $1000, and $1800.
            0x0000..=0x1FFF => self.ram[address as usize % 2048],

            // The cartridge's program ROM.
            0x8000..=0xFFFF => self.cartridge.read_prg(address),

            // The picture chip's registers. Nobody home yet.
            0x2000..=0x3FFF => 0,

            // Sound and input. Also nobody.
            0x4000..=0x401F => 0,

            // Cartridge territory that NROM boards leave unwired.
            0x4020..=0x7FFF => 0,
        }
    }

    /// One address in, one byte delivered.
    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x1FFF => self.ram[address as usize % 2048] = value,

            // Writes to ROM change nothing. The clue is in the name.
            0x8000..=0xFFFF => {}

            // Everywhere else: nobody listening yet.
            _ => {}
        }
    }
}
