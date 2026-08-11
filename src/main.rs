// Every file in `src/` is a "module": a drawer of related code.
// This line tells Rust that the drawer `src/cpu.rs` exists and is part of
// our program.
mod cpu;
// The drawer for everything that comes out of a .nes file.
mod cartridge;
// The wiring between the CPU and everything else.
mod bus;
// The picture chip.
mod ppu;

use minifb::{Key, Scale, Window, WindowOptions};
use cpu::Cpu;
use cartridge::Cartridge;

/// The NES picture is exactly 256 pixels wide...
const WIDTH: usize = 256;

/// ...and 240 pixels tall.
const HEIGHT: usize = 240;

fn main() {
    // One path on the command line boots a machine — and now we KEEP it.
    // Two paths run the grader instead.
    let args: Vec<String> = std::env::args().collect();
    let mut machine = match args.len() {
        2 => Some(boot_rom(&args[1])),
        3 => {
            nestest_diff(&args[1], &args[2]);
            None
        }
        _ => None,
    };

    // The frame buffer: one number for every pixel on our screen.
    // 0 means black, so right now this is a picture of nothing.
    let mut buffer = vec![0u32; WIDTH * HEIGHT];

    // No machine? The test pattern still earns its keep. With one, the
    // window loop below runs the game and draws what IT painted.
    if machine.is_none() {
        draw_test_pattern(&mut buffer);
        draw_tile_grid(&mut buffer);
    }

    // Ask the operating system for a window. `Scale::X2` doubles every pixel,
    // because 256x240 is tiny on a modern screen.
    let mut window = Window::new(
        "graybox",
        WIDTH,
        HEIGHT,
        WindowOptions {
            scale: Scale::X2,
            ..WindowOptions::default()
        },
    )
    .expect("could not open a window");

    // A real NES shows 60 pictures every second. So will we.
    window.set_target_fps(60);

    // Show the buffer, over and over, until the window is closed
    // or Esc is pressed.
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // One trip around this loop is one frame: the game computes
        // while the "beam" draws, rests briefly in vblank, and then we
        // paint what it described. The instruction counts are a crude
        // stand-in for real timing — chapter 18 earns the real thing.
        if let Some(cpu) = &mut machine {
            for _ in 0..10_000 {
                cpu.step();
            }
            cpu.bus.ppu.vblank.set(true);
            // A game that armed PPUCTRL bit 7 gets vblank hand-delivered:
            // the tap on the shoulder. (Its handler ends in RTI — which
            // chapter 12's grader made us build. Everything connects.)
            if cpu.bus.ppu.ctrl & 0b1000_0000 != 0 {
                cpu.nmi();
            }
            for _ in 0..1_000 {
                cpu.step();
            }
            render_background(cpu, &mut buffer);
        }

        window
            .update_with_buffer(&buffer, WIDTH, HEIGHT)
            .expect("could not draw to the window");
    }
}

/// Fill the whole buffer with a smooth color gradient, so we can SEE that
/// every pixel is under our control.
fn draw_test_pattern(buffer: &mut [u32]) {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            // Red grows from left to right. x already counts 0..=255,
            // which is exactly the range a color part can hold.
            let red = x as u32;

            // Green grows from top to bottom, stretched to reach 255
            // on the last row.
            let green = (y as u32 * 255) / (HEIGHT as u32 - 1);

            // A fixed dash of blue everywhere.
            let blue = 128;

            // Pack the three parts into one number: 0x00RRGGBB.
            let color = (red << 16) | (green << 8) | blue;

            buffer[y * WIDTH + x] = color;
        }
    }
}

/// Darken every 8th row and column. The NES builds its whole picture out
/// of 8x8 tiles; this grid marks their territory.
fn draw_tile_grid(buffer: &mut [u32]) {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if x % 8 == 0 || y % 8 == 0 {
                let color = buffer[y * WIDTH + x];

                // `>> 1` halves every color part at once, darkening the
                // pixel. The mask clears the bits that spilled over from
                // one part into its neighbor.
                buffer[y * WIDTH + x] = (color >> 1) & 0x007F_7F7F;
            }
        }
    }
}

