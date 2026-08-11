// Every file in `src/` is a "module": a drawer of related code.
// This line tells Rust that the drawer `src/cpu.rs` exists and is part of
// our program.
mod cpu;
// The drawer for everything that comes out of a .nes file.
mod cartridge;
// The wiring between the CPU and everything else.
mod bus;

use minifb::{Key, Scale, Window, WindowOptions};
use cpu::Cpu;
use cartridge::Cartridge;

/// The NES picture is exactly 256 pixels wide...
const WIDTH: usize = 256;

/// ...and 240 pixels tall.
const HEIGHT: usize = 240;

fn main() {
    // One path on the command line boots it. A second path is a golden
    // log to grade ourselves against.
    let args: Vec<String> = std::env::args().collect();
    match args.len() {
        2 => boot_rom(&args[1]),
        3 => nestest_diff(&args[1], &args[2]),
        _ => {}
    }

    // The frame buffer: one number for every pixel on our screen.
    // 0 means black, so right now this is a picture of nothing.
    let mut buffer = vec![0u32; WIDTH * HEIGHT];

    draw_test_pattern(&mut buffer);

    draw_tile_grid(&mut buffer);

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

/// Load a .nes file, wire a whole machine around it, press reset, and
/// watch the first dozen instructions run.
fn boot_rom(path: &str) {
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
