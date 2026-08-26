//! The bus — the wiring that decides which chip answers each address.

use std::cell::Cell;

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::clock::Clock;
use crate::controller::Controller;
use crate::ppu::Ppu;

/// Everything on the far side of the CPU's read and write lines.
pub struct Bus {
    /// Cycles a DMA stole from the CPU, waiting to be billed.
    pub dma_stall: u64,

    /// A $4014 write happened: the 513-or-514 bill is the CPU's to
    /// compute, since only it knows the cycle parity.
    pub oam_dma_started: bool,

    /// The instruction cycle where a sampler fetch was billed full.
    pub dmc_full_bill_at: Option<u64>,

    /// The two halves of the controller-bit steal: the port was
    /// read this instruction; a sampler fetch landed this
    /// instruction. When both are true, one bit is gone.
    pub read_4016_this_instruction: Cell<bool>,
    pub dmc_fetched_this_instruction: Cell<bool>,

    /// The wire remembers: whatever byte last crossed the bus. An
    /// unwired address answers with it.
    pub open_bus: Cell<u8>,

    /// The PPU's own I/O latch: every byte through $2000-$2007
    /// refreshes it, write-only registers answer with it, and each
    /// BIT decays to zero in about 600 milliseconds unrefreshed —
    /// eight capacitors, eight timers.
    pub ppu_latch: Cell<u8>,
    pub ppu_latch_frame: [Cell<u64>; 8],

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

    /// The interrupt-request line: any device on the bus may hold it
    /// high, and the CPU answers between instructions if the I flag
    /// allows. Nobody drives it yet; cartridges and the sound chip
    /// will.
    pub irq_line: bool,
}

impl Bus {
    /// Wire a bus around a cartridge.
    pub fn new(cartridge: Cartridge) -> Bus {
        Bus {
            dma_stall: 0,
            oam_dma_started: false,
            dmc_full_bill_at: None,
            read_4016_this_instruction: Cell::new(false),
            dmc_fetched_this_instruction: Cell::new(false),
            open_bus: Cell::new(0),
            ppu_latch: Cell::new(0),
            ppu_latch_frame: [const { Cell::new(0) }; 8],
            ram: [0; 2048],
            // Built before `cartridge` moves in, since it reads from it.
            ppu: Ppu::new(cartridge.vertical_mirroring),
            controller: Controller::new(),
            apu: Apu::new(),
            clock: Clock::new(),
            irq_line: false,
            cartridge,
        }
    }

    /// One address in, one byte out — and whatever comes out lingers
    /// on the wire, because that is what wires do.
    pub fn read(&self, address: u16) -> u8 {
        let value = self.read_decoded(address);
        self.open_bus.set(value);
        value
    }

    /// The PPU's latch, each bit decayed if stale: ~600 ms is 36
    /// frames, and every bit keeps its own clock.
    fn ppu_stale(&self) -> u8 {
        let now = self.clock.frame;
        let mut value = self.ppu_latch.get();
        for bit in 0..8 {
            if now.wrapping_sub(self.ppu_latch_frame[bit].get()) >= 36 {
                value &= !(1 << bit);
            }
        }
        value
    }

    /// A byte through the PPU's port refreshes the latch — only the
    /// bits in `mask`: $2002 drives three, a palette read six, most
    /// traffic all eight.
    fn ppu_refresh(&self, value: u8, mask: u8) -> u8 {
        let kept = self.ppu_latch.get() & !mask;
        self.ppu_latch.set(kept | (value & mask));
        for bit in 0..8 {
            if mask & (1 << bit) != 0 {
                self.ppu_latch_frame[bit].set(self.clock.frame);
            }
        }
        value
    }

