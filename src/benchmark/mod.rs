//! Repeated host-side measurements for the simulator's three subsystems.

pub mod fs_bench;
pub mod mm_bench;
pub mod sched_bench;

pub use fs_bench::{run_fs_benchmark, FsBenchResult};
pub use mm_bench::{run_mm_benchmark, MmBenchResult};
pub use sched_bench::{run_sched_benchmark, SchedBenchResult};

pub(crate) const WARMUP_ROUNDS: usize = 3;
pub(crate) const MEASURED_ROUNDS: usize = 11;

pub(crate) fn collect_samples<T>(mut run: impl FnMut() -> T) -> Vec<T> {
    for _ in 0..WARMUP_ROUNDS {
        std::hint::black_box(run());
    }
    (0..MEASURED_ROUNDS).map(|_| run()).collect()
}

pub(crate) fn median<T: Ord + Copy>(mut values: Vec<T>) -> T {
    assert!(!values.is_empty(), "a median requires samples");
    values.sort_unstable();
    values[values.len() / 2]
}

pub(crate) fn median_f64(mut values: Vec<f64>) -> f64 {
    assert!(!values.is_empty(), "a median requires samples");
    values.sort_unstable_by(f64::total_cmp);
    values[values.len() / 2]
}

pub fn run_all_benchmarks() {
    println!("\n========================================================");
    println!("        Mini-OS Kernel 核心子系统综合性能评测套件         ");
    println!("========================================================");
    println!("采样：{WARMUP_ROUNDS} 轮预热，{MEASURED_ROUNDS} 轮独立测量，报告中位数。");
    println!("计时范围不含初始化；指标反映本次宿主机运行和指定负载。");

    let mm = run_mm_benchmark();
    let sched = run_sched_benchmark();
    let fs = run_fs_benchmark();

    println!("\n========================================================");
    println!("                   综合性能测试结果                      ");
    println!("========================================================");
    println!("1. 内存分配与回收：");
    println!("   - Buddy 有效吞吐中位数: {:.0} ops/sec", mm.buddy_ops_per_sec);
    println!("   - Slab 有效吞吐中位数:  {:.0} ops/sec", mm.slab_ops_per_sec);
    println!("   - 小对象模拟物理页节约率: {:.2}%（不含宿主机元数据）", mm.memory_saved_ratio);
    println!("2. 进程调度：");
    println!("   - 该固定三任务负载的 Jain 公平性指数: {:.4}", sched.jains_fairness_index);
    println!("   - 每轮任务选择次数: {}", sched.task_selections);
    println!("   - 调度推进延迟中位数: {:.2} ns/step", sched.avg_schedule_step_latency_ns);
    println!("   - 模拟 CPU 上下文加载延迟中位数: {:.2} ns/load", sched.avg_cpu_load_latency_ns);
    println!("   - 调度推进吞吐中位数: {:.0} steps/sec", sched.sched_ops_per_sec);
    println!("3. 块设备与块缓存：");
    println!("   - 缓存命中率: {:.2}%", fs.cache_hit_rate);
    println!("   - 宿主机耗时中位数: 直接访问 {}µs，带缓存 {}µs", fs.uncached_time_us, fs.cached_time_us);
    println!("   - 宿主机缓存/直接耗时比: {:.2}x（越小越快）", fs.host_cached_to_direct_ratio);
    println!("   - 固定 I/O 周期模型收益: {:.2}x，减少 {} 周期", fs.simulated_io_speedup, fs.io_cycles_saved);
    println!("========================================================\n");
}
