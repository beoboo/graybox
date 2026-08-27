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
// The debugger's typeface.
mod font;
// The picture the window shows.
mod canvas;
// Stopping the machine, and looking inside.
mod debugger;
// Bytes back into instructions.
mod disasm;

use minifb::{Key, KeyRepeat, Scale, Window, WindowOptions};
// The speaker's plumbing: a queue of samples, shared with the audio
// thread behind a lock.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use canvas::Canvas;
use cartridge::Cartridge;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpu::Cpu;
use debugger::{Debugger, Step};
use debugger::{Mode, Page, Trap};
use font::Font;

/// The NES picture is exactly 256 pixels wide...
const WIDTH: usize = 256;

/// ...and 240 pixels tall.
const HEIGHT: usize = 240;

fn main() {
    // One path on the command line boots a machine — and now we KEEP it.
    // Two paths run the grader instead.
    let args: Vec<String> = std::env::args().collect();
    // The command line, sorted: file paths first, then the switches
    // that ask for a picture instead of a window.
    let (paths, options) = parse_args(&args[1..]);
    let mut machine = match paths.len() {
        1 => Some(boot_rom(&paths[0])),
        2 => {
            nestest_diff(&paths[0], &paths[1]);
            None
        }
        _ => None,
    };

    // A picture instead of a window: run, paint, write, and leave.
    if let (Some(cpu), Some(out)) = (machine.as_mut(), &options.out) {
        headless(cpu, &options, out);
        return;
    }

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

    // The debugger and its typeface, ready before the first frame.
    let font = load_font();
    let mut debugger = Debugger::new();

    // The window is as big as the picture it shows. That changes when
    // the debugger opens, so the size is remembered to notice.
    let mut window = open_window(WIDTH, HEIGHT);
    let mut shown = (WIDTH, HEIGHT);

    // Show the buffer, over and over, until the window is closed
    // or Esc is pressed.
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // Tab opens the debugger and stops the machine, or closes it and
        // lets the machine go; the step keys move it while it is open.
        // Tab counts once per press. The step keys keep counting while
        // held, so holding one runs the machine in slow motion.
        if window.is_key_pressed(Key::Tab, KeyRepeat::No) {
            debugger.toggle();
        }
        // Every other key is the debugger's: a step, a trap, or a digit
        // of one.
        for key in window.get_keys_pressed(KeyRepeat::Yes) {
            debugger.press(key);
        }

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

            // How far the machine moves is the debugger's call: a whole
            // frame while it runs, one step when asked, nothing at all
            // while it stands still.
            debugger.run(cpu);

            // The picture was painted during the frame, one dot at a
            // time; the window just collects it.
            buffer.copy_from_slice(&cpu.bus.ppu.frame);

            // A frame of picture gets its sixtieth of a second
            // of sound.
            if let Some(speaker) = &audio {
                speaker.sing_a_frame(&mut cpu.bus.apu);
            }
        }

        // What the window shows: the picture alone, or the picture with
        // the debugger's sidebar. A window cannot change size once it is
        // open, so a canvas of a new shape gets a new window, opened
        // where the old one stood.
        let canvas = compose(&buffer, machine.as_ref(), &debugger, &font);
        if (canvas.width, canvas.height) != shown {
            let (x, y) = window.get_position();
            window = open_window(canvas.width, canvas.height);
            window.set_position(x, y);
            shown = (canvas.width, canvas.height);
        }
        window
            .update_with_buffer(&canvas.pixels, canvas.width, canvas.height)
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
    Some(Speaker {
        _stream: stream,
        queue,
        sample_rate,
    })
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

        // The early exit is gone: there is no opcode this machine
        // doesn't know. The Part II story was this chapter.
        cpu.step();
    }
    println!("all {matched} lines matched");
}

/// The debugger's typeface, `fonts/font8x8.bin`. Without it there is no
/// text to draw, so its absence is worth stopping for.
fn load_font() -> Font {
    Font::load("fonts/font8x8.bin").unwrap_or_else(|why| {
        eprintln!("{why}");
        eprintln!("the debugger needs font8x8.bin in fonts/");
        std::process::exit(1);
    })
}

/// Ask the operating system for a window of `width` by `height` pixels.
/// `Scale::X2` doubles every pixel, because 256x240 is tiny on a modern
/// screen; a real NES shows 60 pictures every second, and so will we.
fn open_window(width: usize, height: usize) -> Window {
    let mut window = Window::new(
        "graybox",
        width,
        height,
        WindowOptions {
            scale: Scale::X2,
            ..WindowOptions::default()
        },
    )
    .expect("could not open a window");
    window.set_target_fps(60);
    window
}

/// How much room the debugger's panels get beside the picture: a gutter,
/// twenty-eight characters of text, a margin.
const SIDEBAR: usize = 8 + 28 * 8 + 8;

/// The picture for the window: the game's frame alone, or — with the
/// debugger open — the frame with a sidebar for the panels.
fn compose(frame: &[u32], cpu: Option<&Cpu>, debugger: &Debugger, font: &Font) -> Canvas {
    match cpu {
        Some(cpu) if debugger.open => {
            let mut canvas = Canvas::new(WIDTH + SIDEBAR, HEIGHT);
            canvas.fill(WIDTH, 0, SIDEBAR, HEIGHT, debugger::PANEL);
            canvas.blit(0, 0, WIDTH, frame);
            debugger.paint_registers(&mut canvas, font, cpu, WIDTH + 8, 4);
            match debugger.page {
                Page::Listing => debugger.paint_listing(&mut canvas, font, cpu, WIDTH + 8, 78),
                Page::Ledger => debugger.paint_ledger(&mut canvas, font, WIDTH + 8, 78),
            }
            debugger.paint_status(&mut canvas, font, WIDTH + 8, HEIGHT - 32);

            debugger.paint_keys(&mut canvas, font, WIDTH + 8, HEIGHT - 24);

            canvas
        }
        _ => {
            let mut canvas = Canvas::new(WIDTH, HEIGHT);
            canvas.blit(0, 0, WIDTH, frame);
            canvas
        }
    }
}

