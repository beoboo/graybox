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

    /// The picture, one color per pixel, painted a dot at a time as
    /// the beam walks. `main` collects it when the frame completes.
    pub frame: Vec<u32>,

    /// The conveyor: pattern bits for the tile being drawn and the
    /// tile behind it. The front pixel leaves from bit 15.
    shift_low: u16,
    shift_high: u16,

    /// The same conveyor for the palette choice, two bits per pixel.
    attr_low: u16,
    attr_high: u16,

    /// The pockets the fetch fills: one tile's pattern planes and
    /// its palette pick, waiting for the next tile boundary.
    next_pattern_low: u8,
    next_pattern_high: u8,
    next_attr: u8,

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
            frame: vec![0; 256 * 240],
            shift_low: 0,
            shift_high: 0,
            attr_low: 0,
            attr_high: 0,
            next_pattern_low: 0,
            next_pattern_high: 0,
            next_attr: 0,
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

            // The palettes' 32 bytes, through the fold.
            _ => self.palette_ram[Self::palette_index(address)] = value,
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
            _ => self.palette_ram[Self::palette_index(address)],
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

    /// Fill the pockets for one tile of one scanline: which tile
    /// (the nametable), its palette pick (the attribute table), and
    /// its two planes of pattern bits — chapter 13's read, done by
    /// the chip itself this time.
    fn fetch_tile(&mut self, scanline: usize, col: usize, chr: &[u8]) {
        let (row, fine_y) = (scanline / 8, scanline % 8);
        let tile = self.vram[row * 32 + col] as usize;

        let attribute = self.vram[0x3C0 + (row / 4) * 8 + col / 4];
        let shift = ((row % 4) / 2) * 4 + ((col % 4) / 2) * 2;
        self.next_attr = (attribute >> shift) & 0b11;

        // PPUCTRL bit 4 picks the background's half of the album:
        // sixteen bytes a tile, the high plane eight bytes in.
        let table = if self.ctrl & 0b0001_0000 != 0 {
            0x1000
        } else {
            0
        };
        let start = table + tile * 16 + fine_y;
        self.next_pattern_low = chr[start];
        self.next_pattern_high = chr[start + 8];
    }

    /// The belt advances one pixel: every register slides one bit
    /// toward the front.
    fn shift(&mut self) {
        self.shift_low <<= 1;
        self.shift_high <<= 1;
        self.attr_low <<= 1;
        self.attr_high <<= 1;
    }

    /// A tile boundary: the pockets empty onto the back of the belt.
    /// The two attribute bits stretch to cover all eight pixels.
    fn reload_shifts(&mut self) {
        self.shift_low = (self.shift_low & 0xFF00) | self.next_pattern_low as u16;
        self.shift_high = (self.shift_high & 0xFF00) | self.next_pattern_high as u16;

        let low = if self.next_attr & 1 != 0 { 0xFF } else { 0 };
        let high = if self.next_attr & 2 != 0 { 0xFF } else { 0 };
        self.attr_low = (self.attr_low & 0xFF00) | low;
        self.attr_high = (self.attr_high & 0xFF00) | high;
    }

    /// One pixel, from whatever is at the front of the belt right
    /// now — which is the whole point: the belt's front IS the beam.
    fn draw_dot(&mut self, scanline: usize, x: usize) {
        let color = if self.mask & 0b0000_1000 != 0 {
            let low = (self.shift_low >> 15) & 1;
            let high = (self.shift_high >> 15) & 1;
            let value = ((high << 1) | low) as usize;

            let palette =
                (((self.attr_high >> 15) & 1) << 1 | ((self.attr_low >> 15) & 1)) as usize;

            // Pattern value 0 is the shared backdrop, whichever
            // palette the neighborhood picked.
            let crayon = if value == 0 {
                self.palette_ram[0]
            } else {
                self.palette_ram[palette * 4 + value]
            };
            SYSTEM_PALETTE[crayon as usize]
        } else {
            // With rendering off and the address register parked
            // inside the crayon box, the screen shows THAT crayon —
            // the door `full_palette` draws its rainbow through.
            let address = self.address.get() & 0x3FFF;
            if address >= 0x3F00 {
                let crayon = self.palette_ram[Self::palette_index(address)];
                self.frame[scanline * 256 + x] = SYSTEM_PALETTE[crayon as usize];
                return;
            }

            // The background is hidden: the dot is backdrop.
            SYSTEM_PALETTE[self.palette_ram[0] as usize]
        };

        self.frame[scanline * 256 + x] = color;
    }

    /// One dot of the picture chip's day. Visible dots leave through
    /// the belt; the belt advances and the pockets refill on the
    /// schedule the beam sets; the tail of every line — warm-up lap
    /// included — primes the first two tiles of the line below.
    pub fn tick(&mut self, scanline: u16, dot: u16, chr: &[u8]) {
        let (scanline, dot) = (scanline as usize, dot as usize);
        let drawing_line = scanline < 240;

        if drawing_line && (1..=256).contains(&dot) {
            self.draw_dot(scanline, dot - 1);
        }

        if !self.rendering() || (!drawing_line && scanline != 261) {
            return;
        }

        if drawing_line && (1..=256).contains(&dot) {
            self.shift();

            // Boundaries land after every eighth pixel; the tile
            // fetched here reaches the front sixteen dots later.
            if dot % 8 == 0 && dot <= 240 {
                self.fetch_tile(scanline, dot / 8 + 1, chr);
                self.reload_shifts();
            }
        }

        if (321..=336).contains(&dot) {
            self.shift();

            if dot == 328 || dot == 336 {
                let below = if scanline == 261 { 0 } else { scanline + 1 };
                self.fetch_tile(below, if dot == 328 { 0 } else { 1 }, chr);
                self.reload_shifts();
            }
        }
    }

    /// The crayon box folds: $3F10, $3F14, $3F18 and $3F1C are the
    /// same cells as $3F00, $3F04, $3F08 and $3F0C — one shared
    /// backdrop column for backgrounds and sprites alike.
    fn palette_index(address: u16) -> usize {
        let index = (address & 0x001F) as usize;
        if index >= 0x10 && index % 4 == 0 {
            index - 0x10
        } else {
            index
        }
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

    /// A cartridge's worth of art for the belt tests: tile 0 blank,
    /// tile 1 solid pattern-value 3.
    fn test_chr() -> Vec<u8> {
        let mut chr = vec![0; 8192];
        for byte in &mut chr[16..32] {
            *byte = 0xFF;
        }
        chr
    }

    /// Walk the belt through the warm-up lap's priming and into a
    /// scanline, so the frame's first pixels arrive the way real ones do.
    fn prime_and_draw(ppu: &mut Ppu, chr: &[u8], dots: usize) {
        for dot in 321..=336 {
            ppu.tick(261, dot, chr);
        }
        for dot in 1..=dots as u16 {
            ppu.tick(0, dot, chr);
        }
    }

    #[test]
    fn the_crayon_box_folds_at_3f10() {
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x3F);
        ppu.write_address(0x10);
        ppu.write_data(0x2A);
        assert_eq!(ppu.palette_ram[0], 0x2A); // landed on $3F00
    }

    #[test]
    fn the_belt_delivers_the_tile_under_the_beam() {
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0000_1000; // background on
        ppu.vram[0] = 1; // first cell: the solid tile
        ppu.vram[2] = 1; // third cell: solid again
        ppu.palette_ram[0] = 0x0F; // backdrop: black
        ppu.palette_ram[3] = 0x30; // palette 0, value 3: white

        prime_and_draw(&mut ppu, &test_chr(), 16);

        // Tile 0 of the line is solid value 3; its neighbor is blank
        // and falls through to the backdrop.
        assert_eq!(ppu.frame[0], SYSTEM_PALETTE[0x30]);
        assert_eq!(ppu.frame[7], SYSTEM_PALETTE[0x30]);
        assert_eq!(ppu.frame[8], SYSTEM_PALETTE[0x0F]);

        // The palette answers at pixel time: repaint the crayon
        // mid-line, and the third cell — same tile, same bits,
        // already fetched — comes out in the new color.
        ppu.palette_ram[3] = 0x16;
        for dot in 17..=24 {
            ppu.tick(0, dot, &test_chr());
        }
        assert_eq!(ppu.frame[16], SYSTEM_PALETTE[0x16]);
    }

    #[test]
    fn rendering_off_shows_the_crayon_the_address_points_at() {
        let mut ppu = Ppu::new(false);
        ppu.palette_ram[7] = 0x21;
        ppu.write_address(0x3F);
        ppu.write_address(0x07);

        ppu.tick(0, 5, &test_chr());
        assert_eq!(ppu.frame[4], SYSTEM_PALETTE[0x21]);
    }

}
