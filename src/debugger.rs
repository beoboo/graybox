//! Stopping the machine, and looking at it while it stands still.

use crate::bus::Touch;
use crate::canvas::Canvas;
use crate::cpu::Cpu;
use crate::disasm::disassemble;
use crate::font::Font;
use minifb::Key;

/// How many instructions the debugger remembers, and how many of them
/// and of the ones ahead the listing shows: thirteen lines, which is what
/// fits under the registers in a sidebar as tall as the picture.
const HISTORY: usize = 16;
const HISTORY_SHOWN: usize = 4;
const AHEAD_SHOWN: usize = 8;

/// How many touches the ledger keeps: the twelve lines that fit under
/// its heading.
const LEDGER: usize = 12;

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

/// A question asked around every instruction. The machine stops in
/// front of an address it is about to execute, after an instruction
/// that touched a watched byte, or at the first instruction boundary
/// on or past a scanline.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Trap {
    Execute(u16),
    Read(u16),
    Write(u16),
    Line(u16),
}

/// Which panel sits under the registers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Listing,
    Ledger,
}

/// A trap being typed: its letter, then the digits so far.
#[derive(Clone, PartialEq, Debug)]
pub struct Prompt {
    pub letter: char,
    pub digits: String,
}

/// One line of the ledger: the instruction that touched the watched
/// byte, the frame it ran in, and the bus's note.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Entry {
    pub pc: u16,
    pub frame: u64,
    pub touch: Touch,
}

