//! 进程间通信与同步原语 (IPC & Synchronization Primitives)
//!
//! 提供管道 (Pipes)、信号量 (Semaphores) 与消息队列

use std::collections::VecDeque;

/// 环形缓冲区管道 (Pipe)
pub struct Pipe {
    pub buffer: VecDeque<u8>,
    pub capacity: usize,
    pub readers_count: usize,
    pub writers_count: usize,
    pub read_open: bool,
    pub write_open: bool,
}

impl Pipe {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            readers_count: 1,
            writers_count: 1,
            read_open: true,
            write_open: true,
        }
    }

    /// 写入数据
    pub fn write(&mut self, data: &[u8]) -> Result<usize, &'static str> {
        if self.readers_count == 0 || !self.read_open {
            return Err("Broken pipe: reader closed");
        }
        let available = self.capacity.saturating_sub(self.buffer.len());
        let to_write = data.len().min(available);
        self.buffer.extend(data[..to_write].iter().copied());
        Ok(to_write)
    }

    /// 预览待读取的数据，用户缓冲区写入成功前不消费管道内容。
    pub fn peek(&self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.buffer.len());
        for (slot, byte) in buf.iter_mut().zip(self.buffer.iter()).take(to_read) {
            *slot = *byte;
        }
        to_read
    }

    /// 提交已经完成的读取。
    pub fn consume(&mut self, count: usize) -> usize {
        let to_read = count.min(self.buffer.len());
        drop(self.buffer.drain(..to_read));
        to_read
    }

    /// 读取数据
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = self.peek(buf);
        self.consume(to_read)
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_eof(&self) -> bool {
        self.buffer.is_empty() && (self.writers_count == 0 || !self.write_open)
    }

    pub fn is_broken(&self) -> bool {
        self.readers_count == 0 || !self.read_open
    }

    pub fn available_write_space(&self) -> usize {
        self.capacity.saturating_sub(self.buffer.len())
    }

    pub fn close_reader(&mut self) -> bool {
        self.readers_count = self.readers_count.saturating_sub(1);
        if self.readers_count == 0 {
            self.read_open = false;
        }
        self.readers_count == 0
    }

    pub fn close_writer(&mut self) -> bool {
        self.writers_count = self.writers_count.saturating_sub(1);
        if self.writers_count == 0 {
            self.write_open = false;
        }
        self.writers_count == 0
    }

    pub fn is_dead(&self) -> bool {
        self.readers_count == 0 && self.writers_count == 0
    }
}

/// 计数信号量 (Counting Semaphore)
pub struct Semaphore {
    count: isize,
    wait_queue: VecDeque<usize>, // 等待信号量的 PID 队列
}

impl Semaphore {
    pub fn new(initial_count: isize) -> Self {
        Self {
            count: initial_count,
            wait_queue: VecDeque::new(),
        }
    }

    /// 尝试获取信号量 (P 操作)
    pub fn wait(&mut self, pid: usize) -> bool {
        self.count -= 1;
        if self.count < 0 {
            self.wait_queue.push_back(pid);
            false // 需要阻塞
        } else {
            true // 成功获取
        }
    }

    /// 释放信号量 (V 操作)
    pub fn post(&mut self) -> Option<usize> {
        self.count += 1;
        if self.count <= 0 {
            self.wait_queue.pop_front() // 唤醒等待者
        } else {
            None
        }
    }
}
