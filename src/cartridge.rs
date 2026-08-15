//! The cartridge — the game itself.

/// The four ways a board can wire the picture chip's two rooms of
/// name RAM over the four addressable names. Part I met the first
/// two; the single-screen pair arrives with boards that can choose.
#[derive(Clone, Copy, PartialEq)]
pub enum Mirroring {
    Vertical,
    Horizontal,
    SingleLow,
    SingleHigh,
}

/// What comes out of a .nes file: the two ROMs and the board type.
pub struct Cartridge {
    /// PRG ROM — the PRoGram: code and data for the CPU.
    pub prg_rom: Vec<u8>,

    /// CHR ROM — the CHaRacters: tile graphics for the picture chip.
    pub chr_rom: Vec<u8>,

    /// Which cartridge board ("mapper") the game expects. Number 0 is
    /// NROM, the simplest: two ROMs, no tricks.
    pub mapper: u8,

    /// How the board wires the picture chip's two rooms of name RAM:
    /// stacked side by side (vertical mirroring) or one above the
    /// other (horizontal).
    pub vertical_mirroring: bool,

    /// CHR RAM: boards that ship a blank album and draw their own
    /// art. Present exactly when the file brings no CHR ROM.
    pub chr_ram: Vec<u8>,

    /// PRG RAM at $6000-$7FFF: the pocket on the board.
    pub prg_ram: Vec<u8>,

    /// UxROM's latch: which 16 KiB program bank sits at $8000.
    pub prg_bank: u8,

    /// CNROM's latch: which 8 KiB album is wired in.
    pub chr_bank: u8,

    /// MMC1's serial port — five writes, one bit at a time — and
    /// the four registers those writes fill.
    pub shift: u8,
    pub control: u8,
    pub chr0: u8,
    pub chr1: u8,
    pub prg: u8,
}

impl Cartridge {
    /// Unpack an iNES file: sixteen bytes of header, then the ROMs.
    pub fn load(bytes: &[u8]) -> Result<Cartridge, String> {
        // Every iNES file opens with the same four bytes: the letters
        // N, E, S, then $1A. Anything else is not our format.
        if bytes.len() < 16 || &bytes[0..4] != b"NES\x1A" {
            return Err("not an iNES file".to_string());
        }

        // The sizes come in chunks: PRG in 16 KiB units, CHR in 8 KiB.
        let prg_size = bytes[4] as usize * 16 * 1024;
        let chr_size = bytes[5] as usize * 8 * 1024;

        // The mapper number arrives split in half: its low four bits
        // ride in the top of byte 6, its high four bits in byte 7.
        let mapper = (bytes[7] & 0xF0) | (bytes[6] >> 4);

        // Byte 6's lowest bit: how the board wires the name RAM.
        let vertical_mirroring = bytes[6] & 0b0000_0001 != 0;

        // Some old files carry a 512-byte "trainer" before the ROMs —
        // a relic of 1990s copying hardware. Step politely over it.
        let mut start = 16;
        if bytes[6] & 0b0000_0100 != 0 {
            start += 512;
        }

        if bytes.len() < start + prg_size + chr_size {
            return Err("file is shorter than its header promises".to_string());
        }

        Ok(Cartridge {
            prg_rom: bytes[start..start + prg_size].to_vec(),
            chr_rom: bytes[start + prg_size..start + prg_size + chr_size].to_vec(),
            mapper,
            vertical_mirroring,
            // A file with no CHR ROM gets a blank album to draw in.
            chr_ram: if chr_size == 0 {
                vec![0; 8 * 1024]
            } else {
                Vec::new()
            },
            prg_ram: vec![0; 8 * 1024],
            prg_bank: 0,
            chr_bank: 0,
            shift: 0b1_0000,
            control: 0x0C, // MMC1 wakes with the last bank fixed
            chr0: 0,
            chr1: 0,
            prg: 0,
        })
    }

    /// Read from PRG as the CPU sees it, through whichever board is
    /// in the slot. NROM's modulo survives as the simplest case; the
    /// switchers choose which 16 KiB stands where.
    pub fn read_prg(&self, address: u16) -> u8 {
        let offset = address as usize - 0x8000;
        let last = self.prg_rom.len() - 16 * 1024;

        let index = match self.mapper {
            // MMC1: the control register's PRG mode decides.
            1 => match (self.control >> 2) & 0b11 {
                // 32 KiB at a time: the bank number's low bit ignored.
                0 | 1 => (self.prg as usize & !1) * 16 * 1024 + offset,
                // First bank fixed in front, chosen bank behind.
                2 => {
                    if offset < 0x4000 {
                        offset
                    } else {
                        self.prg as usize * 16 * 1024 + (offset - 0x4000)
                    }
                }
                // Chosen bank in front, last bank fixed behind.
                _ => {
                    if offset < 0x4000 {
                        self.prg as usize * 16 * 1024 + offset
                    } else {
                        last + (offset - 0x4000)
                    }
                }
            },

            // UxROM: a chosen bank in front, the last bank bolted
            // down behind — the fixed half is what keeps the reset
            // vector reachable no matter what the latch says.
            2 => {
                if offset < 0x4000 {
                    self.prg_bank as usize * 16 * 1024 + offset
                } else {
                    last + (offset - 0x4000)
                }
            }


            // NROM and CNROM: no PRG tricks, the modulo is the wire.
            _ => offset % self.prg_rom.len(),
        };
        self.prg_rom[index % self.prg_rom.len()]
    }