/// The debugger: a mode, and whether its panels are on screen.
pub struct Debugger {
    pub mode: Mode,
    pub open: bool,
    /// Where the last few instructions ran from, oldest first. Bytes
    /// behind the program counter cannot be read as a program — only
    /// remembered as one.
    pub history: Vec<u16>,
    /// Addresses the machine stops in front of.
    pub breakpoints: Vec<u16>,
    /// The byte being watched, and whether a read or a write of it
    /// stops the machine.
    pub watch: Option<Trap>,
    /// The scanline to stop on, once.
    pub line_trap: Option<u16>,
    /// Every touch of the watched byte, oldest first.
    pub ledger: Vec<Entry>,
    /// Why the machine last stopped, or what was just armed.
    pub notice: String,
    pub page: Page,
    pub prompt: Option<Prompt>,
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
            history: Vec::new(),
            breakpoints: Vec::new(),
            watch: None,
            line_trap: None,
            ledger: Vec::new(),
            notice: String::new(),
            page: Page::Listing,
            prompt: None,
        }
    }

    /// Open the panels and stop the machine — or close them and let it go.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.mode = if self.open {
            Mode::Paused
        } else {
            Mode::Running
        };
    }

    /// Ask for one step. A running machine ignores the request: the step
    /// keys mean something only while the panels are open.
    pub fn step(&mut self, step: Step) {
        if self.open {
            self.mode = Mode::Step(step);
        }
    }

    /// Note where the CPU is about to run from, keeping the last few.
    fn remember(&mut self, cpu: &Cpu) {
        if self.history.len() == HISTORY {
            self.history.remove(0);
        }
        self.history.push(cpu.pc);
    }

    /// Move the machine as far as the mode says, or as far as a trap
    /// lets it. A step ends in a pause; a sprung trap ends anything in
    /// one; a jammed machine stops wherever it is.
    pub fn run(&mut self, cpu: &mut Cpu) {
        cpu.bus.watch = self.watched();
        match self.mode {
            Mode::Running | Mode::Step(Step::Frame) => self.run_frame(cpu),
            Mode::Paused => {}
            Mode::Step(Step::Instruction) => {
                self.one_instruction(cpu);
            }
            Mode::Step(Step::Scanline) => self.run_scanline(cpu),
        }

        if let Mode::Step(_) = self.mode {
            self.mode = Mode::Paused;
        }
    }

    /// One instruction, with the traps' questions around it. Returns
    /// false when the machine must stop here: a trap sprang, or the CPU
    /// jammed.
    fn one_instruction(&mut self, cpu: &mut Cpu) -> bool {
        self.remember(cpu);
        let from = cpu.pc;
        let line_before = cpu.bus.clock.scanline;
        cpu.bus.touched.set(None);
        let alive = cpu.step();

        if let Some(touch) = cpu.bus.touched.get() {
            self.record(from, cpu.bus.clock.frame, touch);
            let sprung = match self.watch {
                Some(Trap::Read(_)) => touch.read,
                Some(Trap::Write(_)) => touch.written,
                _ => false,
            };
            if sprung {
                let what = if touch.written { "written" } else { "read" };
                self.stop(format!(
                    "{:04X} {what} by {from:04X}",
                    cpu.bus.watch.unwrap_or(0)
                ));
                return false;
            }
        }
        if self.breakpoints.contains(&cpu.pc) {
            self.stop(format!("break at {:04X}", cpu.pc));
            return false;
        }
        if let Some(line) = self.line_trap {
            if reached(line_before, cpu.bus.clock.scanline, line) {
                self.line_trap = None;
                self.stop(format!("line {line} reached"));
                return false;
            }
        }
        alive
    }

    /// Run until the clock's frame counter moves, or a trap springs.
    fn run_frame(&mut self, cpu: &mut Cpu) {
        let frame = cpu.bus.clock.frame;
        while cpu.bus.clock.frame == frame && self.one_instruction(cpu) {}
    }

    /// Run until the beam is on a different scanline, or a trap springs.
    /// The stop lands on an instruction boundary, never on the line's
    /// first dot: this CPU finishes what it started, and the panel
    /// shows exactly how far past the line it got.
    fn run_scanline(&mut self, cpu: &mut Cpu) {
        let scanline = cpu.bus.clock.scanline;
        while cpu.bus.clock.scanline == scanline && self.one_instruction(cpu) {}
    }

    /// Stop the machine and say why. A trap that springs while the
    /// panels are closed opens them: that is what it was set for.
    fn stop(&mut self, why: String) {
        self.mode = Mode::Paused;
        self.open = true;
        self.notice = why;
    }

    /// The watched byte's address, if there is one.
    fn watched(&self) -> Option<u16> {
        match self.watch {
            Some(Trap::Read(address)) | Some(Trap::Write(address)) => Some(address),
            _ => None,
        }
    }

    /// Add a line to the ledger, forgetting the oldest past its size.
    fn record(&mut self, pc: u16, frame: u64, touch: Touch) {
        if self.ledger.len() == LEDGER {
            self.ledger.remove(0);
        }
        self.ledger.push(Entry { pc, frame, touch });
    }

    /// Set a trap. An address already on the execute list comes off it
    /// instead; a new watch starts a fresh ledger.
    pub fn arm(&mut self, trap: Trap) {
        match trap {
            Trap::Execute(address) => match self.breakpoints.iter().position(|&b| b == address) {
                Some(index) => {
                    self.breakpoints.remove(index);
                    self.notice = format!("B {address:04X} off");
                }
                None => {
                    self.breakpoints.push(address);
                    self.notice = format!("B {address:04X} set");
                }
            },
            Trap::Read(address) | Trap::Write(address) => {
                self.watch = Some(trap);
                self.ledger.clear();
                let letter = if trap == Trap::Read(address) {
                    'R'
                } else {
                    'W'
                };
                self.notice = format!("{letter} {address:04X} set");
            }
            Trap::Line(line) if line >= 262 => self.notice = format!("no line {line}"),
            Trap::Line(line) => {
                self.line_trap = Some(line);
                self.notice = format!("P {line} set");
            }
        }
    }

    /// Let the machine go with the panels still open: it runs until a
    /// trap springs.
    pub fn resume(&mut self) {
        self.mode = Mode::Running;
        self.notice = "running".to_string();
    }

    /// Take every trap off.
    pub fn disarm(&mut self) {
        self.breakpoints.clear();
        self.watch = None;
        self.line_trap = None;
        self.notice = "traps off".to_string();
    }

    /// A key, while the panels are open: a digit for the prompt if one
    /// is open, otherwise a step, a page, or the start of a trap.
    pub fn press(&mut self, key: Key) {
        if !self.open {
            return;
        }
        self.notice.clear();
        if let Some(prompt) = self.prompt.take() {
            self.prompt = self.typed(prompt, key);
            return;
        }
        match key {
            Key::N => self.step(Step::Instruction),
            Key::L => self.step(Step::Scanline),
            Key::F => self.step(Step::Frame),
            Key::C => self.resume(),
            Key::X => self.disarm(),
            Key::Key1 => self.page = Page::Listing,
            Key::Key2 => self.page = Page::Ledger,
            Key::B | Key::W | Key::R | Key::P => {
                let letter = format!("{key:?}").chars().next().unwrap_or('B');
                self.prompt = Some(Prompt {
                    letter,
                    digits: String::new(),
                });
            }
            _ => {}
        }
    }

    /// One key into an open prompt: a digit joins it, Backspace takes
    /// one away — or closes an empty prompt — and the last digit wanted
    /// arms the trap. Returns the prompt if it is still open.
    fn typed(&mut self, mut prompt: Prompt, key: Key) -> Option<Prompt> {
        if key == Key::Backspace {
            return prompt.digits.pop().map(|_| prompt);
        }
        if let Some(digit) = prompt.digit(key) {
            prompt.digits.push(digit);
        }
        if prompt.digits.len() < prompt.wanted() {
            return Some(prompt);
        }
        self.arm(prompt.trap());
        None
    }

    /// The registers panel, its top-left corner at (x, y): the CPU's
    /// pockets, the flags spelled out over their bits, and where the beam
    /// is.
    pub fn paint_registers(&self, canvas: &mut Canvas, font: &Font, cpu: &Cpu, x: usize, y: usize) {
        // Ten pixels a line: eight of glyph, two of air.
        let mut line = |row: usize, text: &str, color: u32| {
            canvas.text(font, x, y + row * 10, text, color);
        };

        line(
            0,
            &format!(
                "A:{:02X} X:{:02X} Y:{:02X} SP:{:02X}",
                cpu.a, cpu.x, cpu.y, cpu.sp
            ),
            INK,
        );
        line(1, &format!("PC:{:04X}", cpu.pc), INK);
        line(2, "NV-BDIZC", DIM);
        line(3, &format!("{:08b}", cpu.status), INK);

        let clock = &cpu.bus.clock;
        line(5, &format!("frame {}", clock.frame), INK);
        line(
            6,
            &format!("line {}  dot {}", clock.scanline, clock.dot),
            INK,
        );
    }

    /// The listing panel at (x, y): the last few instructions run, dim;
    /// the one about to run, marked; and the ones after it, decoded from
    /// the bytes ahead — which are only a program until a branch says
    /// otherwise.
    pub fn paint_listing(&self, canvas: &mut Canvas, font: &Font, cpu: &Cpu, x: usize, y: usize) {
        let mut row = 0;
        let mut line = |canvas: &mut Canvas, address: u16, marker: &str, color: u32| {
            let (text, length) = disassemble(cpu, address);
            let mut bytes = String::new();
            for offset in 0..length {
                match cpu.peek(address.wrapping_add(offset)) {
                    Some(byte) => bytes.push_str(&format!("{byte:02X} ")),
                    None => bytes.push_str("?? "),
                }
            }
            let entry = format!("{marker}{address:04X}  {bytes:<9} {text}");
            canvas.text(font, x, y + row * 10, &entry, color);
            row += 1;
            length
        };

        let behind = self.history.len().saturating_sub(HISTORY_SHOWN);
        for &address in &self.history[behind..] {
            line(canvas, address, " ", DIM);
        }

        let mut next = cpu.pc;
        next = next.wrapping_add(line(canvas, next, ">", INK));
        for _ in 0..AHEAD_SHOWN {
            let mark = if self.breakpoints.contains(&next) { "*" } else { " " };
            next = next.wrapping_add(line(canvas, next, mark, INK));
        }
    }
    /// The ledger panel at (x, y): every touch of the watched byte the
    /// debugger has seen, oldest first — which instruction, what the
    /// byte became, and where the beam was.
    pub fn paint_ledger(&self, canvas: &mut Canvas, font: &Font, x: usize, y: usize) {
        let address = match self.watched() {
            Some(address) => address,
            None => {
                canvas.text(font, x, y, "no byte watched: R or W", DIM);
                return;
            }
        };
        canvas.text(font, x, y, &format!("{address:04X} by      frame line.dot"), DIM);
        for (row, entry) in self.ledger.iter().enumerate() {
            let kind = if entry.touch.written { 'w' } else { 'r' };
            let text = format!(
                "{kind}{:02X}  {:04X} {:>8} {:>3}.{:<3}",
                entry.touch.value, entry.pc, entry.frame, entry.touch.scanline, entry.touch.dot
            );
            canvas.text(font, x, y + (row + 1) * 10, &text, INK);
        }
    }

    /// The status line at (x, y): the trap being typed, else why the
    /// machine last stopped, else the keys that set one.
    pub fn paint_status(&self, canvas: &mut Canvas, font: &Font, x: usize, y: usize) {
        match &self.prompt {
            Some(prompt) => canvas.text(font, x, y, &prompt.text(), INK),
            None if !self.notice.is_empty() => canvas.text(font, x, y, &self.notice, INK),
            None => canvas.text(font, x, y, "B W R P  C go  X clear  1 2", DIM),
        }
    }

    /// The keys, two dim lines wherever there is room for them.
    pub fn paint_keys(&self, canvas: &mut Canvas, font: &Font, x: usize, y: usize) {
        canvas.text(font, x, y, "Tab run/stop  N step", DIM);
        canvas.text(font, x, y + 10, "L scanline    F frame", DIM);
    }
}
impl Prompt {
    /// Digits the trap takes: four of hex for an address, three of
    /// decimal for a scanline.
    fn wanted(&self) -> usize {
        if self.letter == 'P' {
            3
        } else {
            4
        }
    }

