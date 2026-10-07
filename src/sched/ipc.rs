//! 进程间通信与同步原语 (IPC & Synchronization Primitives)
//! 
//! 提供管道 (Pipes)、信号量 (Semaphores) 与消息队列

use std::collections::VecDeque;

/// 环形缓冲区管道 (Pipe)
pub struct Pipe {
    buffer: VecDeque<u8>,
    capacity: usize,
    pub read_open: bool,
    pub write_open: bool,
}

impl Pipe {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            read_open: true,
            write_open: true,
        }
    }

    /// 写入数据
    pub fn write(&mut self, data: &[u8]) -> Result<usize, &'static str> {
        if !self.read_open {
            return Err("Broken pipe: reader closed");
        }
        let available = self.capacity.saturating_sub(self.buffer.len());
        let to_write = data.len().min(available);
        for &byte in &data[..to_write] {
            self.buffer.push_back(byte);
        }
        Ok(to_write)
    }

    /// 读取数据
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.buffer.len());
        for slot in buf.iter_mut().take(to_read) {
            *slot = self.buffer.pop_front().unwrap();
        }
        to_read
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
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
