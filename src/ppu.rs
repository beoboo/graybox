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

    /// All 512 colors the chip can emit — 64 crayons under each of
    /// the eight emphasis settings — decoded from the video signal
    /// when the machine powers on.
    colors: Vec<u32>,

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

    /// `v` — the address the chip is USING: where the beam reads
    /// tiles while drawing, and where the data port lands between
    /// frames. A `Cell`, because READS move it too.
    v: Cell<u16>,

    /// `t` — the address being STAGED: what the game writes through
    /// $2000, $2005 and $2006, waiting to be copied into `v`.
    t: u16,

    /// `x` — fine X: which of a tile's eight pixels the left screen
    /// edge cuts through.
    x: u8,

    /// The one-byte waiting room for PPUDATA reads: what a read hands
    /// back is the byte fetched by the PREVIOUS read.
    read_buffer: Cell<u8>,

    /// `w` — the shared two-knock latch: $2005 and $2006 both knock
    /// twice, on the SAME latch, and reading $2002 resets it.
    w: Cell<bool>,

    /// $2002 bit 6: sprite zero's opaque pixel met an opaque
    /// background pixel this frame. The flag games split screens on.
    sprite_zero_hit: bool,

    /// $2002 bit 5: a scanline wanted a ninth sprite.
    sprite_overflow: bool,

    /// OAMADDR ($2003): where the next $2004 access lands.
    oam_addr: u8,

    /// The eight (at most) sprites chosen for the line being drawn,
    /// their pixels already decoded and flipped.
    line_sprites: Vec<LineSprite>,

    /// How this cartridge's board arranges the four nametable names
    /// over the two real rooms of VRAM.
    vertical_mirroring: bool,
}

/// One sprite as the beam meets it: a single row of eight decoded
/// pixels, parked at an X position, with its manners attached.
struct LineSprite {
    x: u8,
    pixels: [u8; 8],
    attributes: u8,
    is_zero: bool,
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
            colors: (0..512).map(decode_color).collect(),
            shift_low: 0,
            shift_high: 0,
            attr_low: 0,
            attr_high: 0,
            next_pattern_low: 0,
            next_pattern_high: 0,
            next_attr: 0,
            vblank: Cell::new(false),
            v: Cell::new(0),
            t: 0,
            x: 0,
            read_buffer: Cell::new(0),
            w: Cell::new(false),
            sprite_zero_hit: false,
            sprite_overflow: false,
            oam_addr: 0,
            line_sprites: Vec::new(),
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

    /// A write to PPUCTRL ($2000): the settings byte — and its bottom
    /// two bits are secretly an address write: they stage which
    /// nametable `t` starts from.
    pub fn write_ctrl(&mut self, value: u8) {
        self.ctrl = value;
        self.t = (self.t & !0x0C00) | (((value & 0b11) as u16) << 10);
    }

    /// A write to PPUSCROLL ($2005): the camera. First knock stages
    /// the X scroll — coarse tile into `t`, fine pixel into `x` —
    /// and the second knock stages Y the same way, fine bits up top.
    pub fn write_scroll(&mut self, value: u8) {
        if self.w.get() {
            self.t = (self.t & !0x73E0)
                | (((value & 0b111) as u16) << 12)
                | (((value >> 3) as u16) << 5);
        } else {
            self.t = (self.t & !0x001F) | ((value >> 3) as u16);
            self.x = value & 0b111;
        }
        self.w.set(!self.w.get());
    }

    /// A write to OAMADDR ($2003): aim the roster's door.
    pub fn write_oam_address(&mut self, value: u8) {
        self.oam_addr = value;
    }

    /// A write to OAMDATA ($2004): one byte through the door, and
    /// the door moves on — OAM DMA is 256 of these in a row.
    pub fn write_oam_data(&mut self, value: u8) {
        self.oam[self.oam_addr as usize] = value;
        self.oam_addr = self.oam_addr.wrapping_add(1);
    }

    /// A read of OAMDATA: what the door shows, without moving it.
    /// An attribute byte's three unwired bits read back as zero —
    /// they simply don't exist on the chip.
    pub fn read_oam_data(&self) -> u8 {
        let value = self.oam[self.oam_addr as usize];
        if self.oam_addr & 3 == 2 {
            value & 0xE3
        } else {
            value
        }
    }

