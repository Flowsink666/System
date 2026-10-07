//! 进程与调度子系统 (Process & Scheduling Subsystem)

pub mod cfs;
pub mod ipc;
pub mod pcb;

pub use cfs::CfsScheduler;
pub use ipc::{Pipe, Semaphore};
pub use pcb::{BlockedReason, FileDescriptorEntry, ProcessControlBlock, ProcessState};

use std::collections::HashMap;

/// 结构化调度事件（零堆分配开销，供热路径高效处理）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleEvent {
    ContextSwitch { old_pid: Option<usize>, new_pid: usize },
    Running { pid: usize, exec_ticks: u64, remaining_burst: u64, vruntime: u64 },
    Terminated { pid: usize },
    Idle,
}

impl ScheduleEvent {
    pub fn display(&self, processes: &HashMap<usize, ProcessControlBlock>) -> String {
        match *self {
            ScheduleEvent::ContextSwitch { old_pid, new_pid } => {
                let name = processes.get(&new_pid).map(|p| p.name.as_str()).unwrap_or("unknown");
                format!("[Switch] {:?} -> PID {} ({})", old_pid, new_pid, name)
            }
            ScheduleEvent::Running { pid, exec_ticks, remaining_burst, vruntime } => {
                let name = processes.get(&pid).map(|p| p.name.as_str()).unwrap_or("unknown");
                format!(
                    "PID {}: [{}] executed {} ticks, remaining burst={}, vruntime={}",
                    pid, name, exec_ticks, remaining_burst, vruntime
                )
            }
            ScheduleEvent::Terminated { pid } => format!("[Terminated] PID {}", pid),
            ScheduleEvent::Idle => "System Idle (No runnable processes)".to_string(),
        }
    }
}

pub struct ProcessManager {
    pub processes: HashMap<usize, ProcessControlBlock>,
    pub current_pid: Option<usize>,
    pub scheduler: CfsScheduler,
    pub pipes: HashMap<usize, Pipe>,
    pub semaphores: HashMap<usize, Semaphore>,
    pub tick_ns: u64,
    next_pid: usize,
    next_pipe_id: usize,
    next_sem_id: usize,
}

impl ProcessManager {
    pub fn new(tick_ns: u64) -> Self {
        Self {
            processes: HashMap::new(),
            current_pid: None,
            scheduler: CfsScheduler::new(20, 2, tick_ns), // 20 周期目标延迟，2 周期最小粒度
            pipes: HashMap::new(),
            semaphores: HashMap::new(),
            tick_ns,
            next_pid: 1,
            next_pipe_id: 1,
            next_sem_id: 1,
        }
    }

    /// 创建并就绪新进程
    pub fn spawn(&mut self, name: &str, nice: i8, burst: u64) -> usize {
        let pid = self.next_pid;
        self.next_pid += 1;

        let mut pcb = ProcessControlBlock::new(pid, 0, name, nice, burst);
        pcb.vruntime = self.scheduler.min_vruntime; // 规范化 vruntime
        let weight = pcb.weight;
        let vruntime = pcb.vruntime;

        self.processes.insert(pid, pcb);
        self.scheduler.enqueue_task(pid, vruntime, weight);

        pid
    }

    /// 复制父进程基础 PCB (Fork)
    pub fn fork_pcb(&mut self, parent_pid: usize) -> Result<usize, &'static str> {
        let parent = self.processes.get(&parent_pid).ok_or("Parent PID not found")?;
        let child_pid = self.next_pid;
        self.next_pid += 1;

        let mut child_pcb = ProcessControlBlock::new(
            child_pid,
            parent_pid,
            &format!("{}_fork", parent.name),
            parent.nice,
            (parent.cpu_burst_remaining / 2).max(1),
        );
        child_pcb.vruntime = parent.vruntime;
        child_pcb.fd_table = parent.fd_table.clone();
        child_pcb.vma_list = parent.vma_list.clone();
        child_pcb.context = parent.context;

        let weight = child_pcb.weight;
        let vruntime = child_pcb.vruntime;

        self.processes.insert(child_pid, child_pcb);
        self.scheduler.enqueue_task(child_pid, vruntime, weight);

