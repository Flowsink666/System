//! 原生界面的有界文本、RAM 文件与协作式内核任务；不依赖 std、浏览器或宿主系统。
use core::fmt;

#[derive(Clone, Copy)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Text<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
    pub fn push(&mut self, byte: u8) -> bool {
        if self.len == N || !byte.is_ascii() {
            return false;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        true
    }
    pub fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }
}
impl<const N: usize> fmt::Write for Text<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !text.is_ascii() || text.len() > N - self.len {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}

pub struct RamFs {
    pub note: Text<1024>,
}
impl Default for RamFs {
    fn default() -> Self {
        Self::new()
    }
}
impl RamFs {
    pub const fn new() -> Self {
        Self { note: Text::new() }
    }
    pub fn read(&self, name: &str) -> Option<&str> {
        match name {
            "readme.txt" => Some(
                "MINI-OS NATIVE KERNEL\nBooted directly from a UEFI disk.\nFirmware boot services have been exited.\nF1 Overview  F2 Memory  F3 Tasks\nF4 Terminal  F5 Files\nTry: help, mem, alloc, free, spawn, tasks, ls\nwrite TEXT stores note.txt in kernel RAM.\n",
            ),
            "note.txt" => Some(self.note.as_str()),
            _ => None,
        }
    }
    pub fn write_note(&mut self, text: &str) -> bool {
        use fmt::Write;
        let mut replacement = Text::new();
        if replacement.write_str(text).is_err() {
            return false;
        }
        self.note = replacement;
        true
    }
}

#[derive(Clone, Copy)]
pub struct Task {
    pub id: usize,
    pub runs: u64,
    pub value: u64,
}
pub struct Tasks {
    pub items: [Task; 8],
    pub count: usize,
    next: usize,
    next_id: usize,
}
impl Default for Tasks {
    fn default() -> Self {
        Self::new()
    }
}
impl Tasks {
    pub const fn new() -> Self {
        Self {
            items: [Task {
                id: 0,
                runs: 0,
                value: 1,
            }; 8],
            count: 0,
            next: 0,
            next_id: 1,
        }
    }
    pub fn spawn(&mut self) -> bool {
        if self.count == self.items.len() {
            return false;
        }
        self.items[self.count] = Task {
            id: self.next_id,
            runs: 0,
            value: 1,
        };
        self.count += 1;
        self.next_id += 1;
        true
    }
    pub fn kill_last(&mut self) -> bool {
        if self.count == 0 {
            return false;
        }
        self.count -= 1;
        self.next = 0;
        true
    }
    pub fn step(&mut self) {
        if self.count == 0 {
            return;
        }
        self.next %= self.count;
        let task = &mut self.items[self.next];
        task.runs += 1;
        task.value = task.value.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.next = (self.next + 1) % self.count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ram_files_reject_overflow_without_destroying_old_content() {
        let mut fs = RamFs::new();
        assert!(fs.write_note("hello"));
        assert!(!fs.write_note(&"x".repeat(1025)));
        assert_eq!(fs.read("note.txt"), Some("hello"));
        assert_eq!(fs.read("absent"), None);
    }
    #[test]
    fn cooperative_tasks_execute_evenly_and_stop_when_removed() {
        let mut tasks = Tasks::new();
        tasks.spawn();
        tasks.spawn();
        for _ in 0..10 {
            tasks.step();
        }
        assert_eq!(tasks.items[0].runs, 5);
        assert_eq!(tasks.items[1].runs, 5);
        assert!(tasks.kill_last());
        tasks.step();
        assert_eq!(tasks.items[0].runs, 6);
    }
}
