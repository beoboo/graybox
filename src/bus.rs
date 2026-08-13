//! The bus — the wiring that decides which chip answers each address.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::clock::Clock;
use crate::controller::Controller;
use crate::ppu::Ppu;

/// Everything on the far side of the CPU's read and write lines.
pub struct Bus {
    /// The console's own RAM: two kibibytes. All of it. It was 1983.
    pub ram: [u8; 2048],

    /// The game, occupying the top half of the address space.
    pub cartridge: Cartridge,

    /// The picture chip, reachable through its eight registers.
    pub ppu: Ppu,

    /// The first controller port.
    pub controller: Controller,

    /// The sound chip.
    pub apu: Apu,

    /// The metronome: where the beam is, dot by dot.
    pub clock: Clock,
}

impl Bus {
    /// Wire a bus around a cartridge.
    pub fn new(cartridge: Cartridge) -> Bus {
        Bus {
            ram: [0; 2048],
            // Built before `cartridge` moves in, since it reads from it.
            ppu: Ppu::new(cartridge.vertical_mirroring),
            controller: Controller::new(),
            apu: Apu::new(),
            clock: Clock::new(),
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

            // The picture chip's eight registers, echoed every eight
            // bytes to the top of the range. Two answer reads — and
            // both change state when looked at.
            0x2000..=0x3FFF => match address & 0x0007 {
                0x0002 => self.ppu.read_status(),
                0x0007 => self.ppu.read_data(&self.cartridge.chr_rom),
                0x0004 => self.ppu.read_oam_data(),
                _ => 0,
            },

            // The first controller: one button per read.
            0x4016 => self.controller.read(),

            // Sound registers, and the second controller port. Nobody.
            0x4000..=0x401F => 0,

            // Cartridge territory that NROM boards leave unwired.
            0x4020..=0x7FFF => 0,
        }
    }

    /// One address in, one byte delivered.
    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x1FFF => self.ram[address as usize % 2048] = value,

            // The picture chip's registers, for the writes games lean
            // on: settings, the address, the data.
            0x2000..=0x3FFF => match address & 0x0007 {
                0x0000 => self.ppu.write_ctrl(value),
                0x0006 => self.ppu.write_address(value),
                0x0007 => self.ppu.write_data(value),
                0x0001 => self.ppu.mask = value,
                0x0005 => self.ppu.write_scroll(value),
                0x0003 => self.ppu.write_oam_address(value),
                0x0004 => self.ppu.write_oam_data(value),
                _ => {}
            },

            // The controller's strobe line.
            0x4016 => self.controller.write(value),

            // Sprite memory's fast lane: one write here streams a
            // whole 256-byte page of RAM into OAM. On hardware the
            // CPU stands still for 513 cycles while it flows; ours
            // is instant, and that debt is on the books for Part II.
            0x4014 => {
                let base = (value as u16) << 8;
                for offset in 0..256u16 {
                    let byte = self.read(base + offset);
                    self.ppu.write_oam_data(byte);
                }
            }

            // Everything else on the sound chip's page: the APU's
            // registers. ($4014 and $4016 were claimed above; $4017,
            // the frame counter, falls through inside — Part II.)
            0x4000..=0x4017 => self.apu.write(address, value),

            // Writes to ROM change nothing. The clue is in the name.
            0x8000..=0xFFFF => {}

            // Everywhere else: nobody listening yet.
            _ => {}
        }
    }
}