    /// The digit a key types into this prompt, if it takes one.
    fn digit(&self, key: Key) -> Option<char> {
        let hex = self.letter != 'P';
        match key {
            Key::Key0 => Some('0'),
            Key::Key1 => Some('1'),
            Key::Key2 => Some('2'),
            Key::Key3 => Some('3'),
            Key::Key4 => Some('4'),
            Key::Key5 => Some('5'),
            Key::Key6 => Some('6'),
            Key::Key7 => Some('7'),
            Key::Key8 => Some('8'),
            Key::Key9 => Some('9'),
            Key::A if hex => Some('A'),
            Key::B if hex => Some('B'),
            Key::C if hex => Some('C'),
            Key::D if hex => Some('D'),
            Key::E if hex => Some('E'),
            Key::F if hex => Some('F'),
            _ => None,
        }
    }

    /// The trap the finished prompt spells.
    fn trap(&self) -> Trap {
        let address = u16::from_str_radix(&self.digits, 16).unwrap_or(0);
        match self.letter {
            'B' => Trap::Execute(address),
            'R' => Trap::Read(address),
            'W' => Trap::Write(address),
            _ => Trap::Line(self.digits.parse().unwrap_or(0)),
        }
    }

    /// The prompt as the status line shows it: `W 20_`.
    fn text(&self) -> String {
        format!("{} {}_", self.letter, self.digits)
    }
}

