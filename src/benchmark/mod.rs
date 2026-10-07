//! 性能基准测试套件聚合入口 (Benchmark Suite)

pub mod fs_bench;
pub mod mm_bench;
pub mod sched_bench;

pub use fs_bench::{run_fs_benchmark, FsBenchResult};
pub use mm_bench::{run_mm_benchmark, MmBenchResult};
pub use sched_bench::{run_sched_benchmark, SchedBenchResult};

pub fn run_all_benchmarks() {
    println!("\n========================================================");
    println!("        Mini-OS Kernel 核心子系统综合性能评测套件         ");
    println!("========================================================");

    let mm_res = run_mm_benchmark();
    let sched_res = run_sched_benchmark();
    let fs_res = run_fs_benchmark();

    println!("\n========================================================");
    println!("                   综合性能测试总结报告                  ");
    println!("========================================================");
    println!("1. 内存管理 (Buddy + Slab O(1)):");
    println!("   - Buddy 有效吞吐: {:.0} ops/sec", mm_res.buddy_ops_per_sec);
    println!("   - Slab 吞吐:     {:.0} ops/sec", mm_res.slab_ops_per_sec);
    println!("   - 小对象物理内存节约率: {:.2}%", mm_res.memory_saved_ratio);
    println!("2. 进程调度 (CFS):");
    println!("   - Jain 公平性指数: {:.4} (理论上限 1.0)", sched_res.jains_fairness_index);
    println!("   - 单次调度决策判断延迟: {:.2} ns", sched_res.avg_decision_latency_ns);
    println!("   - 单次上下文切换平均延迟: {:.2} ns", sched_res.avg_switch_latency_ns);
    println!("   - 调度吞吐量: {:.0} decisions/sec", sched_res.sched_ops_per_sec);
    println!("3. 文件系统 (VFS + Buffer Cache):");
    println!("   - 高速缓冲命中率: {:.2}%", fs_res.cache_hit_rate);
    println!("   - 底层模拟 I/O 周期加速比: {:.2}x", fs_res.simulated_io_speedup);
    println!("   - 削减底层磁盘周期: {} 周期", fs_res.io_cycles_saved);
    println!("========================================================\n");
}
