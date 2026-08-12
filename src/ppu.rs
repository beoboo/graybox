//! The picture chip — built here, piece by piece, starting with the
//! part every picture is made of: tiles.

/// Decode one tile: 16 bytes of CHR become 8x8 pixels of 0..=3.
///
/// A tile's first 8 bytes are plane 0 (every pixel's low bit, one byte
/// per row), its second 8 are plane 1 (the high bits). Bit 7 is the
/// leftmost pixel.
pub fn decode_tile(chr: &[u8], tile: usize) -> [[u8; 8]; 8] {
    let start = tile * 16;
    let mut pixels = [[0u8; 8]; 8];

    for row in 0..8 {
        let plane0 = chr[start + row];
        let plane1 = chr[start + row + 8];

        for col in 0..8 {
            let bit = 7 - col;
            let low = (plane0 >> bit) & 1;
            let high = (plane1 >> bit) & 1;
            pixels[row][col] = (high << 1) | low;
        }
    }
    pixels
}

use std::cell::Cell;

/// The picture chip's own state: its private memory and the handful of
/// registers the CPU can reach through the bus.
///
/// Two fields wear a `Cell` — a box Rust lets us change even through a
/// shared reference. It exists for exactly our situation: hardware that
/// changes state when it is merely LOOKED AT.
pub struct Ppu {
    /// The nametables' RAM: 2 KiB on the console's board.
    pub vram: [u8; 2048],

    /// The eight palettes — 32 bytes the games fill at boot.
    pub palette_ram: [u8; 32],

    /// The sprite roster — OAM, Object Attribute Memory: 64 sprites,
    /// four bytes each. Games rebuild all 256 bytes every frame.
    pub oam: [u8; 256],

    /// PPUCTRL, as last written: a byte of settings.
    pub ctrl: u8,

    /// PPUMASK ($2001): which layers draw, plus the tint and clipping
    /// switches — the register that says whether rendering is on.
    pub mask: u8,

    /// Whether the PPU is in its vertical-blank rest period.
    pub vblank: Cell<bool>,

    /// Where in PPU memory the next data access will land. A `Cell`,
    /// because READS move it too.
    address: Cell<u16>,

    /// The one-byte waiting room for PPUDATA reads: what a read hands
    /// back is the byte fetched by the PREVIOUS read.
    read_buffer: Cell<u8>,

    /// The address arrives through an 8-bit register in two knocks;
    /// this remembers whether the SECOND knock is next.
    expecting_low: Cell<bool>,

    /// How this cartridge's board arranges the four nametable names
    /// over the two real rooms of VRAM.
    vertical_mirroring: bool,
}

impl Ppu {
    /// A powered-on PPU: memory blank, registers zero, wired the way
    /// the cartridge's board dictates.
    pub fn new(vertical_mirroring: bool) -> Ppu {
        Ppu {
            vram: [0; 2048],
            palette_ram: [0; 32],
            oam: [0; 256],
            ctrl: 0,
            mask: 0,
            vblank: Cell::new(false),
            address: Cell::new(0),
            read_buffer: Cell::new(0),
            expecting_low: Cell::new(false),
            vertical_mirroring,
        }
    }

    /// Fold a nametable address ($2000-$2FFF and its echo) onto the two
    /// real 1 KiB rooms. Four names, two rooms: VERTICAL mirroring puts
    /// the rooms side by side ($2000/$2800 share, $2400/$2C00 share);
    /// HORIZONTAL stacks them ($2000/$2400 share, $2800/$2C00 share).
    fn mirror(&self, address: u16) -> usize {
        let index = address as usize & 0x0FFF; // within the 4 KiB window
        let name = index / 0x400; // which of the four names, 0..=3
        let room = if self.vertical_mirroring {
            name & 1
        } else {
            name / 2
        };
        room * 0x400 + (index & 0x3FF)
    }

    /// A write to PPUADDR ($2006): half of an address — big half on the
    /// first knock, little half on the second.
    pub fn write_address(&mut self, value: u8) {
        let address = self.address.get();
        if self.expecting_low.get() {
            self.address.set((address & 0xFF00) | value as u16);
        } else {
            self.address.set((address & 0x00FF) | ((value as u16) << 8));
        }
        self.expecting_low.set(!self.expecting_low.get());
    }

    /// A write to PPUDATA ($2007): one byte into PPU memory, wherever
    /// the address points — which then walks forward on its own.
    pub fn write_data(&mut self, value: u8) {
        let address = self.address.get() & 0x3FFF;
        match address {
            // Pattern tables are the cartridge's ROM: writes bounce off.
            0x0000..=0x1FFF => {}

            // The nametables' two rooms, found through the board's
            // mirroring arrangement.
            0x2000..=0x3EFF => self.vram[self.mirror(address)] = value,

            // The palettes' 32 bytes.
            _ => self.palette_ram[address as usize & 0x001F] = value,
        }

        self.step_address();
    }