    /// A write to PPUADDR ($2006): half of an address — big half on
    /// the first knock, little half on the second. Both land in `t`,
    /// and the second knock copies the whole of `t` into `v`: setting
    /// the address and moving the camera are the same wire.
    pub fn write_address(&mut self, value: u8) {
        if self.w.get() {
            self.t = (self.t & 0xFF00) | value as u16;
            self.v.set(self.t);
        } else {
            self.t = (self.t & 0x00FF) | (((value & 0x3F) as u16) << 8);
        }
        self.w.set(!self.w.get());
    }

    /// A write to PPUDATA ($2007): one byte into PPU memory, wherever
    /// the address points — which then walks forward on its own.
    pub fn write_data(&mut self, value: u8) {
        let address = self.v.get() & 0x3FFF;
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
        self.v.set(self.v.get().wrapping_add(step));
    }

    /// A read of PPUSTATUS ($2002): reports vblank in the top bit — and
    /// the act of looking clears the flag AND resets the address
    /// register to expect a first knock. Reading, here, is touching —
    /// which is why those two fields live in `Cell`s.
    pub fn read_status(&self) -> u8 {
        let status = ((self.vblank.get() as u8) << 7)
            | ((self.sprite_zero_hit as u8) << 6)
            | ((self.sprite_overflow as u8) << 5);
        self.vblank.set(false);
        self.w.set(false);
        status
    }