    /// One address in, one byte out — from whichever chip lives there.
    /// (A `..=` pattern matches a whole range of values at once.)
    fn read_decoded(&self, address: u16) -> u8 {
        match address {
            // The console's RAM — and its three echoes. The board only
            // decodes enough address wires to tell 2 KiB apart, so the
            // same bytes answer again at $0800, $1000, and $1800.
            0x0000..=0x1FFF => self.ram[address as usize % 2048],

            // The picture chip's eight registers, echoed every eight
            // bytes to the top of the range. Two answer reads — and
            // both change state when looked at.
            0x2000..=0x3FFF => match address & 0x0007 {
                0x0002 => {
                    // The status register drives only its top three
                    // bits; the other five are the latch's ghosts —
                    // and only the driven three refresh.
                    let status = self.ppu.read_status() & 0xE0;
                    let ghosts = self.ppu_stale() & 0x1F;
                    self.ppu_refresh(status, 0xE0);
                    status | ghosts
                }
                0x0004 => {
                    let value = self.ppu.read_oam_data();
                    self.ppu_refresh(value, 0xFF)
                }
                0x0007 => {
                    let reading_palette = self.ppu.data_address() & 0x3FFF >= 0x3F00;
                    self.cartridge.watch_address(self.ppu.data_address());
                    let value = self.ppu.read_data(&self.cartridge);
                    // The port stepped; the bus now shows the new
                    // address, and its bit 12 counts too.
                    self.cartridge.watch_address(self.ppu.data_address());
                    if reading_palette {
                        // A palette entry is six bits; the top two
                        // come from the latch and don't refresh it.
                        let value = (value & 0x3F) | (self.ppu_stale() & 0xC0);
                        self.ppu_refresh(value, 0x3F);
                        value
                    } else {
                        self.ppu_refresh(value, 0xFF)
                    }
                }
                // The write-only registers: nothing drives the bus,
                // so the latch answers — stale ghosts and all.
                _ => self.ppu_stale(),
            },

            // The sound chip's one readable register.
            0x4015 => self.apu.read_status(),
            // The first controller: one button per read.
            0x4016 => {
                // A sampler DMA that landed earlier in this very
                // instruction re-clocked the port: the bit it read
                // is gone before the program looks.
                if self.dmc_fetched_this_instruction.get() {
                    self.controller.read();
                }
                self.read_4016_this_instruction.set(true);
                // The port drives only its low bits; the top three
                // are whatever the wire remembers.
                (self.open_bus.get() & 0xE0) | (self.controller.read() & 0x1F)
            }

            // The second port: nobody plugged in, low bits grounded,
            // top bits the wire's memory.
            0x4017 => self.open_bus.get() & 0xE0,

            // Sound registers, and the second controller port. Nobody.
            // Write-only sound registers: nothing answers, so the
            // wire's memory does.
            0x4000..=0x401F => self.open_bus.get(),

            // Cartridge territory the boards on our shelf leave unwired.
            0x4020..=0x5FFF => self.open_bus.get(),

            // The pocket on the board.
            0x6000..=0x7FFF => self.cartridge.prg_ram[address as usize - 0x6000],

            // The cartridge's program ROM.
            0x8000..=0xFFFF => self.cartridge.read_prg(address),
        }
    }

    /// One address in, one byte delivered.
    pub fn write(&mut self, address: u16, value: u8) {
        self.open_bus.set(value);
        if matches!(address, 0x2000..=0x3FFF) {
            self.ppu_refresh(value, 0xFF);
        }
        match address {
            0x0000..=0x1FFF => self.ram[address as usize % 2048] = value,

            // The picture chip's registers, for the writes games lean
            // on: settings, the address, the data.
            0x2000..=0x3FFF => match address & 0x0007 {
                0x0000 => self.ppu.write_ctrl(value),
                0x0001 => self.ppu.mask = value,
                0x0003 => self.ppu.write_oam_address(value),
                0x0004 => self.ppu.write_oam_data(value),
                0x0005 => self.ppu.write_scroll(value),
                0x0006 => {
                    self.ppu.write_address(value);
                    self.cartridge.watch_address(self.ppu.data_address());
                }
                // Pattern space lives on the cartridge: a data-port
                // write aimed below $2000 goes to the board (blank
                // albums accept it), and the port still steps.
                0x0007 => {
                    let address = self.ppu.data_address();
                    self.cartridge.watch_address(address);
                    if address < 0x2000 {
                        self.cartridge.write_chr(address, value);
                        self.ppu.step_address();
                    } else {
                        self.ppu.write_data(value);
                    }
                    self.cartridge.watch_address(self.ppu.data_address());
                }
                _ => {}
            },

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
                self.oam_dma_started = true;
            }

            // The controller's strobe line.
            0x4016 => self.controller.write(value),

            // Everything else on the sound chip's page: the APU's
            // registers. ($4014 and $4016 were claimed above; $4017,
            // the frame counter, falls through inside — Part II.)
            0x4000..=0x4017 => self.apu.write(address, value),

            // The pocket accepts ink too.
            0x6000..=0x7FFF => {
                self.cartridge.prg_ram[address as usize - 0x6000] = value;
            }

            // A write into ROM territory reaches the board's latches
            // — and the board may have rewired the name RAM by the
            // time it's done, so the picture chip gets the news.
            0x8000..=0xFFFF => {
                self.cartridge.write_prg(address, value);
                self.ppu.set_mirroring(self.cartridge.mirroring());
            }

            // Everywhere else: nobody listening yet.
            _ => {}
        }
    }
}
