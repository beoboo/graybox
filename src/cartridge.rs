//! The cartridge — the game itself.

/// What comes out of a .nes file: the two ROMs and the board type.
pub struct Cartridge {
    /// PRG ROM — the PRoGram: code and data for the CPU.
    pub prg_rom: Vec<u8>,

    /// CHR ROM — the CHaRacters: tile graphics for the picture chip.
    pub chr_rom: Vec<u8>,

    /// Which cartridge board ("mapper") the game expects. Number 0 is
    /// NROM, the simplest: two ROMs, no tricks.
    pub mapper: u8,
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
        })
    }

    /// Read from PRG ROM as the CPU will see it: addresses $8000-$FFFF.
    /// A 32 KiB program fills that window exactly. A 16 KiB program
    /// appears TWICE — the smaller board simply doesn't connect the
    /// address wire that could tell the two halves apart, and the
    /// modulo plays the part of the missing wire.
    pub fn read_prg(&self, address: u16) -> u8 {
        let offset = (address as usize - 0x8000) % self.prg_rom.len();
        self.prg_rom[offset]
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
}
