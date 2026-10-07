//! 虚拟定时器与时钟中断机制

use std::collections::BinaryHeap;
use std::cmp::Ordering;

#[derive(Debug, Clone, Eq, PartialEq)]
struct SleepEvent {
    wake_tick: u64,
    pid: usize,
}

impl Ord for SleepEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // 小顶堆：最早到期的排在前面
        other.wake_tick.cmp(&self.wake_tick)
    }
}

impl PartialOrd for SleepEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct VirtualTimer {
    pub current_tick: u64,
    pub tick_interval_ns: u64,
    sleep_queue: BinaryHeap<SleepEvent>,
}

impl VirtualTimer {
    pub fn new(tick_interval_ns: u64) -> Self {
        Self {
            current_tick: 0,
            tick_interval_ns,
            sleep_queue: BinaryHeap::new(),
        }
    }

    /// 触发一个时钟周期（时钟中断发生）
    pub fn tick(&mut self) -> Vec<usize> {
        self.current_tick += 1;
        let mut awakened_pids = Vec::new();

        while let Some(event) = self.sleep_queue.peek() {
            if event.wake_tick <= self.current_tick {
                awakened_pids.push(event.pid);
                self.sleep_queue.pop();
            } else {
                break;
            }
        }

        awakened_pids
    }

    /// 注册一个进程在指定 tick 唤醒
    pub fn add_sleep(&mut self, pid: usize, ticks_to_sleep: u64) {
        let wake_tick = self.current_tick + ticks_to_sleep;
        self.sleep_queue.push(SleepEvent { wake_tick, pid });
    }

    /// 当前系统时间戳 (纳秒)
    pub fn now_ns(&self) -> u64 {
        self.current_tick * self.tick_interval_ns
    }
}
