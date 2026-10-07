//! 进程调度子系统性能基准测试 (CFS Scheduler Benchmarks)

use crate::sched::{CfsScheduler, ProcessManager};
use std::time::Instant;

pub struct SchedBenchResult {
    pub jains_fairness_index: f64,
    pub context_switches: u64,
    pub total_schedule_time_us: u128,
    pub avg_decision_latency_ns: f64,
    pub avg_switch_latency_ns: f64,
    pub sched_ops_per_sec: f64,
}

pub fn run_sched_benchmark() -> SchedBenchResult {
    println!("\n========== [2/3] 进程调度子系统性能基准测试 ==========");

    // 1. CFS 调度公平性验证 (Jain's Fairness Index)
    let mut pm = ProcessManager::new(1000);
    
    // 创建不同优先级 (Nice) 的进程任务
    // Nice -10 (高权重 9548)
    // Nice   0 (基准权重 1024)
    // Nice +10 (低权重 110)
    let p_high = pm.spawn("high_prio_worker", -10, 2000);
    let p_mid  = pm.spawn("normal_worker",     0, 2000);
    let p_low  = pm.spawn("low_prio_worker",   10, 2000);

    let test_ticks = 1_000;
    for _ in 0..test_ticks {
        let _ = pm.schedule_step(1);
    }

    let p_high_proc = pm.processes.get(&p_high).unwrap();
    let p_mid_proc  = pm.processes.get(&p_mid).unwrap();
    let p_low_proc  = pm.processes.get(&p_low).unwrap();

    let fairness_data = vec![
        (p_high_proc.exec_time, p_high_proc.weight),
        (p_mid_proc.exec_time, p_mid_proc.weight),
        (p_low_proc.exec_time, p_low_proc.weight),
    ];

    let jains_index = CfsScheduler::compute_jains_fairness(&fairness_data);

    println!("  [CFS 优先级权重与公平性验证 (Jain's Fairness)]");
    println!("    - 高优先级 (-10, 权重 9548): 累计运行 {} ns", p_high_proc.exec_time);
    println!("    - 中优先级 (  0, 权重 1024): 累计运行 {} ns", p_mid_proc.exec_time);
    println!("    - 低优先级 (+10, 权重  110): 累计运行 {} ns", p_low_proc.exec_time);
    println!("    - Jain 公平性指数: {:.4} (理论完全公平值为 1.0000, 实际表现达 {:.2}%)", 
        jains_index, jains_index * 100.0);

    // 2. 高并发多任务调度延迟与吞吐量测试 (100 个并发进程, 5000 次调度周期)
    let mut pm_stress = ProcessManager::new(1000);
    let task_count = 100;
    for i in 0..task_count {
        let nice = ((i % 5) as i8) - 2; // -2, -1, 0, 1, 2
        pm_stress.spawn(&format!("task_{:03}", i), nice, 10_000);
    }

    let sched_steps = 5_000;
    let start_sched = Instant::now();
    for _ in 0..sched_steps {
        let _ = pm_stress.schedule_step(1);
    }
    let sched_duration = start_sched.elapsed();

    let switches = pm_stress.scheduler.context_switches;
    let avg_decision_latency_ns = (sched_duration.as_nanos() as f64) / (sched_steps as f64);
    let avg_switch_latency_ns = (sched_duration.as_nanos() as f64) / (switches.max(1) as f64);
    let sched_ops_per_sec = (sched_steps as f64) / sched_duration.as_secs_f64();

    println!("\n  [CFS 高并发调度延迟与平衡树吞吐量]");
    println!("    - 并发任务数: {} 个活跃进程 (BTreeSet 纳秒级就绪树)", task_count);
    println!("    - 调度循环次数: {} 次推进, 发生真实切换: {} 次", sched_steps, switches);
    println!("    - 总测试耗时: {:.2?}", sched_duration);
    println!("    - 单次调度判断延迟: {:.2} ns/step", avg_decision_latency_ns);
    println!("    - 单次上下文切换平均耗时: {:.2} ns/switch", avg_switch_latency_ns);
    println!("    - 调度决策吞吐量: {:.0} decisions/sec", sched_ops_per_sec);

    SchedBenchResult {
        jains_fairness_index: jains_index,
        context_switches: switches,
        total_schedule_time_us: sched_duration.as_micros(),
        avg_decision_latency_ns,
        avg_switch_latency_ns,
        sched_ops_per_sec,
    }
}