/// Load a .nes file, wire a whole machine around it, press reset, watch
/// the first dozen instructions run — and hand the machine back.
fn boot_rom(path: &str) -> Cpu {
    let bytes = std::fs::read(path).expect("could not read the file");
    let cartridge = Cartridge::load(&bytes).expect("could not parse the file");

    println!();
    println!("  {path}");
    println!(
        "  PRG ROM: {} KiB, CHR ROM: {} KiB, mapper {}",
        cartridge.prg_rom.len() / 1024,
        cartridge.chr_rom.len() / 1024,
        cartridge.mapper,
    );

    let mut cpu = Cpu::new(cartridge);
    cpu.reset();

    for _ in 0..12 {
        if let Some(line) = cpu.trace() {
            println!("  {line}");
        }
        cpu.step();
    }
    println!("  ...and on it goes.");
    // Hand the machine back to whoever booted it.
    cpu
}

/// Paint the background the game described: the nametable says which
/// tile goes in each of the 30 rows of 32 cells; PPUCTRL says which
/// pattern table backgrounds shop from.
fn render_background(cpu: &Cpu, buffer: &mut [u32]) {
    let ppu = &cpu.bus.ppu;
    let chr = &cpu.bus.cartridge.chr_rom;

    // PPUCTRL bit 4: which half of the album holds background tiles.
    let table = if ppu.ctrl & 0b0001_0000 != 0 { 256 } else { 0 };

    for row in 0..30 {
        for col in 0..32 {
            let tile = ppu.vram[row * 32 + col] as usize;
            let pixels = ppu::decode_tile(chr, table + tile);
            let palette = background_palette(ppu, row, col);

            for y in 0..8 {
                for x in 0..8 {
                    let crayon = palette[pixels[y][x] as usize];
                    let color = ppu::SYSTEM_PALETTE[crayon as usize];
                    buffer[(row * 8 + y) * WIDTH + col * 8 + x] = color;
                }
            }
        }
    }
}

/// Which four crayons a background tile paints with: the attribute
/// table's two bits pick one of the four background palettes, and
/// crayon 0 is always the shared backdrop color.
fn background_palette(ppu: &ppu::Ppu, row: usize, col: usize) -> [u8; 4] {
    // One attribute byte governs a 4x4-tile block; within it, two bits
    // per 2x2-tile quadrant.
    let attribute = ppu.vram[0x3C0 + (row / 4) * 8 + col / 4];
    let shift = ((row % 4) / 2) * 4 + ((col % 4) / 2) * 2;
    let which = ((attribute >> shift) & 0b11) as usize;

    [
        ppu.palette_ram[0], // the backdrop, everyone's crayon 0
        ppu.palette_ram[which * 4 + 1],
        ppu.palette_ram[which * 4 + 2],
        ppu.palette_ram[which * 4 + 3],
    ]
}

/// Run nestest in automation mode and grade our CPU against a golden
/// log, line by line. The first difference is the truth.
fn nestest_diff(rom_path: &str, log_path: &str) {
    let bytes = std::fs::read(rom_path).expect("could not read the ROM");
    let cartridge = Cartridge::load(&bytes).expect("could not parse the ROM");
    let golden = std::fs::read_to_string(log_path).expect("could not read the log");

    let mut cpu = Cpu::new(cartridge);
    cpu.reset();
    // nestest's automation mode: start at $C000 instead of the vector,
    // and the tests run with no picture chip needed at all.
    cpu.pc = 0xC000;

    let mut matched = 0;
    for line in golden.lines() {
        // The fields both sides have: PC and the five pockets. (The log
        // also carries cycle counts — a Part II subject.)
        let ours = format!(
            "{:04X} A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X}",
            cpu.pc, cpu.a, cpu.x, cpu.y, cpu.status, cpu.sp
        );
        let theirs = format!("{} {}", &line[0..4], &line[48..73]);

        if ours != theirs {
            println!("MISMATCH after {matched} matching lines.");
            println!("  golden: {theirs}");
            println!("  ours:   {ours}");
            println!("  the golden line in full:");
            println!("  {line}");
            return;
        }
        matched += 1;

        if Cpu::opcode_name_and_length(cpu.read(cpu.pc)).is_none() {
            println!(
                "{matched} lines matched — then {:#04X}, an opcode we",
                cpu.read(cpu.pc)
            );
            println!("don't implement. Unofficial. A Part II story.");
            return;
        }
        cpu.step();
    }
    println!("all {matched} lines matched");
}
