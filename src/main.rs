// Every file in `src/` is a "module": a drawer of related code.
// This line tells Rust that the drawer `src/cpu.rs` exists and is part of
// our program.
mod cpu;

use minifb::{Key, Scale, Window, WindowOptions};
use cpu::Cpu;

/// The NES picture is exactly 256 pixels wide...
const WIDTH: usize = 256;

/// ...and 240 pixels tall.
const HEIGHT: usize = 240;

fn main() {
    // Give the CPU a moment in the terminal before the window steals
    // the show.
    cpu_demo();

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

/// Load a little machine-code program, wake the CPU, run it to the end,
/// and print every pocket after every instruction.
fn cpu_demo() {
    // A program that goes BACKWARDS: count X up to three, by looping
    // over the same instructions.
    //
    // bytes    meaning
    // A9 00    LDA #0     start A at zero
    // AA       TAX        X = 0 too
    // E8       INX        add 1 to X          <-- $8003, the loop target
    // E0 03    CPX #3     is X 3 yet?
    // D0 FB    BNE -5     not yet? back to INX
    // 00       BRK        done: stop
    let program = [0xA9, 0x00, 0xAA, 0xE8, 0xE0, 0x03, 0xD0, 0xFB, 0x00];

    let mut cpu = Cpu::new();

    // Copy the program into memory at $8000 — the neighborhood where
    // cartridge programs traditionally live.
    for (i, byte) in program.iter().enumerate() {
        cpu.write(0x8000 + i as u16, *byte);
    }

    // Write $8000 into the reset vector, little end first,
    // so the CPU wakes up inside our program.
    cpu.write(0xFFFC, 0x00);
    cpu.write(0xFFFD, 0x80);

    cpu.reset();

    // Run it, printing every pocket after every instruction.
    println!("  pc     a  x  y");
    println!("  ---------------");
    loop {
        let keep_going = cpu.step();
        println!("  {:04X}   {:02X} {:02X} {:02X}", cpu.pc, cpu.a, cpu.x, cpu.y);
        if !keep_going {
            println!("  (BRK: the program is done)");
            break;
        }
    }
}
