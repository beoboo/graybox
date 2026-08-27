//! Stopping the machine, and looking at it while it stands still.

use crate::canvas::Canvas;
use crate::cpu::Cpu;
use crate::font::Font;

/// How far to move the machine before it stops again.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Step {
    /// One instruction — the smallest move this CPU can make.
    Instruction,
    /// Until the beam is on the next scanline.
    Scanline,
    /// Until the picture is complete.
    Frame,
}

/// What the machine does on the next trip round the window loop.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// A whole frame per trip, as if nobody were watching.
    Running,
    /// Nothing at all: the machine stands still and the panels read it.
    Paused,
    /// One step, then a pause.
    Step(Step),
}

/// The debugger: a mode, and whether its panels are on screen.
pub struct Debugger {
    pub mode: Mode,
    pub open: bool,
}

/// Text in the panels: light on dark.
pub const INK: u32 = 0x00E0_E0E0;

/// Labels and the key help, dimmer, so the numbers stand out.
pub const DIM: u32 = 0x0080_8080;

/// The panels' background: dark, but not the black the game's own
/// picture might use, so the two never blend.
pub const PANEL: u32 = 0x0018_1820;

impl Debugger {
    /// Closed and running: a machine that does not know it is watched.
    pub fn new() -> Debugger {
        Debugger {
            mode: Mode::Running,
            open: false,
        }
    }

    /// Open the panels and stop the machine — or close them and let it go.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.mode = if self.open { Mode::Paused } else { Mode::Running };
    }

    /// Ask for one step. A running machine ignores the request: the step
    /// keys mean something only while the panels are open.
    pub fn step(&mut self, step: Step) {
        if self.open {
            self.mode = Mode::Step(step);
        }
    }

    /// Move the machine as far as the mode says. A step ends in a pause;
    /// a jammed machine stops wherever it is.
    pub fn run(&mut self, cpu: &mut Cpu) {
        match self.mode {
            Mode::Running => run_frame(cpu),
            Mode::Paused => {}
            Mode::Step(Step::Instruction) => {
                cpu.step();
            }
            Mode::Step(Step::Scanline) => run_scanline(cpu),
            Mode::Step(Step::Frame) => run_frame(cpu),
        }

        if let Mode::Step(_) = self.mode {
            self.mode = Mode::Paused;
        }
    }

    /// The registers panel, its top-left corner at (x, y): the CPU's
    /// pockets, the flags spelled out over their bits, where the beam is,
    /// and the keys.
    pub fn paint_registers(&self, canvas: &mut Canvas, font: &Font, cpu: &Cpu, x: usize, y: usize) {
        // Ten pixels a line: eight of glyph, two of air.
        let mut line = |row: usize, text: &str, color: u32| {
            canvas.text(font, x, y + row * 10, text, color);
        };

        line(0, &format!("A:{:02X} X:{:02X} Y:{:02X} SP:{:02X}", cpu.a, cpu.x, cpu.y, cpu.sp), INK);
        line(1, &format!("PC:{:04X}", cpu.pc), INK);
        line(2, "NV-BDIZC", DIM);
        line(3, &format!("{:08b}", cpu.status), INK);

        let clock = &cpu.bus.clock;
        line(5, &format!("frame {}", clock.frame), INK);
        line(6, &format!("line {}  dot {}", clock.scanline, clock.dot), INK);

        line(8, "Tab run/stop  N step", DIM);
        line(9, "L scanline    F frame", DIM);
    }

}


/// Run until the clock's frame counter moves — the loop `main` used to
/// own. A jammed machine keeps its last picture.
fn run_frame(cpu: &mut Cpu) {
    let frame = cpu.bus.clock.frame;
    while cpu.bus.clock.frame == frame {
        if !cpu.step() {
            break;
        }
    }
}

/// Run until the beam is on a different scanline. The stop lands on an
/// instruction boundary, never on the line's first dot: this CPU finishes
/// what it started, and the panel shows exactly how far past the line it
/// got.
fn run_scanline(cpu: &mut Cpu) {
    let scanline = cpu.bus.clock.scanline;
    while cpu.bus.clock.scanline == scanline {
        if !cpu.step() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;

    /// A machine whose only program is a jump to itself: three cycles,
    /// forever, with the picture chip ticking along underneath.
    fn idling_machine() -> Cpu {
        let mut prg = vec![0xEA; 16 * 1024];
        prg[0..3].copy_from_slice(&[0x4C, 0x00, 0x80]); // JMP $8000
        prg[0x3FFC] = 0x00; // the reset vector: $8000
        prg[0x3FFD] = 0x80;

        let mut rom = vec![b'N', b'E', b'S', 0x1A, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        rom.extend(prg);

        let mut cpu = Cpu::new(Cartridge::load(&rom).unwrap());
        cpu.reset();
        cpu
    }

    #[test]
    fn a_closed_debugger_runs_and_ignores_the_step_keys() {
        let mut debugger = Debugger::new();
        debugger.step(Step::Instruction);
        assert_eq!(debugger.mode, Mode::Running);

        let mut cpu = idling_machine();
        debugger.run(&mut cpu);
        assert_eq!(cpu.bus.clock.frame, 1, "running means a frame per trip");
    }

    #[test]
    fn opening_the_debugger_stops_the_machine() {
        let mut debugger = Debugger::new();
        debugger.toggle();
        assert_eq!(debugger.mode, Mode::Paused);

        let mut cpu = idling_machine();
        let before = cpu.cycles;
        debugger.run(&mut cpu);
        assert_eq!(cpu.cycles, before, "paused means not a single cycle");
    }

    #[test]
    fn an_instruction_step_is_one_instruction_then_a_pause() {
        let mut debugger = Debugger::new();
        debugger.toggle();
        debugger.step(Step::Instruction);

        let mut cpu = idling_machine();
        let before = cpu.cycles;
        debugger.run(&mut cpu);
        assert_eq!(cpu.cycles - before, 3, "JMP costs three cycles");
        assert_eq!(debugger.mode, Mode::Paused);
    }

    #[test]
    fn a_scanline_step_lands_on_the_next_line() {
        let mut debugger = Debugger::new();
        debugger.toggle();
        debugger.step(Step::Scanline);

        let mut cpu = idling_machine();
        let line = cpu.bus.clock.scanline;
        debugger.run(&mut cpu);
        assert_eq!(cpu.bus.clock.scanline, line + 1);
        assert!(cpu.bus.clock.dot < 9, "at most one instruction past the line's start");
    }

    #[test]
    fn a_frame_step_finishes_the_picture() {
        let mut debugger = Debugger::new();
        debugger.toggle();
        debugger.step(Step::Frame);

        let mut cpu = idling_machine();
        debugger.run(&mut cpu);
        assert_eq!(cpu.bus.clock.frame, 1);
        assert_eq!(debugger.mode, Mode::Paused);
    }
}