/// Run without a window: the frames asked for, then one picture — the
/// game alone, or beside the debugger's panels — written to `out` as a
/// PPM file.
fn headless(cpu: &mut Cpu, options: &Options, out: &str) {
    let font = load_font();
    let mut debugger = Debugger::new();
    // A trap set before the frames run springs as often as it likes
    // during them; the ledger keeps every time. The picture is taken
    // at the next stop after the frames — the panels open by then.
    if let Some(trap) = options.trap {
        debugger.arm(trap);
    }

    // The frames, counted on the clock: a sprung trap ends a trip round
    // this loop early, and the machine is let go again until the frames
    // are all run. Then it runs to the next stop — or gives up after a
    // second of nothing springing.
    while cpu.bus.clock.frame < options.frames {
        debugger.run(cpu);
        if debugger.mode == Mode::Paused {
            debugger.resume();
        }
    }
    if options.trap.is_some() {
        while debugger.mode == Mode::Running && cpu.bus.clock.frame < options.frames + 60 {
            debugger.run(cpu);
        }
    }
    if options.ledger {
        debugger.page = Page::Ledger;
    }

    if options.debug {
        debugger.toggle();
    }

    // Stopping partway down the next frame: open the debugger if it is
    // not, then step a scanline at a time until the beam arrives.
    if let Some(line) = options.line {
        if !debugger.open {
            debugger.toggle();
        }
        while cpu.bus.clock.scanline != line {
            debugger.step(Step::Scanline);
            debugger.run(cpu);
        }
    }

    let machine: &Cpu = cpu;
    let canvas = compose(&machine.bus.ppu.frame, Some(machine), &debugger, &font);
    std::fs::write(out, write_ppm(&canvas)).expect("could not write the picture");
    println!("  {} frames, then {out}", options.frames);
}

/// A canvas as a PPM file: a three-line header, then one byte per color
/// part — red, green, blue — row after row. The plainest picture format
/// there is, which is why no library is needed to write it.
fn write_ppm(canvas: &Canvas) -> Vec<u8> {
    let mut file = format!("P6\n{} {}\n255\n", canvas.width, canvas.height).into_bytes();
    for &pixel in &canvas.pixels {
        file.extend([(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8]);
    }
    file
}

/// The switches a picture needs. Without them the window opens as always.
struct Options {
    /// Frames to run before the picture is taken.
    frames: u64,
    /// Where to write the picture; `None` means open a window instead.
    out: Option<String>,
    /// Take the picture with the debugger's panels open.
    debug: bool,
    /// Stop the beam on this scanline of the frame after the last — the
    /// picture half painted, the way the debugger shows it.
    line: Option<u16>,
    /// A trap to arm before the frames run: `--break ADDR` or
    /// `--watch ADDR`, in hex.
    trap: Option<Trap>,
    /// Take the picture with the ledger page up instead of the listing.
    ledger: bool,
}

/// Split the command line into file paths and switches — `--frames N`,
/// `--out FILE`, `--debug`, `--line N`. Anything else is a path.
fn parse_args(args: &[String]) -> (Vec<String>, Options) {
    let mut paths = Vec::new();
    let mut options = Options {
        frames: 60,
        out: None,
        debug: false,
        line: None,
        trap: None,
        ledger: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frames" => {
                options.frames = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(60);
                i += 1;
            }
            "--out" => {
                options.out = args.get(i + 1).cloned();
                i += 1;
            }
            "--debug" => options.debug = true,
            "--line" => {
                options.line = args.get(i + 1).and_then(|n| n.parse().ok());
                i += 1;
            }
            "--break" | "--watch" => {
                let address = args
                    .get(i + 1)
                    .and_then(|n| u16::from_str_radix(n, 16).ok());
                options.trap = address.map(|address| match args[i].as_str() {
                    "--break" => Trap::Execute(address),
                    _ => Trap::Write(address),
                });
                i += 1;
            }
            "--ledger" => options.ledger = true,
            path => paths.push(path.to_string()),
        }
        i += 1;
    }
    (paths, options)
}

#[cfg(test)]
mod picture_tests {
    use super::*;

    #[test]
    fn a_ppm_is_a_header_and_three_bytes_a_pixel() {
        let mut canvas = Canvas::new(2, 1);
        canvas.pixels[1] = 0x00AB_CDEF;
        let file = write_ppm(&canvas);
        assert!(file.starts_with(b"P6\n2 1\n255\n"));
        assert_eq!(&file[11..], &[0, 0, 0, 0xAB, 0xCD, 0xEF]);
    }

    #[test]
    fn switches_come_out_of_the_paths() {
        let args: Vec<String> = [
            "game.nes", "--frames", "120", "--debug", "--line", "100", "--out", "shot.ppm",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (paths, options) = parse_args(&args);
        assert_eq!(paths, ["game.nes"]);
        assert_eq!(options.frames, 120);
        assert!(options.debug);
        assert_eq!(options.line, Some(100));
        assert_eq!(options.out.as_deref(), Some("shot.ppm"));
    }
}