    /// A write into ROM territory reaches the BOARD, not the ROM —
    /// this is where every latch on the shelf lives.
    pub fn write_prg(&mut self, address: u16, value: u8) {
        match self.mapper {
            1 => self.mmc1_serial(address, value),
            2 => self.prg_bank = value & 0x0F,
            3 => self.chr_bank = value & 0x03,
            _ => {}
        }
    }

    /// MMC1 listens one bit at a time: five writes fill the shift
    /// register, and the FIFTH write's address picks which register
    /// receives it. Bit 7 slams the port shut and starts over.
    fn mmc1_serial(&mut self, address: u16, value: u8) {
        if value & 0x80 != 0 {
            self.shift = 0b1_0000;
            self.control |= 0x0C;
            return;
        }

        let complete = self.shift & 1 != 0;
        let shifted = (self.shift >> 1) | ((value & 1) << 4);
        if !complete {
            self.shift = shifted;
            return;
        }

        match address {
            0x8000..=0x9FFF => self.control = shifted,
            0xA000..=0xBFFF => self.chr0 = shifted,
            0xC000..=0xDFFF => self.chr1 = shifted,
            _ => self.prg = shifted & 0x0F,
        }
        self.shift = 0b1_0000;
    }

    /// Read one byte of the album, through the board: CHR RAM if the
    /// album is blank, a chosen bank if the board switches, plain
    /// ROM otherwise.
    pub fn read_chr(&self, address: u16) -> u8 {
        let address = address as usize;
        if !self.chr_ram.is_empty() {
            return self.chr_ram[address];
        }

        let index = match self.mapper {
            1 => {
                if self.control & 0b1_0000 != 0 {
                    // 4 KiB halves, chosen separately.
                    let bank = if address < 0x1000 {
                        self.chr0
                    } else {
                        self.chr1
                    };
                    bank as usize * 4 * 1024 + (address & 0x0FFF)
                } else {
                    (self.chr0 as usize & !1) * 4 * 1024 + address
                }
            }
            3 => self.chr_bank as usize * 8 * 1024 + address,
            _ => address,
        };
        self.chr_rom[index % self.chr_rom.len()]
    }

    /// Write into the album — only blank albums accept ink.
    pub fn write_chr(&mut self, address: u16, value: u8) {
        if !self.chr_ram.is_empty() {
            self.chr_ram[address as usize] = value;
        }
    }