/// Whether the beam passed `line` going from one scanline to another —
/// across the end of the frame if it had to.
fn reached(before: u16, after: u16, line: u16) -> bool {
    if after >= before {
        before < line && line <= after
    } else {
        line > before || line <= after
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
        assert!(
            cpu.bus.clock.dot < 9,
            "at most one instruction past the line's start"
        );
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
    #[test]
    fn a_step_is_remembered() {
        let mut debugger = Debugger::new();
        debugger.toggle();
        debugger.step(Step::Instruction);

        let mut cpu = idling_machine();
        debugger.run(&mut cpu);
        assert_eq!(debugger.history, [0x8000]);
    }

    #[test]
    fn the_memory_is_sixteen_deep() {
        let mut debugger = Debugger::new();
        let mut cpu = idling_machine();
        debugger.run(&mut cpu);
        assert_eq!(debugger.history.len(), HISTORY);
        assert!(
            debugger.history.iter().all(|&pc| pc == 0x8000),
            "a jump to itself, forever"
        );
    }
        /// The idling machine with a different program at $8000.
    fn machine_with(program: &[u8]) -> Cpu {
        let mut cpu = idling_machine();
        cpu.bus.cartridge.prg_rom[..program.len()].copy_from_slice(program);
        cpu
    }

    /// An open debugger with the keys pressed in order.
    fn pressed(keys: &[Key]) -> Debugger {
        let mut debugger = Debugger::new();
        debugger.toggle();
        for &key in keys {
            debugger.press(key);
        }
        debugger
    }

    #[test]
    fn an_execute_trap_stops_in_front_of_its_address() {
        let mut cpu = machine_with(&[0xEA, 0x4C, 0x00, 0x80]); // NOP; JMP $8000
        let mut debugger = pressed(&[Key::B, Key::Key8, Key::Key0, Key::Key0, Key::Key1, Key::C]);
        assert_eq!(debugger.breakpoints, [0x8001]);
        assert_eq!(debugger.mode, Mode::Running);

        debugger.run(&mut cpu);
        assert_eq!(cpu.pc, 0x8001, "the NOP ran; the JMP has not");
        assert_eq!(debugger.mode, Mode::Paused);
        assert_eq!(debugger.notice, "break at 8001");
    }

    #[test]
    fn a_write_trap_springs_after_the_instruction_that_wrote() {
        // LDA #$05; STA $0200; JMP $8000
        let mut cpu = machine_with(&[0xA9, 0x05, 0x8D, 0x00, 0x02, 0x4C, 0x00, 0x80]);
        let mut debugger = pressed(&[Key::W, Key::Key0, Key::Key2, Key::Key0, Key::Key0, Key::C]);

        debugger.run(&mut cpu);
        assert_eq!(cpu.bus.ram[0x200], 5, "the write went through before the stop");
        assert_eq!(cpu.pc, 0x8005, "stopped after the STA");
        assert_eq!(debugger.notice, "0200 written by 8002");
        let entry = debugger.ledger[0];
        assert_eq!((entry.pc, entry.touch.value, entry.touch.written), (0x8002, 5, true));
    }

    #[test]
    fn a_read_trap_lets_writes_pass_but_the_ledger_keeps_them() {
        let mut cpu = machine_with(&[0xA9, 0x05, 0x8D, 0x00, 0x02, 0x4C, 0x00, 0x80]);
        let mut debugger = pressed(&[Key::R, Key::Key0, Key::Key2, Key::Key0, Key::Key0, Key::C]);

        debugger.run(&mut cpu);
        assert_eq!(debugger.mode, Mode::Running, "a whole frame, no stop");
        assert_eq!(debugger.ledger.len(), LEDGER);
        assert!(debugger.ledger.iter().all(|entry| entry.touch.written && !entry.touch.read));
    }

    #[test]
    fn a_read_modify_write_is_one_touch_that_did_both() {
        let mut cpu = machine_with(&[0xEE, 0x00, 0x02, 0x4C, 0x00, 0x80]); // INC $0200; JMP
        let mut debugger = pressed(&[Key::R, Key::Key0, Key::Key2, Key::Key0, Key::Key0, Key::C]);

        debugger.run(&mut cpu);
        let touch = debugger.ledger[0].touch;
        assert!(touch.read && touch.written);
        assert_eq!(touch.value, 1, "the value that was written, not the one read");
    }

    #[test]
    fn a_line_trap_lands_on_the_first_boundary_past_the_line() {
        let mut cpu = idling_machine();
        let mut debugger = pressed(&[Key::P, Key::Key1, Key::Key0, Key::Key0, Key::C]);

        debugger.run(&mut cpu);
        assert_eq!(cpu.bus.clock.scanline, 100);
        assert!(cpu.bus.clock.dot < 9, "at most one instruction past the line's start");
        assert_eq!(debugger.line_trap, None, "a line trap springs once");
    }

    #[test]
    fn a_trap_opens_a_closed_debugger() {
        let mut cpu = machine_with(&[0xEA, 0x4C, 0x00, 0x80]);
        let mut debugger = pressed(&[Key::B, Key::Key8, Key::Key0, Key::Key0, Key::Key1]);
        debugger.toggle();
        assert!(!debugger.open);

        debugger.run(&mut cpu);
        assert!(debugger.open);
        assert_eq!(debugger.mode, Mode::Paused);
    }

    #[test]
    fn the_prompt_takes_hex_and_backspace_and_an_address_twice_is_off() {
        let debugger = pressed(&[Key::B, Key::C, Key::Backspace, Key::C, Key::E, Key::Key1, Key::Key6]);
        assert_eq!(debugger.breakpoints, [0xCE16]);
        assert_eq!(debugger.prompt, None);

        let debugger = pressed(&[Key::B, Key::C, Key::E, Key::Key1, Key::Key6, Key::B, Key::C, Key::E, Key::Key1, Key::Key6]);
        assert_eq!(debugger.breakpoints, []);
        assert_eq!(debugger.notice, "B CE16 off");
    }

    #[test]
    fn a_line_prompt_takes_decimal_and_refuses_a_line_the_frame_has_not_got() {
        let debugger = pressed(&[Key::P, Key::A, Key::Key3, Key::Key0, Key::Key0]);
        assert_eq!(debugger.line_trap, None);
        assert_eq!(debugger.notice, "no line 300");
    }

}
