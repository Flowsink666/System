use crate::serial::trace;
use core::fmt::Write;
use mini_os_native::{graphics::Framebuffer, input::Key};
use mini_os_native::{
    memory::FrameAllocator,
    model::{RamFs, Tasks, Text},
};

const TITLES: [&str; 5] = ["OVERVIEW", "MEMORY", "TASKS", "TERMINAL", "FILES"];
pub struct Desktop {
    pub frames: FrameAllocator,
    pub view: usize,
    pub paused: bool,
    tasks: Tasks,
    files: RamFs,
    line: Text<60>,
    history: [Text<60>; 16],
    history_count: usize,
    pub message: Text<80>,
    file: usize,
}
impl Desktop {
    pub fn new(frames: FrameAllocator) -> Self {
        let mut desktop = Self {
            frames,
            view: 0,
            paused: false,
            tasks: Tasks::new(),
            files: RamFs::new(),
            line: Text::new(),
            history: [Text::new(); 16],
            history_count: 0,
            message: Text::new(),
            file: 0,
        };
        desktop.tasks.spawn();
        desktop.tasks.spawn();
        desktop.status("Native kernel ready. F1-F5 switch applications.");
        desktop.history("Mini-OS terminal. Type help and press Enter.");
        desktop
    }
    fn status(&mut self, text: &str) {
        self.message.clear();
        let _ = write!(self.message, "{text}");
    }
    fn history(&mut self, text: &str) {
        if self.history_count == 16 {
            self.history.copy_within(1..16, 0);
            self.history_count -= 1;
        }
        let entry = &mut self.history[self.history_count];
        entry.clear();
        let _ = entry.write_str(&text[..text.len().min(60)]);
        self.history_count += 1;
    }
    fn allocate(&mut self) {
        if let Some(frame) = self.frames.allocate() {
            // 内存来自固件 ConventionalMemory，仍使用引导期建立的恒等映射。
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, 4096);
            }
            self.message.clear();
            let _ = write!(self.message, "Allocated physical frame 0x{frame:x}");
            trace!("ALLOC: physical frame 0x{frame:x}\n");
        } else {
            self.status("No available frames or allocation limit reached.");
        }
    }
    fn free(&mut self) {
        if let Some(frame) = self.frames.last_frame() {
            if self.frames.free(frame) {
                self.status("Physical frame released.");
                trace!("FREE: physical frame 0x{frame:x}\n");
            }
        } else {
            self.status("No allocated frames to release.");
        }
    }
    fn spawn(&mut self) {
        if self.tasks.spawn() {
            self.status("Cooperative kernel task created.");
            trace!("TASK: spawned count={}\n", self.tasks.count);
        } else {
            self.status("Task table full (8 slots).");
        }
    }
    pub fn tick(&mut self) {
        if !self.paused {
            self.tasks.step();
        }
    }
    pub fn key(&mut self, key: Key) {
        match key {
            Key::View(view) => self.view = view,
            Key::Escape => self.view = 0,
            Key::Tab => self.view = (self.view + 1) % 5,
            Key::Enter if self.view == 3 => self.command(),
            Key::Backspace if self.view == 3 => self.line.pop(),
            Key::Character(byte) if self.view == 3 => {
                self.line.push(byte);
            }
            Key::Character(byte) => match byte.to_ascii_lowercase() {
                b'1'..=b'5' => self.view = (byte - b'1') as usize,
                b'a' => self.allocate(),
                b'f' => self.free(),
                b'n' => self.spawn(),
                b'k' => {
                    self.tasks.kill_last();
                    self.status("Last kernel task removed.");
                }
                b' ' => self.paused = !self.paused,
                _ => {}
            },
            _ => {}
        }
        trace!(
            "UI: view={} allocated={} tasks={}\n",
            self.view,
            self.frames.allocated_pages(),
            self.tasks.count
        );
    }
    pub fn click(&mut self, x: usize, y: usize) {
        if x < 180 && (112..342).contains(&y) {
            self.view = (y - 112) / 46;
        }
        if (260..306).contains(&y) {
            if self.view == 1 && (220..410).contains(&x) {
                self.allocate();
            } else if self.view == 1 && (430..620).contains(&x) {
                self.free();
            } else if self.view == 2 && (220..410).contains(&x) {
                self.spawn();
            } else if self.view == 2 && (430..620).contains(&x) {
                self.tasks.kill_last();
            } else if self.view == 4 {
                self.file = usize::from(x >= 430);
            }
        }
        trace!("MOUSE: click x={x} y={y} view={}\n", self.view);
    }
    fn command(&mut self) {
        let input = self.line;
        let command = input.as_str().trim();
        let mut echo = Text::<64>::new();
        let _ = write!(echo, "> {command}");
        self.history(echo.as_str());
        trace!("COMMAND: {command}\n");
        match command {
            "help" => {
                self.history("help mem alloc free tasks spawn kill ls cat write clear");
                self.history("cat readme.txt | cat note.txt | write YOUR TEXT");
            }
            "mem" => {
                let mut text = Text::<60>::new();
                let _ = write!(
                    text,
                    "{} free pages / {} total; {} allocated",
                    self.frames.free_pages(),
                    self.frames.total_pages,
                    self.frames.allocated_pages()
                );
                self.history(text.as_str());
            }
            "alloc" => {
                self.allocate();
                let message = self.message;
                self.history(message.as_str());
            }
            "free" => {
                self.free();
                let message = self.message;
                self.history(message.as_str());
            }
            "spawn" => {
                self.spawn();
                let message = self.message;
                self.history(message.as_str());
            }
            "kill" => {
                self.tasks.kill_last();
                self.history("Last task removed.");
            }
            "tasks" => {
                let mut text = Text::<60>::new();
                let _ = write!(text, "{} cooperative kernel tasks", self.tasks.count);
                self.history(text.as_str());
            }
            "ls" => self.history("/ram/readme.txt   /ram/note.txt"),
            "clear" => self.history_count = 0,
            other if other.starts_with("write ") => {
                if self.files.write_note(&other[6..]) {
                    self.history("Saved /ram/note.txt");
                    trace!("RAMFS: note saved\n");
                }
            }
            other if other.starts_with("cat ") => {
                let mut copy = Text::<1024>::new();
                if let Some(content) = self.files.read(other[4..].trim()) {
                    let _ = copy.write_str(content);
                    for line in copy.as_str().lines() {
                        self.history(line);
                    }
                } else {
                    self.history("File not found.");
                }
            }
            "" => {}
            _ => self.history("Unknown command. Type help."),
        }
        self.line.clear();
    }
    pub fn draw(&self, fb: &mut Framebuffer, ticks: u64) {
        let w = fb.width;
        let h = fb.height;
        let left = 210;
        let body = w.saturating_sub(left + 26);
        fb.rect(0, 0, w, h, 0xf0f3f8);
        fb.rect(0, 0, 180, h, 0x17283f);
        fb.text(22, 30, "MINI OS", 0xe2f1ff, 3);
        fb.text(24, 66, "NATIVE KERNEL", 0x7d99b8, 1);
        for (index, title) in TITLES.iter().enumerate() {
            let y = 112 + index * 46;
            if self.view == index {
                fb.rect(12, y, 156, 36, 0x2f557e);
            }
            let mut label = Text::<24>::new();
            let _ = write!(label, "F{} {title}", index + 1);
            fb.text(
                24,
                y + 11,
                label.as_str(),
                if self.view == index {
                    0xffffff
                } else {
                    0x8fa8c2
                },
                2,
            );
        }
        fb.text(20, h - 56, "BARE METAL", 0x71bd9b, 1);
        fb.text(20, h - 35, "X86_64 / RING 0", 0x7d99b8, 1);
        fb.rect(180, 0, w - 180, 48, 0xffffff);
        fb.text(left, 18, "MINI-OS / KERNEL DESKTOP", 0x52667b, 1);
        fb.text(left, 76, TITLES[self.view], 0x223b57, 3);
        fb.text(
            left,
            112,
            "DIRECT FRAMEBUFFER - NO HOST OS OR BROWSER",
            0x7b8ea1,
            1,
        );
        let card_width = (body - 24) / 3;
        for index in 0..3 {
            fb.rect(
                left + index * (card_width + 12),
                150,
                card_width,
                82,
                0xffffff,
            );
        }
        fb.text(left + 15, 163, "HARDWARE TICKS", 0x7d90a3, 1);
        let mut text = Text::<60>::new();
        let _ = write!(text, "{ticks}");
        fb.text(left + 15, 185, text.as_str(), 0x315780, 3);
        fb.text(
            left + card_width + 27,
            163,
            "FREE PHYSICAL RAM",
            0x7d90a3,
            1,
        );
        text.clear();
        let _ = write!(text, "{} MB", self.frames.free_pages() * 4 / 1024);
        fb.text(left + card_width + 27, 185, text.as_str(), 0x315780, 3);
        fb.text(
            left + 2 * (card_width + 12) + 15,
            163,
            "KERNEL TASKS",
            0x7d90a3,
            1,
        );
        text.clear();
        let _ = write!(text, "{}", self.tasks.count);
        fb.text(
            left + 2 * (card_width + 12) + 15,
            185,
            text.as_str(),
            0x315780,
            3,
        );
        fb.rect(left, 248, body, h - 316, 0xffffff);
        match self.view {
            0 => {
                fb.text(
                    left + 20,
                    274,
                    "YOUR OPERATING SYSTEM IS RUNNING.",
                    0x315780,
                    2,
                );
                fb.text(left + 20, 318, "UEFI BOOT SERVICES: EXITED", 0x338368, 2);
                fb.text(left + 20, 355, "8259 PIC + 100 HZ PIT: ACTIVE", 0x52667b, 2);
                fb.text(
                    left + 20,
                    390,
                    "PS/2 KEYBOARD AND MOUSE: DIRECT INPUT",
                    0x52667b,
                    2,
                );
                fb.text(
                    left + 20,
                    425,
                    "PHYSICAL FRAME ALLOCATOR: READY",
                    0x52667b,
                    2,
                );
                fb.text(
                    left + 20,
                    478,
                    "F1-F5 OR CLICK SIDEBAR TO OPEN AN APP.",
                    0x8292a2,
                    1,
                );
                fb.text(
                    left + 20,
                    504,
                    "KERNEL TASKS ARE COOPERATIVE; USER MODE IS NEXT.",
                    0x8292a2,
                    1,
                );
            }
            1 => {
                Self::button(fb, 220, 260, "A  ALLOCATE PAGE");
                Self::button(fb, 430, 260, "F  RELEASE PAGE");
                text.clear();
                let _ = write!(text, "MANAGED: {} PAGES", self.frames.total_pages);
                fb.text(left + 20, 340, text.as_str(), 0x52667b, 2);
                text.clear();
                let _ = write!(text, "ALLOCATED: {} PAGES", self.frames.allocated_pages());
                fb.text(left + 20, 385, text.as_str(), 0x52667b, 2);
                if let Some(frame) = self.frames.last_frame() {
                    text.clear();
                    let _ = write!(text, "LAST FRAME: 0X{frame:X}");
                    fb.text(left + 20, 430, text.as_str(), 0x315780, 2);
                }
                fb.text(
                    left + 20,
                    490,
                    "PAGES COME FROM UEFI CONVENTIONAL MEMORY.",
                    0x8292a2,
                    1,
                );
                fb.text(
                    left + 20,
                    515,
                    "RESERVED AND FIRMWARE PAGES ARE NOT REUSED.",
                    0x8292a2,
                    1,
                );
            }
            2 => {
                Self::button(fb, 220, 260, "N  NEW TASK");
                Self::button(fb, 430, 260, "K  REMOVE LAST");
                fb.text(left + 20, 328, "ID      EXECUTIONS       TYPE", 0x8292a2, 1);
                for (row, task) in self.tasks.items[..self.tasks.count]
                    .iter()
                    .take(h.saturating_sub(450) / 28)
                    .enumerate()
                {
                    text.clear();
                    let _ = write!(
                        text,
                        "{:02}      {:08}      KERNEL WORKER",
                        task.id, task.runs
                    );
                    fb.text(left + 20, 358 + row * 28, text.as_str(), 0x52667b, 2);
                }
                fb.text(
                    left + 20,
                    h - 103,
                    if self.paused {
                        "PAUSED - SPACE TO RESUME"
                    } else {
                        "SPACE TO PAUSE COOPERATIVE TASKS"
                    },
                    0x8292a2,
                    1,
                );
            }
            3 => {
                fb.rect(left + 12, 260, body - 24, h - 350, 0x1b2c42);
                let visible = ((h - 405) / 23).min(16);
                let begin = self.history_count.saturating_sub(visible);
                for (row, line) in self.history[begin..self.history_count].iter().enumerate() {
                    fb.text(left + 24, 276 + row * 23, line.as_str(), 0xbad6c8, 2);
                }
                text.clear();
                let _ = write!(text, "> {}_", self.line.as_str());
                fb.text(left + 24, h - 132, text.as_str(), 0xe7f4fa, 2);
                fb.text(
                    left + 24,
                    h - 105,
                    "TYPE A COMMAND. ENTER RUNS IT. F1 RETURNS HOME.",
                    0x8292a2,
                    1,
                );
            }
            4 => {
                Self::button(fb, 220, 260, "README.TXT");
                Self::button(fb, 430, 260, "NOTE.TXT");
                let name = if self.file == 0 {
                    "readme.txt"
                } else {
                    "note.txt"
                };
                let content = self.files.read(name).unwrap_or("");
                for (row, line) in content.lines().take(12).enumerate() {
                    fb.text(left + 20, 335 + row * 24, line, 0x52667b, 2);
                }
                fb.text(
                    left + 20,
                    h - 103,
                    "RAM FILES: USE F4 AND WRITE TEXT TO EDIT NOTE.TXT",
                    0x8292a2,
                    1,
                );
            }
            _ => {}
        }
        fb.text(left, h - 44, self.message.as_str(), 0x6c8298, 1);
    }
    fn button(fb: &mut Framebuffer, x: usize, y: usize, text: &str) {
        fb.rect(x, y, 190, 40, 0xe8eff8);
        fb.text(x + 12, y + 13, text, 0x315780, 1);
    }
}
