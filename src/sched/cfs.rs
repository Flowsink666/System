//! 完全公平调度器 (Completely Fair Scheduler - CFS)
//! 
//! 高性能特性：
//! 1. 基于 B-Tree 就绪树结构 (BTreeSet) 实现 O(log N) 插入/删除与 pop_first() 单趟提取最小 vruntime 任务
//! 2. 严格按 Linux 40 级 Nice 权重分配 CPU 时间配额
//! 3. 动态时间片计算 (Dynamic Time Slice) 与最小粒度保护 (Min Granularity)
//! 4. 维护全局单调递增 min_vruntime，防止新任务或休眠唤醒任务长期霸占 CPU
//! 5. 严格维护就绪队列总权重 total_weight，防止权重泄露

use std::collections::{BTreeSet, HashMap};

pub struct CfsScheduler {
    /// 就绪平衡树队列：元素为 (vruntime, pid)
    runqueue: BTreeSet<(u64, usize)>,
    /// 任务入队元数据索引: pid -> (normalized_vruntime, weight)
    enqueued_tasks: HashMap<usize, (u64, u64)>,
    /// 全局基准虚拟运行时间，单调递增
    pub min_vruntime: u64,
    /// 运行队列当前活跃总权重
    pub total_weight: u64,
    /// 调度延迟周期 (目标延迟纳秒数，例如 20ms = 20_000_000 ns 或 20 个 tick)
    pub target_latency_ns: u64,
    /// 最小调度粒度纳秒数，防止过度频繁发生上下文切换
    pub min_granularity_ns: u64,
    /// 单个时钟周期的纳秒换算
    pub tick_ns: u64,
    /// 上下文切换计数器
    pub context_switches: u64,
}

impl CfsScheduler {
    pub fn new(target_latency_ticks: u64, min_granularity_ticks: u64, tick_ns: u64) -> Self {
        Self {
            runqueue: BTreeSet::new(),
            enqueued_tasks: HashMap::new(),
            min_vruntime: 0,
            total_weight: 0,
            target_latency_ns: target_latency_ticks * tick_ns,
            min_granularity_ns: min_granularity_ticks * tick_ns,
            tick_ns,
            context_switches: 0,
        }
    }

    /// 将就绪进程入队
    pub fn enqueue_task(&mut self, pid: usize, vruntime: u64, weight: u64) {
        // 如果该任务已在就绪队列中，先移除旧权重
        self.dequeue_task_by_pid(pid);

        // 防止休眠或新唤醒的任务 vruntime 过小
        let normalized_vruntime = vruntime.max(self.min_vruntime);
        self.runqueue.insert((normalized_vruntime, pid));
        self.enqueued_tasks.insert(pid, (normalized_vruntime, weight));
        self.total_weight += weight;
        self.update_min_vruntime();
    }

    /// 根据 pid 移除就绪任务
    pub fn dequeue_task_by_pid(&mut self, pid: usize) -> bool {
        if let Some((vruntime, weight)) = self.enqueued_tasks.remove(&pid) {
            self.runqueue.remove(&(vruntime, pid));
            self.total_weight = self.total_weight.saturating_sub(weight);
            self.update_min_vruntime();
            true
        } else {
            false
        }
    }

    /// 兼容旧接口：从就绪队列中移除进程
    pub fn dequeue_task(&mut self, pid: usize, _vruntime: u64, _weight: u64) -> bool {
        self.dequeue_task_by_pid(pid)
    }

    /// 极速挑选下一个最应获得 CPU 的进程：单次遍历弹出 BTree 最左最小节点
    pub fn pick_next_task(&mut self) -> Option<usize> {
        if let Some((_, pid)) = self.runqueue.pop_first() {
            if let Some((_, weight)) = self.enqueued_tasks.remove(&pid) {
                self.total_weight = self.total_weight.saturating_sub(weight);
            }
            self.context_switches += 1;
            Some(pid)
        } else {
            None
        }
    }

    /// 计算任务的动态时间片 (返回 ticks)
    /// 注意：由于任务已出队，总活跃负载必须包含当前任务自身的权重，保证权重分母的精准性
    pub fn calculate_timeslice_ticks(&self, task_weight: u64) -> u64 {
        let min_ticks = (self.min_granularity_ns / self.tick_ns).max(1);
        let active_weight = self.total_weight + task_weight;
        if active_weight == 0 {
            return min_ticks;
        }
        let slice_ns = (self.target_latency_ns * task_weight) / active_weight;
        let slice_ticks = slice_ns / self.tick_ns;
        slice_ticks.max(min_ticks)
    }

    /// 兼容接口：计算时间片
    pub fn calculate_timeslice(&self, task_weight: u64) -> u64 {
        self.calculate_timeslice_ticks(task_weight)
    }

    /// 检查是否有就绪任务的虚拟运行时间落后当前任务超过 min_granularity_ns，若超过则触发抢占
    pub fn check_preempt(&self, current_vruntime: u64) -> bool {
        if let Some(&(leftmost_vruntime, _)) = self.runqueue.iter().next()
            && leftmost_vruntime + self.min_granularity_ns < current_vruntime {
                return true;
            }
        false
    }

    /// 更新单调基准 min_vruntime
    fn update_min_vruntime(&mut self) {
        if let Some(&(leftmost_vruntime, _)) = self.runqueue.iter().next() {
            self.min_vruntime = self.min_vruntime.max(leftmost_vruntime);
        }
    }

    /// 就绪队列任务数
    pub fn runnable_count(&self) -> usize {
        self.runqueue.len()
    }

    /// 计算 Jain's Fairness Index 公平性指数: (Σ x_i)^2 / (n * Σ x_i^2)
    /// 数值越接近 1.0 (100%)，表明调度公平性越高
    pub fn compute_jains_fairness(runtimes_with_weights: &[(u64, u64)]) -> f64 {
        if runtimes_with_weights.is_empty() {
            return 1.0;
        }

        // 计算按权重归一化的服务量: normalized_service = runtime / weight
        let normalized: Vec<f64> = runtimes_with_weights
            .iter()
            .map(|&(rt, weight)| {
                if weight == 0 {
                    0.0
                } else {
                    rt as f64 / weight as f64
                }
            })
            .collect();

        let sum: f64 = normalized.iter().sum();
        let sum_sq: f64 = normalized.iter().map(|&x| x * x).sum();
        let n = normalized.len() as f64;

        if sum_sq == 0.0 {
            1.0
        } else {
            (sum * sum) / (n * sum_sq)
        }
    }
}