    /// The auto-step every PPUDATA access ends with: +1 normally, +32 —
    /// one screen-row down — when PPUCTRL asks for column order.
    fn step_address(&self) {
        let step = if self.ctrl & 0b0000_0100 != 0 { 32 } else { 1 };
        self.address.set(self.address.get().wrapping_add(step));
    }

    /// A read of PPUSTATUS ($2002): reports vblank in the top bit — and
    /// the act of looking clears the flag AND resets the address
    /// register to expect a first knock. Reading, here, is touching —
    /// which is why those two fields live in `Cell`s.
    pub fn read_status(&self) -> u8 {
        let status = (self.vblank.get() as u8) << 7;
        self.vblank.set(false);
        self.expecting_low.set(false);
        status
    }

    /// A read of PPUDATA ($2007). The byte is the small part — it
    /// arrives one read LATE, through the waiting room. The important
    /// part is that reading walks the address forward exactly like
    /// writing, and games lean on that to step past addresses without
    /// disturbing them.
    pub fn read_data(&self) -> u8 {
        let address = self.address.get() & 0x3FFF;
        let value = self.read_buffer.get();

        self.read_buffer.set(match address {
            // Pattern tables live on the cartridge — not wired from
            // this side of the chip yet.
            0x0000..=0x1FFF => 0,
            0x2000..=0x3EFF => self.vram[self.mirror(address)],
            _ => self.palette_ram[address as usize & 0x001F],
        });

        self.step_address();
        value
    }

    /// Rendering is on when the mask shows the background, the
    /// sprites, or both — the condition the clock's odd-frame
    /// skip watches.
    pub fn rendering(&self) -> bool {
        self.mask & 0b0001_1000 != 0
    }
}