    /// How the board is wiring the name RAM RIGHT NOW. Headers speak
    /// once; MMC1 changes its mind at run time, single screens
    /// included.
    pub fn mirroring(&self) -> Mirroring {
        if self.mapper == 1 {
            return match self.control & 0b11 {
                0 => Mirroring::SingleLow,
                1 => Mirroring::SingleHigh,
                2 => Mirroring::Vertical,
                _ => Mirroring::Horizontal,
            };
        }

        if self.vertical_mirroring {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny iNES file in memory: a header plus filler ROMs.
    fn fake_rom(prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; 16];
        bytes[0..4].copy_from_slice(b"NES\x1A");
        bytes[4] = prg_banks;
        bytes[5] = chr_banks;
        bytes.extend(vec![0xAB; prg_banks as usize * 16 * 1024]);
        bytes.extend(vec![0xCD; chr_banks as usize * 8 * 1024]);
        bytes
    }

    #[test]
    fn load_reads_the_header() {
        let cartridge = Cartridge::load(&fake_rom(2, 1)).unwrap();
        assert_eq!(cartridge.prg_rom.len(), 32 * 1024);
        assert_eq!(cartridge.chr_rom.len(), 8 * 1024);
        assert_eq!(cartridge.mapper, 0);
    }

    #[test]
    fn a_wrong_magic_number_is_refused() {
        let mut bytes = fake_rom(1, 1);
        bytes[0] = b'X';
        assert!(Cartridge::load(&bytes).is_err());
    }

    #[test]
    fn a_16k_program_appears_twice() {
        // One PRG bank: $8000 and $C000 read the same byte.
        let mut bytes = fake_rom(1, 1);
        bytes[16] = 0x42; // the first PRG byte
        let cartridge = Cartridge::load(&bytes).unwrap();
        assert_eq!(cartridge.read_prg(0x8000), 0x42);
        assert_eq!(cartridge.read_prg(0xC000), 0x42);
    }

    #[test]
    fn a_32k_program_fills_the_window() {
        let mut bytes = fake_rom(2, 1);
        bytes[16] = 0x11; // first byte of the first bank
        bytes[16 + 16 * 1024] = 0x22; // first byte of the second bank
        let cartridge = Cartridge::load(&bytes).unwrap();
        assert_eq!(cartridge.read_prg(0x8000), 0x11);
        assert_eq!(cartridge.read_prg(0xC000), 0x22);
    }

    #[test]
    fn the_mapper_number_reassembles_from_its_halves() {
        let mut bytes = fake_rom(1, 1);
        bytes[6] = 0x40; // low half 4, riding in the top of byte 6
        bytes[7] = 0x20; // high half 2
        let cartridge = Cartridge::load(&bytes).unwrap();
        assert_eq!(cartridge.mapper, 0x24);
    }

    /// A switching board: `banks` 16 KiB of PRG, each filled with
    /// its own bank number, so reads confess where they came from.
    fn switcher(mapper: u8, banks: u8) -> Cartridge {
        let mut bytes = fake_rom(banks, 1);
        bytes[6] = mapper << 4; // the mapper's low half rides up here
        for bank in 0..banks as usize {
            for b in &mut bytes[16 + bank * 16 * 1024..16 + (bank + 1) * 16 * 1024] {
                *b = bank as u8;
            }
        }
        Cartridge::load(&bytes).unwrap()
    }

    #[test]
    fn uxrom_switches_the_front_and_bolts_the_back() {
        let mut cartridge = switcher(2, 4);
        cartridge.write_prg(0x8000, 2);
        assert_eq!(cartridge.read_prg(0x8000), 2); // the chosen bank
        assert_eq!(cartridge.read_prg(0xC000), 3); // the bolted last
    }

    #[test]
    fn cnrom_swaps_whole_albums() {
        let mut bytes = fake_rom(1, 2);
        bytes[6] = 3 << 4;
        bytes[16 + 16 * 1024] = 0x11; // first byte of album 0
        bytes[16 + 16 * 1024 + 8 * 1024] = 0x22; // first byte of album 1
        let mut cartridge = Cartridge::load(&bytes).unwrap();
        assert_eq!(cartridge.read_chr(0), 0x11);
        cartridge.write_prg(0x8000, 1);
        assert_eq!(cartridge.read_chr(0), 0x22);
    }

    #[test]
    fn mmc1_listens_five_bits_at_a_time() {
        let mut cartridge = switcher(1, 4);
        // Serial-write 0b01110 into control: fix-last mode, then
        // pick bank 2 in front through the PRG register.
        for bit in [0, 1, 1, 1, 0] {
            cartridge.write_prg(0x8000, bit);
        }
        for bit in [0, 1, 0, 0, 0] {
            cartridge.write_prg(0xE000, bit);
        }
        assert_eq!(cartridge.read_prg(0x8000), 2);
        assert_eq!(cartridge.read_prg(0xC000), 3);

        // Bit 7 slams the port shut mid-word: the half-entered word
        // vanishes and fix-last comes back.
        cartridge.write_prg(0x8000, 1);
        cartridge.write_prg(0x8000, 0x80);
        for bit in [1, 0, 0, 0, 0] {
            cartridge.write_prg(0xE000, bit);
        }
        assert_eq!(cartridge.read_prg(0x8000), 1);
    }

    #[test]
    fn mmc1_rewires_the_name_ram() {
        let mut cartridge = switcher(1, 2);
        for bit in [0, 1, 0, 0, 0] {
            cartridge.write_prg(0x8000, bit); // control = 0b00010
        }
        assert!(cartridge.mirroring() == Mirroring::Vertical);
        for bit in [1, 1, 0, 0, 0] {
            cartridge.write_prg(0x8000, bit); // control = 0b00011
        }
        assert!(cartridge.mirroring() == Mirroring::Horizontal);
    }

    #[test]
    fn a_blank_album_accepts_ink() {
        let mut cartridge = Cartridge::load(&fake_rom(1, 0)).unwrap();
        assert_eq!(cartridge.chr_ram.len(), 8 * 1024);
        cartridge.write_chr(0x0123, 0x77);
        assert_eq!(cartridge.read_chr(0x0123), 0x77);
    }
}
