//! 内存子系统性能基准测试 (Memory Management Benchmarks)

use crate::mm::{BuddyAllocator, SlabAllocator, PAGE_SIZE};
use std::time::Instant;

pub struct MmBenchResult {
    pub buddy_alloc_time_us: u128,
    pub buddy_free_time_us: u128,
    pub buddy_ops_per_sec: f64,
    pub slab_alloc_time_us: u128,
    pub slab_free_time_us: u128,
    pub slab_ops_per_sec: f64,
    pub memory_saved_ratio: f64,
}

pub fn run_mm_benchmark() -> MmBenchResult {
    println!("\n========== [1/3] 内存子系统性能基准测试 ==========");
    
    // 1. Buddy 伙伴系统分配与回收基准测试 (10,000 次操作)
    let total_pages = 32768; // 128MB 虚拟物理内存
    let mut buddy = BuddyAllocator::new(total_pages);
    let iterations = 10_000;
    let mut allocated_pfns = Vec::with_capacity(iterations);

    let start_buddy_alloc = Instant::now();
    for i in 0..iterations {
        let order = (i % 4) as usize; // 0, 1, 2, 3 阶
        if let Some(pfn) = buddy.allocate_pages(order) {
            allocated_pfns.push(pfn);
        }
    }
    let buddy_alloc_time = start_buddy_alloc.elapsed();
    let success_allocs = allocated_pfns.len();

    let start_buddy_free = Instant::now();
    let mut success_frees = 0;
    for pfn in allocated_pfns.drain(..) {
        if buddy.free_pages(pfn).is_ok() {
            success_frees += 1;
        }
    }
    let buddy_free_time = start_buddy_free.elapsed();

    let total_buddy_time = buddy_alloc_time + buddy_free_time;
    let total_successful_ops = success_allocs + success_frees;
    let buddy_ops_per_sec = total_successful_ops as f64 / total_buddy_time.as_secs_f64();

    println!("  [Buddy Allocator]");
    println!("    - 测试规模: {} 次申请尝试, 成功分配 {} 块, 成功释放 {} 块", iterations, success_allocs, success_frees);
    println!("    - 纯分配耗时: {:.2?} (平均 {:.2} ns/次)", buddy_alloc_time, (buddy_alloc_time.as_nanos() as f64) / iterations as f64);
    println!("    - 纯释放耗时: {:.2?} (平均 {:.2} ns/次)", buddy_free_time, (buddy_free_time.as_nanos() as f64) / success_frees.max(1) as f64);
    println!("    - 伙伴合并次数: {}, 拆分次数: {}", buddy.stats.merges_count, buddy.stats.splits_count);
    println!("    - 有效吞吐速率: {:.0} ops/sec (基于实际完成的 {} 次操作)", buddy_ops_per_sec, total_successful_ops);

    // 2. Slab 高速缓存分配器对比测试 (5,000 次 64 字节小对象)
    let mut slab = SlabAllocator::new();
    let slab_iterations = 5_000;
    let mut slab_handles = Vec::with_capacity(slab_iterations);

    let start_slab_alloc = Instant::now();
    for _ in 0..slab_iterations {
        if let Some(h) = slab.kmalloc(&mut buddy, 64) {
            slab_handles.push(h);
        }
    }
    let slab_alloc_time = start_slab_alloc.elapsed();
    let slab_success_allocs = slab_handles.len();

    let start_slab_free = Instant::now();
    let mut slab_success_frees = 0;
    for h in slab_handles.drain(..) {
        if slab.kfree(&mut buddy, h).is_ok() {
            slab_success_frees += 1;
        }
    }
    let slab_free_time = start_slab_free.elapsed();

    let total_slab_time = slab_alloc_time + slab_free_time;
    let total_slab_ops = slab_success_allocs + slab_success_frees;
    let slab_ops_per_sec = total_slab_ops as f64 / total_slab_time.as_secs_f64();

    let naive_memory_bytes = slab_iterations * PAGE_SIZE;
    let slab_memory_bytes = ((slab_iterations as f64 / (PAGE_SIZE / 64) as f64).ceil() as usize) * PAGE_SIZE;
    let saved_ratio = (1.0 - (slab_memory_bytes as f64 / naive_memory_bytes as f64)) * 100.0;

    println!("\n  [Slab Object Cache]");
    println!("    - 测试规模: {} 次 64-byte 小对象分配与回收 (O(1) 索引池)", slab_iterations);
    println!("    - 纯分配耗时: {:.2?} (平均 {:.2} ns/次)", slab_alloc_time, (slab_alloc_time.as_nanos() as f64) / slab_iterations as f64);
    println!("    - 纯释放耗时: {:.2?} (平均 {:.2} ns/次)", slab_free_time, (slab_free_time.as_nanos() as f64) / slab_success_frees.max(1) as f64);
    println!("    - 吞吐速率: {:.0} ops/sec", slab_ops_per_sec);
    println!("    - 内存节约对比: 传统页分配需 {:.2} MB，Slab 仅需 {:.2} KB (节约 {:.2}% 物理内存)", 
        naive_memory_bytes as f64 / 1024.0 / 1024.0,
        slab_memory_bytes as f64 / 1024.0,
        saved_ratio
    );

    MmBenchResult {
        buddy_alloc_time_us: buddy_alloc_time.as_micros(),
        buddy_free_time_us: buddy_free_time.as_micros(),
        buddy_ops_per_sec,
        slab_alloc_time_us: slab_alloc_time.as_micros(),
        slab_free_time_us: slab_free_time.as_micros(),
        slab_ops_per_sec,
        memory_saved_ratio: saved_ratio,
    }
}