        Ok(child_pid)
    }

    /// 执行调度步进 (Schedule Step)
    /// 返回 (当前运行PID, 结构化事件, 是否发生了上下文切换)
    pub fn schedule_step(&mut self, ticks: u64) -> (Option<usize>, ScheduleEvent, bool) {
        let mut context_switched = false;
        let old_pid_record = self.current_pid;

        // 1. 判断是否需要上下文切换
        let need_switch = if let Some(curr_pid) = self.current_pid {
            if let Some(curr_proc) = self.processes.get(&curr_pid) {
                curr_proc.state != ProcessState::Running
                    || curr_proc.time_slice_remaining == 0
                    || curr_proc.cpu_burst_remaining == 0
                    || self.scheduler.check_preempt(curr_proc.vruntime)
            } else {
                true
            }
        } else {
            true
        };

        if need_switch {
            // 将旧进程放回就绪队列（若仍可就绪）
            if let Some(old_pid) = self.current_pid {
                if let Some(old_proc) = self.processes.get_mut(&old_pid) {
                    if old_proc.state == ProcessState::Running {
                        if old_proc.cpu_burst_remaining == 0 {
                            old_proc.state = ProcessState::Terminated;
                        } else {
                            old_proc.state = ProcessState::Ready;
                            let vruntime = old_proc.vruntime;
                            let weight = old_proc.weight;
                            self.scheduler.enqueue_task(old_pid, vruntime, weight);
                        }
                    }
                }
            }

            // 从 CFS 挑选下一个进程
            if let Some(next_pid) = self.scheduler.pick_next_task() {
                if let Some(proc) = self.processes.get_mut(&next_pid) {
                    proc.state = ProcessState::Running;
                    proc.time_slice_remaining = self.scheduler.calculate_timeslice(proc.weight);
                    self.current_pid = Some(next_pid);
                    context_switched = old_pid_record != Some(next_pid);
                }
            } else {
                self.current_pid = None;
                return (None, ScheduleEvent::Idle, old_pid_record.is_some());
            }
        }

        // 2. 执行当前进程
        if let Some(pid) = self.current_pid {
            let proc = self.processes.get_mut(&pid).unwrap();
            let exec_ticks = ticks.min(proc.cpu_burst_remaining).min(proc.time_slice_remaining);
            proc.cpu_burst_remaining = proc.cpu_burst_remaining.saturating_sub(exec_ticks);
            proc.time_slice_remaining = proc.time_slice_remaining.saturating_sub(exec_ticks);
            proc.update_vruntime(exec_ticks * self.tick_ns);

            let remaining_burst = proc.cpu_burst_remaining;
            let vruntime = proc.vruntime;

            let event = if context_switched {
                ScheduleEvent::ContextSwitch {
                    old_pid: old_pid_record,
                    new_pid: pid,
                }
            } else if remaining_burst == 0 {
                proc.state = ProcessState::Terminated;
                self.current_pid = None;
                ScheduleEvent::Terminated { pid }
            } else {
                ScheduleEvent::Running {
                    pid,
                    exec_ticks,
                    remaining_burst,
                    vruntime,
                }
            };

            (Some(pid), event, context_switched)
        } else {
            (None, ScheduleEvent::Idle, false)
        }
    }

    /// 终止进程
    pub fn kill(&mut self, pid: usize) -> Result<(), &'static str> {
        let proc = self.processes.get_mut(&pid).ok_or("Process not found")?;
        let vruntime = proc.vruntime;
        let weight = proc.weight;
        proc.state = ProcessState::Terminated;
        self.scheduler.dequeue_task(pid, vruntime, weight);

        if self.current_pid == Some(pid) {
            self.current_pid = None;
        }
        Ok(())
    }

    /// 阻塞进程
    pub fn block(&mut self, pid: usize, reason: BlockedReason) {
        if let Some(proc) = self.processes.get_mut(&pid) {
            let vruntime = proc.vruntime;
            let weight = proc.weight;
            proc.state = ProcessState::Blocked(reason);
            self.scheduler.dequeue_task(pid, vruntime, weight);
            if self.current_pid == Some(pid) {
                self.current_pid = None;
            }
        }
    }

    /// 唤醒进程
    pub fn wake(&mut self, pid: usize) {
        if let Some(proc) = self.processes.get_mut(&pid) {
            if matches!(proc.state, ProcessState::Blocked(_)) {
                proc.state = ProcessState::Ready;
                // 防止长时间睡眠后唤醒产生过度抢占，将 vruntime 对齐当前 min_vruntime
                proc.vruntime = proc.vruntime.max(self.scheduler.min_vruntime);
                let vruntime = proc.vruntime;
                let weight = proc.weight;
                self.scheduler.enqueue_task(pid, vruntime, weight);
            }
        }
    }

    /// 等待子进程退出 (Waitpid)
    pub fn waitpid(&mut self, parent_pid: usize, target_child_pid: usize) -> Result<Option<i32>, &'static str> {
        let child = self.processes.get(&target_child_pid).ok_or("Child process not found")?;
        if child.ppid != parent_pid {
            return Err("Process is not a child of caller");
        }

        if child.state == ProcessState::Terminated || child.state == ProcessState::Zombie {
            let exit_code = child.exit_code;
            self.processes.remove(&target_child_pid);
            Ok(Some(exit_code))
        } else {
            self.block(parent_pid, BlockedReason::WaitingChild);
            Ok(None)
        }
    }

    /// 创建匿名管道
    pub fn create_pipe(&mut self, capacity: usize) -> usize {
        let id = self.next_pipe_id;
        self.next_pipe_id += 1;
        self.pipes.insert(id, Pipe::new(capacity));
        id
    }

    /// 创建计数信号量
    pub fn create_semaphore(&mut self, initial_count: isize) -> usize {
        let id = self.next_sem_id;
        self.next_sem_id += 1;
        self.semaphores.insert(id, Semaphore::new(initial_count));
        id
    }
}
