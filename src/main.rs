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
// The player's end of the machine.
mod controller;
// The sound chip.
mod apu;
// The metronome the whole machine marches to.
mod clock;

use minifb::{Key, Scale, Window, WindowOptions};
// The speaker's plumbing: a queue of samples, shared with the audio
// thread behind a lock.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
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

    // Open the speaker. `None` — no audio device, or a grumpy one —
    // just means a silent movie; the game must still run.
    let audio = start_audio();

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
        if let Some(cpu) = &mut machine {
            // The keyboard becomes the controller — one key per
            // button, in the console's own order: A, B, Select,
            // Start, then the four directions.
            let keys = [
                Key::X,     // A
                Key::Z,     // B
                Key::Space, // Select
                Key::Enter, // Start
                Key::Up,
                Key::Down,
                Key::Left,
                Key::Right,
            ];

            let mut buttons = 0;
            for (bit, key) in keys.iter().enumerate() {
                if window.is_key_down(*key) {
                    buttons |= 1 << bit;
                }
            }

            cpu.bus.controller.buttons = buttons;

            // One trip around this loop is one frame, measured by the
            // machine's own metronome. The vblank flag and the NMI are
            // the clock's business, handled on their exact dots inside
            // `step` — this loop only asks when the frame is over.
            let frame = cpu.bus.clock.frame;
            while cpu.bus.clock.frame == frame {
                if !cpu.step() {
                    break; // a jammed machine keeps its last picture
                }
            }

            // The picture was painted during the frame, one dot at a
            // time; the window just collects it.
            buffer.copy_from_slice(&cpu.bus.ppu.frame);

            // A frame of picture gets its sixtieth of a second
            // of sound.
            if let Some(speaker) = &audio {
                speaker.sing_a_frame(&mut cpu.bus.apu);
            }
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

/// The speaker's end of the machine: the stream that plays (sound
/// stops the moment it is dropped), the queue of samples on their
/// way out, and the rate the device wants them at.
struct Speaker {
    _stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: f32,
}

impl Speaker {
    /// One frame of sound to match a frame of picture: a sixtieth of
    /// a second of samples, sung from wherever the channels stand.
    /// The cap keeps a slow frame from letting the queue — and the
    /// lag behind the picture — grow forever.
    fn sing_a_frame(&self, apu: &mut apu::Apu) {
        let mut queue = self.queue.lock().unwrap();
        if queue.len() < self.sample_rate as usize / 15 {
            for _ in 0..self.sample_rate as usize / 60 {
                queue.push_back(apu.sample(self.sample_rate));
            }
        }
    }
}

/// Ask the operating system for the speaker, and start it playing
/// from its queue.
fn start_audio() -> Option<Speaker> {
    let device = cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    let sample_rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;

    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let feed = Arc::clone(&queue);

    // The sound card calls this little function whenever it wants
    // more sound — from its own thread, on its own schedule. Whatever
    // is queued gets played; an empty queue plays silence, because a
    // speaker with nothing to say should say nothing.
    let stream = device
        .build_output_stream(
            &config.into(),
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut queue = feed.lock().unwrap();
                for frame in out.chunks_mut(channels) {
                    let sample = queue.pop_front().unwrap_or(0.0);
                    for speaker in frame {
                        *speaker = sample;
                    }
                }
            },
            |error| eprintln!("the speaker complained: {error}"),
            None,
        )
        .ok()?;

    stream.play().ok()?;
    Some(Speaker { _stream: stream, queue, sample_rate })
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
        // PC, the five pockets — and now the odometer. The golden
        // log's last column has waited six chapters for this.
        let ours = format!(
            "{:04X} A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X} CYC:{}",
            cpu.pc, cpu.a, cpu.x, cpu.y, cpu.status, cpu.sp, cpu.cycles
        );
        let golden_cycles = line.split("CYC:").nth(1).unwrap_or("?");
        let theirs = format!("{} {} CYC:{}", &line[0..4], &line[48..73], golden_cycles);

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