/// The NES's whole crayon box: the 64 colors it can ever show, as RGB.
///
/// The real chip stores no RGB anywhere — it emits an analog TV signal,
/// and every emulator's table is one honest reading of that signal.
/// These values were computed from the signal's documented voltages
/// (Part II does that computation live, and finds even more colors).
pub const SYSTEM_PALETTE: [u32; 64] = [
    0x525252, 0x001E94, 0x0907C2, 0x3100BD, 0x580086, 0x6F0036, 0x6C0000, 0x501000, 0x272900,
    0x023F00, 0x004B00, 0x004700, 0x003646, 0x000000, 0x000000, 0x000000, 0xA0A0A0, 0x004FFF,
    0x2C2AFF, 0x6D0FFF, 0xA905ED, 0xCB0775, 0xC61909, 0x9D3900, 0x5E6100, 0x208300, 0x009400,
    0x008F1A, 0x00758D, 0x000000, 0x000000, 0x000000, 0xFEFEFE, 0x3EA4FF, 0x7B78FF, 0xC656FF,
    0xFF46FF, 0xFF4ACE, 0xFF634C, 0xFB8B00, 0xB5B800, 0x6BDE00, 0x34F205, 0x1AEC63, 0x1ECFEA,
    0x3C3C3C, 0x000000, 0x000000, 0xFEFEFE, 0xAAD8FF, 0xC6C5FF, 0xE7B5FF, 0xFFADFF, 0xFFB0EA,
    0xFFBBB1, 0xFDCD82, 0xDFE16A, 0xBFF16D, 0xA5F98A, 0x97F7BC, 0x99EBF6, 0xA9A9A9, 0x000000,
    0x000000,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_drawn_row_decodes() {
        // Row 0: plane 0 = $55, plane 1 = $33 — reading the bits down
        // the columns should give 0,1,2,3 repeating.
        let mut chr = vec![0u8; 16];
        chr[0] = 0x55;
        chr[8] = 0x33;

        let pixels = decode_tile(&chr, 0);
        assert_eq!(pixels[0], [0, 1, 2, 3, 0, 1, 2, 3]);
        assert_eq!(pixels[1], [0; 8]); // every other row stayed blank
    }

    #[test]
    fn tiles_are_sixteen_bytes_apart() {
        // A single low bit at the start of tile 1, nowhere near tile 0.
        let mut chr = vec![0u8; 32];
        chr[16] = 0b1000_0000;

        let pixels = decode_tile(&chr, 1);
        assert_eq!(pixels[0][0], 1);
        assert_eq!(decode_tile(&chr, 0)[0][0], 0);
    }

    #[test]
    fn famous_crayons_are_where_they_belong() {
        // $0F is the NES's true black; $20 and $30 are its whites; the
        // $x0 column is the grays, darkest to lightest.
        assert_eq!(SYSTEM_PALETTE[0x0F], 0x000000);
        assert_eq!(SYSTEM_PALETTE[0x20], SYSTEM_PALETTE[0x30]);
        // The white, pinned to the exact byte: a rounding slip here
        // passes every other test and shows up only when a frame sits
        // next to a reference picture.
        assert_eq!(SYSTEM_PALETTE[0x20], 0xFEFEFE);
        assert!(SYSTEM_PALETTE[0x00] < SYSTEM_PALETTE[0x10]);
    }

    #[test]
    fn every_crayon_is_plain_rgb() {
        // No alpha surprises for the frame buffer.
        for color in SYSTEM_PALETTE {
            assert_eq!(color >> 24, 0);
        }
    }

    #[test]
    fn the_address_arrives_big_half_first() {
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x23);
        ppu.write_address(0x45);
        ppu.write_data(0x99);
        assert_eq!(ppu.vram[0x0345], 0x99); // $2345 & $07FF
    }

    #[test]
    fn data_writes_walk_forward_on_their_own() {
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x20);
        ppu.write_address(0x00);
        ppu.write_data(0x11);
        ppu.write_data(0x22);
        assert_eq!(ppu.vram[0], 0x11);
        assert_eq!(ppu.vram[1], 0x22);
    }

    #[test]
    fn ctrl_can_make_the_walk_go_by_rows() {
        let mut ppu = Ppu::new(false);
        ppu.ctrl = 0b0000_0100; // column order: step by 32
        ppu.write_address(0x20);
        ppu.write_address(0x00);
        ppu.write_data(0x11);
        ppu.write_data(0x22);
        assert_eq!(ppu.vram[32], 0x22); // one screen-row down
    }

    #[test]
    fn palette_writes_land_in_the_palette() {
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x3F);
        ppu.write_address(0x01);
        ppu.write_data(0x16);
        assert_eq!(ppu.palette_ram[1], 0x16);
    }

    #[test]
    fn looking_at_status_clears_the_flag() {
        let ppu = Ppu::new(false);
        ppu.vblank.set(true);
        assert_eq!(ppu.read_status() & 0x80, 0x80);
        assert_eq!(ppu.read_status() & 0x80, 0); // looking took it
    }

    #[test]
    fn horizontal_mirroring_keeps_2800_out_of_2000() {
        // Chase's exact accident: write a title byte at $2086, then
        // clear $2886 (nametable C). On a horizontal board those are
        // different rooms — the title must survive.
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x20);
        ppu.write_address(0x86);
        ppu.write_data(0x40);
        ppu.write_address(0x28);
        ppu.write_address(0x86);
        ppu.write_data(0x00);
        assert_eq!(ppu.vram[0x086], 0x40); // the title byte, alive
        assert_eq!(ppu.vram[0x486], 0x00); // the other room, cleared
    }

    #[test]
    fn vertical_mirroring_shares_the_other_way() {
        // On a vertical board, $2000 and $2800 ARE the same room.
        let mut ppu = Ppu::new(true);
        ppu.write_address(0x20);
        ppu.write_address(0x86);
        ppu.write_data(0x40);
        ppu.write_address(0x28);
        ppu.write_address(0x86);
        ppu.write_data(0x77);
        assert_eq!(ppu.vram[0x086], 0x77); // same room: overwritten
    }

    #[test]
    fn reading_data_walks_the_address_too() {
        // The trick real palette loaders use: READ $2007 and discard
        // the byte, just to step past an address without touching it.
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x3F);
        ppu.write_address(0x00);
        ppu.write_data(0x11); // lands at $3F00
        ppu.read_data(); // a footstep past $3F01, touching nothing
        ppu.write_data(0x33); // must land at $3F02
        assert_eq!(ppu.palette_ram[0], 0x11);
        assert_eq!(ppu.palette_ram[1], 0x00); // undisturbed
        assert_eq!(ppu.palette_ram[2], 0x33);
    }

    #[test]
    fn data_reads_arrive_one_read_late() {
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x20);
        ppu.write_address(0x00);
        ppu.write_data(0x55);

        ppu.write_address(0x20);
        ppu.write_address(0x00);
        assert_eq!(ppu.read_data(), 0x00); // the stale waiting room
        assert_eq!(ppu.read_data(), 0x55); // yesterday's byte, today
    }

    #[test]
    fn looking_at_status_also_resets_the_two_knocks() {
        // A game half-through an address read $2002: the next knock
        // must count as a FIRST knock again.
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x3F); // first knock of an abandoned address
        ppu.read_status();
        ppu.write_address(0x20); // a fresh first knock: the big half
        ppu.write_address(0x08);
        ppu.write_data(0x77);
        assert_eq!(ppu.vram[8], 0x77);
    }
}
