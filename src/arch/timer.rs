//! 虚拟定时器与时钟中断机制

use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SleepEvent {
    wake_tick: u64,
    pub pid: usize,
    pub token: u64,
}

impl Ord for SleepEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // 小顶堆：最早到期的排在前面
        (other.wake_tick, other.pid, other.token).cmp(&(self.wake_tick, self.pid, self.token))
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
    next_sleep_token: u64,
}

impl VirtualTimer {
    pub fn new(tick_interval_ns: u64) -> Self {
        Self {
            current_tick: 0,
            tick_interval_ns,
            sleep_queue: BinaryHeap::new(),
            next_sleep_token: 1,
        }
    }

    /// 触发一个时钟周期（时钟中断发生）
    pub fn tick(&mut self) -> Vec<SleepEvent> {
        self.current_tick = self.current_tick.saturating_add(1);
        let mut awakened = Vec::new();

        while let Some(event) = self.sleep_queue.peek() {
            if event.wake_tick <= self.current_tick {
                awakened.push(self.sleep_queue.pop().unwrap());
            } else {
                break;
            }
        }

        awakened
    }

    /// 注册一个进程在指定 tick 唤醒
    pub fn add_sleep(&mut self, pid: usize, ticks_to_sleep: u64) -> Result<u64, &'static str> {
        let wake_tick = self
            .current_tick
            .checked_add(ticks_to_sleep)
            .ok_or("Sleep deadline overflow")?;
        let token = self.next_sleep_token;
        let next_token = token.checked_add(1).ok_or("Sleep token overflow")?;
        self.cancel_sleep(pid);
        self.next_sleep_token = next_token;
        self.sleep_queue.push(SleepEvent {
            wake_tick,
            pid,
            token,
        });
        Ok(token)
    }

    pub fn cancel_sleep(&mut self, pid: usize) {
        self.sleep_queue.retain(|event| event.pid != pid);
    }

    /// 当前系统时间戳 (纳秒)
    pub fn now_ns(&self) -> u64 {
        self.current_tick.saturating_mul(self.tick_interval_ns)
    }
}