    /// A read of PPUDATA ($2007). The byte is the small part — it
    /// arrives one read LATE, through the waiting room. The important
    /// part is that reading walks the address forward exactly like
    /// writing, and games lean on that to step past addresses without
    /// disturbing them.
    pub fn read_data(&self, chr: &[u8]) -> u8 {
        let address = self.v.get() & 0x3FFF;
        let value = self.read_buffer.get();

        self.read_buffer.set(match address {
            // Pattern tables: the cartridge's art, readable through
            // the port like everything else — and some games keep
            // level data on those chips and read it back this way.
            0x0000..=0x1FFF => chr[address as usize],
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

    /// Fill the pockets for the tile `v` names: the nametable cell
    /// `v` points at, its palette pick, and the pattern row `v`'s
    /// fine Y selects — the beam no longer knows where it is; it
    /// knows what `v` says.
    fn fetch_tile(&mut self, chr: &[u8]) {
        let v = self.v.get();

        // `v`'s low twelve bits — coarse X, coarse Y, nametable —
        // ARE a nametable cell address, once parked behind $2000.
        let tile = self.vram[self.mirror(0x2000 | (v & 0x0FFF))] as usize;

        // The attribute byte lives in the same nametable's last 64
        // bytes ($23C0 for the first), one byte per 4x4-tile block:
        // keep the nametable bits, then chop both coordinates to
        // blocks — the top three bits of coarse Y and of coarse X,
        // packed side by side.
        let attribute_address = 0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07);
        let attribute = self.vram[self.mirror(attribute_address)];

        // Inside the byte, two bits per 2x2-tile quadrant, as ever:
        // coarse Y's bit 1 contributes 4, coarse X's bit 1
        // contributes 2.
        let shift = ((v >> 4) & 0b100) | (v & 0b010);
        self.next_attr = (attribute >> shift) & 0b11;

        // PPUCTRL bit 4 picks the background's half of the album:
        // sixteen bytes a tile, the high plane eight bytes in.
        let table = if self.ctrl & 0b0001_0000 != 0 {
            0x1000
        } else {
            0
        };
        let start = table + tile * 16 + ((v >> 12) & 0b111) as usize;
        self.next_pattern_low = chr[start];
        self.next_pattern_high = chr[start + 8];
    }

    /// One tile to the right — and off the edge of a nametable means
    /// INTO ITS NEIGHBOR: coarse X wraps and the horizontal name bit
    /// flips. This is what makes two screens one world.
    fn increment_x(&self) {
        let v = self.v.get();
        if v & 0x001F == 31 {
            self.v.set((v & !0x001F) ^ 0x0400);
        } else {
            self.v.set(v + 1);
        }
    }

    /// One line down: fine Y first, then coarse Y — and row 29 is the
    /// last row of a nametable, so it wraps into the neighbor below.
    /// (Rows 30 and 31 exist — that's the attribute table's territory
    /// — and a game that scrolls into them gets the garbage it asked
    /// for, without the flip.)
    fn increment_y(&self) {
        let mut v = self.v.get();
        if v & 0x7000 != 0x7000 {
            self.v.set(v + 0x1000);
            return;
        }

        v &= !0x7000;
        let coarse_y = (v >> 5) & 0x1F;
        match coarse_y {
            29 => v = (v & !0x03E0) ^ 0x0800,
            31 => v &= !0x03E0,
            _ => v += 0x0020,
        }
        self.v.set(v);
    }

    /// The end-of-line reset: the horizontal half of `t` — coarse X
    /// and the horizontal name bit — copies back into `v`.
    fn copy_horizontal(&self) {
        let v = self.v.get();
        self.v.set((v & !0x041F) | (self.t & 0x041F));
    }

    /// The start-of-frame reset, on the warm-up lap: the vertical
    /// half of `t` copies back into `v`.
    fn copy_vertical(&self) {
        let v = self.v.get();
        self.v.set((v & 0x041F) | (self.t & !0x041F & 0x7FFF));
    }

    /// Choose the sprites for one scanline: walk the roster in order,
    /// keep the first eight whose rows cross the line, and decode
    /// each one's row of pixels on the spot. Sprite zero is whoever
    /// sits in slot zero — the flag's namesake.
    fn evaluate_sprites(&mut self, line: usize, chr: &[u8]) {
        self.line_sprites.clear();
        let height = if self.ctrl & 0b0010_0000 != 0 { 16 } else { 8 };

        let mut n = 0;
        while n < 64 {
            // OAM stores "top minus one": a sprite at Y covers
            // lines Y+1 through Y+height.
            let y = self.oam[n * 4] as usize;
            let row = line.wrapping_sub(y + 1);
            if row < height {
                if self.line_sprites.len() == 8 {
                    self.overflow_scan(line, n, height);
                    return;
                }
                self.line_sprites.push(LineSprite {
                    x: self.oam[n * 4 + 3],
                    pixels: self.sprite_row(
                        self.oam[n * 4 + 1] as usize,
                        self.oam[n * 4 + 2],
                        row,
                        height,
                        chr,
                    ),
                    attributes: self.oam[n * 4 + 2],
                    is_zero: n == 0,
                });
            }
            n += 1;
        }
    }

    /// One row of one sprite, decoded and flipped. Chapter 13's
    /// `decode_tile` finally meets the cast: flip V picks the row
    /// from the bottom up, flip H reverses it — and a tall sprite
    /// is two stacked tiles from the table its own number picks.
    fn sprite_row(
        &self,
        tile: usize,
        attributes: u8,
        mut row: usize,
        height: usize,
        chr: &[u8],
    ) -> [u8; 8] {
        // Flip V reads the sprite bottom-up: row 0 becomes the last.
        if attributes & 0b1000_0000 != 0 {
            row = height - 1 - row;
        }

        // Which tile, from which half of the album (256 tiles each).
        // A tall sprite carries the answer in its own number: the
        // bottom bit picks the half, the rest names the TOP tile,
        // and rows 8-15 come from the tile right after it. A short
        // sprite asks PPUCTRL bit 3, exactly as chapter 17 did.
        let (half, tile) = if height == 16 {
            ((tile & 1) * 256, (tile & !1) + row / 8)
        } else if self.ctrl & 0b0000_1000 != 0 {
            (256, tile)
        } else {
            (0, tile)
        };

        // Chapter 13's decoder hands the whole tile back; keep one
        // row of it — and flip H is nothing more than reversing
        // that row.
        let mut pixels = decode_tile(chr, half + tile)[row % 8];
        if attributes & 0b0100_0000 != 0 {
            pixels.reverse();
        }
        pixels
    }

    /// The ninth-sprite scan, bug included: the real chip means to
    /// keep checking Y coordinates, but it increments the byte
    /// offset alongside the sprite index — so it wanders a diagonal
    /// through the roster, comparing X positions and tile numbers as
    /// if they were Ys. Games learned to live with the flag it
    /// raises; so do we, faithfully.
    fn overflow_scan(&mut self, line: usize, from: usize, height: usize) {
        let mut m = 0;
        for n in from..64 {
            let y = self.oam[n * 4 + m] as usize;
            if line.wrapping_sub(y + 1) < height {
                self.sprite_overflow = true;
                return;
            }
            m = (m + 1) & 3;
        }
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
        // Two layers meet at every dot; the referee picks a crayon,
        // and the mask has the last word on what color it makes.
        let color = if self.rendering() {
            let background = self.background_pixel(x);
            let sprite = self.sprite_pixel(x);
            let crayon = self.composite(background, sprite, x);
            self.color(crayon)
        } else {
            // With rendering off and the address register parked
            // inside the crayon box, the screen shows THAT crayon —
            // the door `full_palette` draws its rainbow through.
            let address = self.v.get() & 0x3FFF;
            if address >= 0x3F00 {
                let crayon = self.palette_ram[Self::palette_index(address)];
                self.frame[scanline * 256 + x] = self.color(crayon);
                return;
            }

            // The background is hidden: the dot is backdrop.
            self.color(self.palette_ram[0])
        };

        self.frame[scanline * 256 + x] = color;
    }

    /// The background's offer for this dot: its pattern value and
    /// the crayon that goes with it. A hidden background — the mask's
    /// layer bit, or the left-edge window over the first eight
    /// pixels — offers value 0, which is to say: backdrop.
    fn background_pixel(&self, x: usize) -> (usize, u8) {
        if self.mask & 0b0000_1000 == 0 || (x < 8 && self.mask & 0b0000_0010 == 0) {
            return (0, self.palette_ram[0]);
        }

        // Fine X moves the tap: instead of always reading the
        // belt's front bit, the beam reads `x` bits in.
        let tap = 15 - self.x as u16;
        let low = (self.shift_low >> tap) & 1;
        let high = (self.shift_high >> tap) & 1;
        let value = ((high << 1) | low) as usize;

        let palette = (((self.attr_high >> tap) & 1) << 1 | ((self.attr_low >> tap) & 1)) as usize;

        let crayon = if value == 0 {
            self.palette_ram[0]
        } else {
            self.palette_ram[palette * 4 + value]
        };
        (value, crayon)
    }

    /// The cast's offer: the first sprite on the line with an opaque
    /// pixel at this X wins — roster order settles fights between
    /// sprites before priority ever meets the background.
    fn sprite_pixel(&self, x: usize) -> Option<(usize, u8, bool)> {
        if self.mask & 0b0001_0000 == 0 || (x < 8 && self.mask & 0b0000_0100 == 0) {
            return None;
        }

        for sprite in &self.line_sprites {
            let offset = x.wrapping_sub(sprite.x as usize);
            if offset < 8 {
                let value = sprite.pixels[offset] as usize;
                if value != 0 {
                    return Some((value, sprite.attributes, sprite.is_zero));
                }
            }
        }
        None
    }

    /// The referee. Sprite zero's opaque pixel meeting an opaque
    /// background pixel raises the famous flag — except at x=255,
    /// where the hardware never checks. Then the priority bit says
    /// who shows: a "behind" sprite loses to scenery but still shows
    /// through its holes. The answer is a CRAYON — what color it
    /// makes is between the mask and the signal.
    fn composite(
        &mut self,
        background: (usize, u8),
        sprite: Option<(usize, u8, bool)>,
        x: usize,
    ) -> u8 {
        let (bg_value, bg_crayon) = background;

        let Some((value, attributes, is_zero)) = sprite else {
            return bg_crayon;
        };

        if is_zero && bg_value != 0 && x != 255 {
            self.sprite_zero_hit = true;
        }

        let behind = attributes & 0b0010_0000 != 0;
        if bg_value != 0 && behind {
            return bg_crayon;
        }

        let palette = (attributes & 0b11) as usize;
        self.palette_ram[0x10 + palette * 4 + value]
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

        // The warm-up lap wipes last frame's score, rendering or not.
        if scanline == 261 && dot == 1 {
            self.sprite_zero_hit = false;
            self.sprite_overflow = false;
        }

        if !self.rendering() || (!drawing_line && scanline != 261) {
            return;
        }

        if drawing_line && (1..=256).contains(&dot) {
            self.shift();

            // Boundaries land after every eighth pixel; the tile
            // fetched here reaches the front sixteen dots later, and
            // `v` walks one cell right at every boundary. The fetch
            // at dot 248 reaches past the right edge — with fine X
            // panning, the last pixels borrow from the tile beyond,
            // and `v` has already wrapped into the neighbor to serve
            // them.
            if dot % 8 == 0 && dot <= 248 {
                self.fetch_tile(chr);
                self.reload_shifts();
                self.increment_x();
            }
        }

        // The line's turnarounds: at dot 256 the beam finishes its
        // pixels and `v` steps one line down; at 257 the horizontal
        // half snaps back to the game's chosen left edge. The warm-up
        // lap does both, and once a frame it also restores the
        // vertical half — the camera's Y, applied.
        if dot == 256 {
            self.increment_y();
        }
        if dot == 257 {
            self.copy_horizontal();
        }
        if scanline == 261 && dot == 280 {
            self.copy_vertical();
        }

        // While the beam turns around, the chip casts the next
        // line: which eight sprites, out of sixty-four, live there.
        // Nobody casts for line zero — a real NES shows no sprites
        // on its first line, and now neither do we.
        if drawing_line && dot == 257 {
            self.evaluate_sprites(scanline + 1, chr);
        }

        if (321..=336).contains(&dot) {
            self.shift();

            if dot == 328 || dot == 336 {
                self.fetch_tile(chr);
                self.reload_shifts();
                self.increment_x();
            }
        }
    }

    /// A crayon, seen through the mask: greyscale (bit 0) forces it
    /// onto the grey column before anything else sees it, and the
    /// top three bits pick which of the eight emphasis palettes the
    /// color is read from.
    fn color(&self, crayon: u8) -> u32 {
        let crayon = if self.mask & 0b0000_0001 != 0 {
            crayon & 0x30
        } else {
            crayon & 0x3F
        };

        let emphasis = (self.mask as usize >> 5) & 0b111;
        self.colors[(emphasis << 6) | crayon as usize]
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

/// Voltages the signal sits at for black and for white, measured
/// against the sync level — the numbers straight off the datasheet.
const BLACK_LEVEL: f32 = 0.518;
const WHITE_LEVEL: f32 = 1.962;

/// How much a de-emphasis bit darkens the samples it reaches.
const ATTENUATION: f32 = 0.746;

/// The two voltages a color's square wave swings between, by
/// brightness: the low four are the trough, the high four the crest.
const SIGNAL_LEVELS: [f32; 8] = [0.350, 0.518, 0.962, 1.550, 1.094, 1.506, 1.962, 1.962];

/// Whether sample `p` of the twelve falls in the half-cycle where
/// `hue`'s wave is high. Twelve samples make one color cycle; a hue
/// is nothing but a phase offset into it.
fn in_phase(hue: i32, p: i32) -> bool {
    (hue + p + 8) % 12 < 6
}

/// Decode one of the 512 entries — crayon in the low six bits,
/// emphasis in the top three — from the signal it would put on the
/// wire: build the square wave, darken what emphasis reaches, then
/// demodulate it the way a television would and convert to RGB.
fn decode_color(index: usize) -> u32 {
    let hue = (index & 0x0F) as i32;
    let mut level = ((index >> 4) & 0x03) as i32;
    let emphasis = (index >> 6) & 0x07;

    // Hues 14 and 15 ignore their brightness bits entirely.
    if hue > 13 {
        level = 1;
    }

    let mut low = SIGNAL_LEVELS[level as usize];
    let mut high = SIGNAL_LEVELS[4 + level as usize];

    // Hue 0 never drops and hues 13-15 never rise: the grey column
    // and the black column, where a flat wave means no color at all.
    if hue == 0 {
        low = high;
    }
    if hue >= 13 {
        high = low;
    }

    let (mut y, mut i, mut q) = (0.0f32, 0.0f32, 0.0f32);
    for p in 0..12 {
        let mut spot = if in_phase(hue, p) { high } else { low };

        // Each de-emphasis bit darkens the samples in phase with one
        // primary — red at 12, green at 4, blue at 8. Not a uniform
        // dimming: a different slice of every hue's wave, which is
        // why 512 entries cannot fold back down to 64 and a number.
        if (emphasis & 1 != 0 && in_phase(12, p))
            || (emphasis & 2 != 0 && in_phase(4, p))
            || (emphasis & 4 != 0 && in_phase(8, p))
        {
            spot *= ATTENUATION;
        }

        // Normalize against black and white, then demodulate: the
        // average is brightness, and the projections onto the color
        // carrier's two phases are the color.
        let v = (spot - BLACK_LEVEL) / (WHITE_LEVEL - BLACK_LEVEL) / 12.0;
        let phase = std::f32::consts::PI * (p as f32) / 6.0;
        y += v;
        i += v * phase.cos();
        q += v * phase.sin();
    }

    // A wave averaged against itself over a cycle comes out at half
    // strength; put the halves back.
    i *= 2.0;
    q *= 2.0;

    // YIQ to RGB by the broadcast matrix, with the gamma bend a CRT
    // would have applied — skip it and everything washes out.
    let channel = |value: f32| -> u32 {
        let bent = if value <= 0.0 {
            0.0
        } else {
            value.powf(2.2 / 1.8)
        };
        (bent * 255.0).clamp(0.0, 255.0) as u32
    };

    let r = channel(y + 0.946_882 * i + 0.623_557 * q);
    let g = channel(y - 0.274_788 * i - 0.635_691 * q);
    let b = channel(y - 1.108_545 * i + 1.709_007 * q);
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crayon box, exactly as chapter 14 typed it. Its one job
    /// is to be the answer key the signal decoding must reproduce,
    /// entry for entry.
    const SYSTEM_PALETTE: [u32; 64] = [
        0x525252, 0x001E94, 0x0907C2, 0x3100BD, 0x580086, 0x6F0036, 0x6C0000, 0x501000, 0x272900,
        0x023F00, 0x004B00, 0x004700, 0x003646, 0x000000, 0x000000, 0x000000, 0xA0A0A0, 0x004FFF,
        0x2C2AFF, 0x6D0FFF, 0xA905ED, 0xCB0775, 0xC61909, 0x9D3900, 0x5E6100, 0x208300, 0x009400,
        0x008F1A, 0x00758D, 0x000000, 0x000000, 0x000000, 0xFEFEFE, 0x3EA4FF, 0x7B78FF, 0xC656FF,
        0xFF46FF, 0xFF4ACE, 0xFF634C, 0xFB8B00, 0xB5B800, 0x6BDE00, 0x34F205, 0x1AEC63, 0x1ECFEA,
        0x3C3C3C, 0x000000, 0x000000, 0xFEFEFE, 0xAAD8FF, 0xC6C5FF, 0xE7B5FF, 0xFFADFF, 0xFFB0EA,
        0xFFBBB1, 0xFDCD82, 0xDFE16A, 0xBFF16D, 0xA5F98A, 0x97F7BC, 0x99EBF6, 0xA9A9A9, 0x000000,
        0x000000,
    ];

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
        ppu.read_data(&[]); // a footstep past $3F01, touching nothing
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
        assert_eq!(ppu.read_data(&[]), 0x00); // the stale waiting room
        assert_eq!(ppu.read_data(&[]), 0x55); // yesterday's byte, today
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
        ppu.mask = 0b0000_1010; // background on, left window open
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

    #[test]
    fn scroll_writes_stage_t_and_x_without_touching_v() {
        // The NesDev wiki's own worked example: $7D then $5E.
        let mut ppu = Ppu::new(false);
        ppu.write_scroll(0x7D); // coarse X 15, fine X 5
        ppu.write_scroll(0x5E); // coarse Y 11, fine Y 6
        assert_eq!(ppu.t, (6 << 12) | (11 << 5) | 15);
        assert_eq!(ppu.x, 5);
        assert_eq!(ppu.v.get(), 0); // staged, not applied
    }

    #[test]
    fn the_second_address_knock_moves_the_camera() {
        let mut ppu = Ppu::new(false);
        ppu.write_address(0x21);
        assert_eq!(ppu.v.get(), 0); // one knock: still staged
        ppu.write_address(0x08);
        assert_eq!(ppu.v.get(), 0x2108); // two knocks: applied
    }

    #[test]
    fn walking_off_a_nametable_lands_in_its_neighbor() {
        let ppu = Ppu::new(false);

        // Right edge: coarse X 31 wraps to 0 and the horizontal
        // name bit flips.
        ppu.v.set(31);
        ppu.increment_x();
        assert_eq!(ppu.v.get(), 0x0400);

        // Bottom edge: fine Y 7 on row 29 wraps to row 0 of the
        // nametable below.
        ppu.v.set((7 << 12) | (29 << 5));
        ppu.increment_y();
        assert_eq!(ppu.v.get(), 0x0800);
    }

    #[test]
    fn the_warm_up_lap_applies_the_staged_camera() {
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0000_1010; // background on, left window open
        ppu.vram[2] = 1; // two cells in: the solid tile
        ppu.palette_ram[3] = 0x30;

        // Aim two tiles right; nothing moves yet.
        ppu.write_scroll(16);
        ppu.write_scroll(0);

        // The warm-up lap's copy-downs apply it, then the usual
        // priming — and the line starts two tiles into the world.
        let chr = test_chr();
        ppu.tick(261, 257, &chr);
        ppu.tick(261, 280, &chr);
        for dot in 321..=336 {
            ppu.tick(261, dot, &chr);
        }
        for dot in 1..=8 {
            ppu.tick(0, dot, &chr);
        }
        assert_eq!(ppu.frame[0], SYSTEM_PALETTE[0x30]);
    }

    #[test]
    fn fine_x_slides_the_world_within_a_tile() {
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0000_1010; // background on, left window open
        ppu.vram[0] = 1; // solid, then blank
        ppu.palette_ram[0] = 0x0F;
        ppu.palette_ram[3] = 0x30;

        ppu.write_scroll(4); // coarse 0, fine 4
        ppu.write_scroll(0);

        let chr = test_chr();
        ppu.tick(261, 257, &chr);
        ppu.tick(261, 280, &chr);
        for dot in 321..=336 {
            ppu.tick(261, dot, &chr);
        }
        for dot in 1..=8 {
            ppu.tick(0, dot, &chr);
        }

        // The screen's first pixel is the tile's FIFTH: four white
        // pixels remain, then the blank neighbor shows through.
        assert_eq!(ppu.frame[3], SYSTEM_PALETTE[0x30]);
        assert_eq!(ppu.frame[4], SYSTEM_PALETTE[0x0F]);
    }

    /// Stand a sprite in the roster: the solid tile, at a position,
    /// with its manners.
    fn place_sprite(ppu: &mut Ppu, slot: usize, x: u8, y: u8, attributes: u8) {
        ppu.oam[slot * 4] = y;
        ppu.oam[slot * 4 + 1] = 1;
        ppu.oam[slot * 4 + 2] = attributes;
        ppu.oam[slot * 4 + 3] = x;
    }

    /// Draw one full line the honest way: prime on the warm-up lap,
    /// walk line 0 (which casts and primes line 1), then line 1.
    fn draw_line_one(ppu: &mut Ppu, chr: &[u8]) {
        for dot in 321..=336 {
            ppu.tick(261, dot, chr);
        }
        for dot in 1..=340 {
            ppu.tick(0, dot, chr);
        }
        for dot in 1..=256 {
            ppu.tick(1, dot, chr);
        }
    }

    #[test]
    fn flips_mirror_the_sprite_not_just_its_position() {
        // Zooming Secretary walked left in two pieces: her tiles
        // swapped positions but nobody read bit 6, so each half
        // faced the wrong way. This pins the mirror forever.
        let mut chr = vec![0; 8192];
        for row in 0..8 {
            chr[16 + row] = 0xF0; // tile 1: left half lit...
            chr[24 + row] = 0xF0; // ...in both planes: value 3
        }
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0001_1110;
        ppu.palette_ram[0] = 0x0F;
        ppu.palette_ram[0x13] = 0x30;

        place_sprite(&mut ppu, 0, 100, 49, 0);
        ppu.evaluate_sprites(50, &chr);
        for dot in 1..=256 {
            ppu.tick(50, dot, &chr);
        }
        let row = &ppu.frame[50 * 256..51 * 256];
        assert_eq!(row[100], SYSTEM_PALETTE[0x30]); // lit half left
        assert_eq!(row[107], SYSTEM_PALETTE[0x0F]);

        place_sprite(&mut ppu, 0, 100, 49, 0b0100_0000); // flip H
        ppu.evaluate_sprites(50, &chr);
        for dot in 1..=256 {
            ppu.tick(50, dot, &chr);
        }
        let row = &ppu.frame[50 * 256..51 * 256];
        assert_eq!(row[100], SYSTEM_PALETTE[0x0F]); // now mirrored
        assert_eq!(row[107], SYSTEM_PALETTE[0x30]);
    }

    #[test]
    fn sprite_zero_reports_the_meeting() {
        let chr = test_chr();
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0001_1110;
        ppu.vram[0] = 1; // scenery under the sprite's left half

        place_sprite(&mut ppu, 0, 4, 0, 0); // covers lines 1-8
        draw_line_one(&mut ppu, &chr);
        assert!(ppu.sprite_zero_hit);

        // No scenery, no meeting: same sprite over a blank world.
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0001_1110;
        place_sprite(&mut ppu, 0, 4, 0, 0);
        draw_line_one(&mut ppu, &chr);
        assert!(!ppu.sprite_zero_hit);
    }

    #[test]
    fn a_ninth_actor_raises_the_overflow_flag() {
        let chr = test_chr();
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0001_1000;
        for slot in 0..8 {
            place_sprite(&mut ppu, slot, (slot * 8) as u8, 49, 0);
        }
        ppu.evaluate_sprites(50, &chr);
        assert!(!ppu.sprite_overflow);

        place_sprite(&mut ppu, 8, 64, 49, 0);
        ppu.evaluate_sprites(50, &chr);
        assert!(ppu.sprite_overflow);
    }

    #[test]
    fn a_behind_sprite_hides_in_scenery_and_shows_in_holes() {
        let chr = test_chr();
        let mut ppu = Ppu::new(false);
        ppu.mask = 0b0001_1110;
        ppu.palette_ram[0] = 0x0F;
        ppu.palette_ram[3] = 0x21;
        ppu.palette_ram[0x13] = 0x30;
        ppu.vram[1] = 1; // scenery over x8-15 only

        place_sprite(&mut ppu, 0, 12, 0, 0b0010_0000); // behind
        draw_line_one(&mut ppu, &chr);
        let row = &ppu.frame[256..512];
        assert_eq!(row[13], SYSTEM_PALETTE[0x21]); // scenery wins
        assert_eq!(row[17], SYSTEM_PALETTE[0x30]); // holes don't
    }

    #[test]
    fn the_roster_door_reads_writes_and_masks() {
        let mut ppu = Ppu::new(false);
        ppu.write_oam_address(5);
        ppu.write_oam_data(0x77);
        assert_eq!(ppu.oam[5], 0x77);

        ppu.write_oam_address(6); // an attribute byte
        ppu.write_oam_data(0xFF);
        ppu.write_oam_address(6);
        assert_eq!(ppu.read_oam_data(), 0xE3); // unwired bits: zero
    }

    #[test]
    fn the_left_window_hides_the_cast() {
        let chr = test_chr();
        let mut ppu = Ppu::new(false);
        ppu.palette_ram[0x13] = 0x30;
        place_sprite(&mut ppu, 0, 2, 0, 0);

        ppu.mask = 0b0001_0000; // sprites on, their window closed
        ppu.evaluate_sprites(1, &chr);
        for dot in 1..=8 {
            ppu.tick(1, dot, &chr);
        }
        assert_ne!(ppu.frame[256 + 3], SYSTEM_PALETTE[0x30]);

        ppu.mask = 0b0001_0100; // window open
        for dot in 1..=8 {
            ppu.tick(1, dot, &chr);
        }
        assert_eq!(ppu.frame[256 + 3], SYSTEM_PALETTE[0x30]);
    }

    #[test]
    fn the_computation_reproduces_chapter_14s_table() {
        // The table you typed in chapter 14 was the output of this
        // exact decoding, done ahead of time. Now the machine does
        // its own homework — and must get the same 64 answers.
        let ppu = Ppu::new(false);
        for crayon in 0..64 {
            assert_eq!(
                ppu.colors[crayon], SYSTEM_PALETTE[crayon],
                "crayon {crayon:#04X} decoded differently than the table"
            );
        }
    }

    #[test]
    fn every_emphasis_setting_is_its_own_palette() {
        let ppu = Ppu::new(false);
        for emphasis in 1..8 {
            let plain = &ppu.colors[..64];
            let dimmed = &ppu.colors[emphasis * 64..emphasis * 64 + 64];
            assert_ne!(plain, dimmed);
        }
    }

    #[test]
    fn the_mask_greys_and_dims() {
        let mut ppu = Ppu::new(false);
        let plain = ppu.color(0x21);

        ppu.mask = 0b0000_0001; // greyscale: the hue drops away
        assert_eq!(ppu.color(0x21), ppu.color(0x20));

        ppu.mask = 0b0010_0000; // red emphasis: same crayon, dimmed
        assert_ne!(ppu.color(0x21), plain);
    }

    #[test]
    fn the_forbidden_crayon_has_grey_cousins() {
        // Chapter 14's curious box: $0D sits below black. Its column
        // is not all black, though — $3D is a true light grey. The
        // signal knows the difference even where a table shrugs.
        let ppu = Ppu::new(false);
        assert_eq!(ppu.colors[0x0D], 0);
        assert!(ppu.colors[0x3D] >> 16 > 100);
    }
}
