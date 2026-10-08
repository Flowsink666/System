//! Scheduler-step throughput and separate simulated CPU context-load timing.

use super::{collect_samples, median, median_f64};
use crate::arch::{CpuContext, PrivilegeLevel, VirtualCpu};
use crate::sched::{CfsScheduler, ProcessManager};
use std::hint::black_box;
use std::time::{Duration, Instant};

pub struct SchedBenchResult {
    pub jains_fairness_index: f64,
    pub task_selections: u64,
    pub total_schedule_time_us: u128,
    pub avg_schedule_step_latency_ns: f64,
    pub cpu_context_loads: u64,
    pub avg_cpu_load_latency_ns: f64,
    pub sched_ops_per_sec: f64,
}

const TASK_COUNT: usize = 100;
const SCHEDULE_STEPS: usize = 5_000;
const CPU_LOADS: u64 = 100_000;

struct ScheduleSample {
    duration: Duration,
    task_selections: u64,
}

fn schedule_sample() -> ScheduleSample {
    let mut pm = ProcessManager::new(1000);
    for index in 0..TASK_COUNT {
        pm.spawn(&format!("task_{index:03}"), (index % 5) as i8 - 2, 10_000);
    }
    let start = Instant::now();
    for _ in 0..SCHEDULE_STEPS {
        black_box(pm.schedule_step(black_box(1)));
    }
    let duration = start.elapsed();
    assert_eq!(pm.processes.values().map(|proc| proc.exec_time).sum::<u64>(), SCHEDULE_STEPS as u64 * 1000);
    // The CFS counter increments on queue extraction, even if the same PID wins.
    let task_selections = pm.scheduler.context_switches;
    black_box(&pm);
    ScheduleSample { duration, task_selections }
}

fn cpu_load_sample() -> Duration {
    let contexts: Vec<_> = (0..16).map(|index| {
        let mut context = CpuContext::new(0x1000 + index * 16, 0x8000 + index * 32);
        context.rax = index;
        context.rbx = index * 3;
        context
    }).collect();
    let mut cpu = VirtualCpu::new();
    let start = Instant::now();
    for index in 0..CPU_LOADS {
        let context = &contexts[index as usize % contexts.len()];
        black_box(&mut cpu).switch_to(black_box(context), PrivilegeLevel::Ring3User);
    }
    let duration = start.elapsed();
    assert_eq!(cpu.context_switches, CPU_LOADS);
    assert_eq!(cpu.cycles, CPU_LOADS * 42);
    assert_eq!(cpu.context.rip, contexts[(CPU_LOADS as usize - 1) % contexts.len()].rip);
    black_box(cpu.context);
    duration
}

pub fn run_sched_benchmark() -> SchedBenchResult {
    println!("\n========== [2/3] 进程调度子系统性能基准测试 ==========");
    let mut fairness_pm = ProcessManager::new(1000);
    let high = fairness_pm.spawn("high_prio_worker", -10, 2000);
    let mid = fairness_pm.spawn("normal_worker", 0, 2000);
    let low = fairness_pm.spawn("low_prio_worker", 10, 2000);
    for _ in 0..1000 {
        black_box(fairness_pm.schedule_step(1));
    }
    let fairness_data: Vec<_> = [high, mid, low].iter().map(|pid| {
        let proc = &fairness_pm.processes[pid];
        (proc.exec_time, proc.weight)
    }).collect();
    let jains_fairness_index = CfsScheduler::compute_jains_fairness(&fairness_data);
    println!("  [固定三任务公平性验证：1000 个 tick]");
    for (pid, nice) in [(high, -10), (mid, 0), (low, 10)] {
        let proc = &fairness_pm.processes[&pid];
        println!("    - Nice {nice:>3}, 权重 {}, 累计运行 {} ns", proc.weight, proc.exec_time);
    }
    println!("    - 权重归一化 Jain 指数: {jains_fairness_index:.4}（只代表本负载）。");

    let schedule = collect_samples(schedule_sample);
    let duration = median(schedule.iter().map(|sample| sample.duration).collect());
    let task_selections = median(schedule.iter().map(|sample| sample.task_selections).collect());
    let avg_schedule_step_latency_ns = duration.as_nanos() as f64 / SCHEDULE_STEPS as f64;
    let sched_ops_per_sec = median_f64(schedule.iter().map(|sample| SCHEDULE_STEPS as f64 / sample.duration.as_secs_f64()).collect());
    let cpu_duration = median(collect_samples(cpu_load_sample));
    let avg_cpu_load_latency_ns = cpu_duration.as_nanos() as f64 / CPU_LOADS as f64;

    println!("  [调度推进：每轮 {TASK_COUNT} 个任务，{SCHEDULE_STEPS} 次 step]");
    println!("    - 每轮队列任务选择: {task_selections} 次；这项计数来自 CFS 队列提取。");
    println!("    - 总耗时中位数: {duration:.2?}");
    println!("    - 每次调度推进中位数: {avg_schedule_step_latency_ns:.2} ns/step（含进程记账与队列维护）");
    println!("    - 推进吞吐中位数: {sched_ops_per_sec:.0} steps/sec");
    println!("  [独立模拟 CPU 上下文加载：每轮 {CPU_LOADS} 次 VirtualCpu::switch_to]");
    println!("    - 每次加载中位数: {avg_cpu_load_latency_ns:.2} ns/load（复制模拟寄存器、更新模式与计数）");

    SchedBenchResult {
        jains_fairness_index,
        task_selections,
        total_schedule_time_us: duration.as_micros(),
        avg_schedule_step_latency_ns,
        cpu_context_loads: CPU_LOADS,
        avg_cpu_load_latency_ns,
        sched_ops_per_sec,
    }
}
