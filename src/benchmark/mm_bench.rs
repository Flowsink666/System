//! Allocation/free throughput, measured on fresh allocators in each sample.

use super::{collect_samples, median, median_f64};
use crate::mm::{BuddyAllocator, SlabAllocator, PAGE_SIZE};
use std::hint::black_box;
use std::time::{Duration, Instant};

pub struct MmBenchResult {
    pub buddy_alloc_time_us: u128,
    pub buddy_free_time_us: u128,
    pub buddy_ops_per_sec: f64,
    pub slab_alloc_time_us: u128,
    pub slab_free_time_us: u128,
    pub slab_ops_per_sec: f64,
    pub memory_saved_ratio: f64,
}

const TOTAL_PAGES: usize = 65_536;
const BUDDY_ITERATIONS: usize = 10_000;
const SLAB_ITERATIONS: usize = 5_000;
const OBJECT_SIZE: usize = 64;

struct AllocationSample {
    allocate: Duration,
    free: Duration,
    completed_ops: usize,
    peak_pages: usize,
}

impl AllocationSample {
    fn ops_per_sec(&self) -> f64 {
        self.completed_ops as f64 / (self.allocate + self.free).as_secs_f64()
    }
}

fn buddy_sample() -> AllocationSample {
    let mut buddy = BuddyAllocator::new(TOTAL_PAGES);
    let mut pfns = Vec::with_capacity(BUDDY_ITERATIONS);
    let start = Instant::now();
    for index in 0..BUDDY_ITERATIONS {
        pfns.push(black_box(buddy.allocate_pages(black_box(index % 4)).expect("buddy benchmark allocation failed")));
    }
    let allocate = start.elapsed();
    let peak_pages = buddy.stats.allocated_pages;
    let start = Instant::now();
    for pfn in pfns {
        buddy.free_pages(black_box(pfn)).expect("buddy benchmark free failed");
    }
    let free = start.elapsed();
    assert_eq!(buddy.stats.free_pages, TOTAL_PAGES, "buddy benchmark leaked pages");
    black_box(buddy.stats);
    AllocationSample { allocate, free, completed_ops: BUDDY_ITERATIONS * 2, peak_pages }
}

fn slab_sample() -> AllocationSample {
    let mut buddy = BuddyAllocator::new(TOTAL_PAGES);
    let mut slab = SlabAllocator::new();
    let mut handles = Vec::with_capacity(SLAB_ITERATIONS);
    let start = Instant::now();
    for _ in 0..SLAB_ITERATIONS {
        handles.push(black_box(slab.kmalloc(&mut buddy, black_box(OBJECT_SIZE)).expect("slab benchmark allocation failed")));
    }
    let allocate = start.elapsed();
    let peak_pages = buddy.stats.allocated_pages;
    let start = Instant::now();
    for handle in handles {
        slab.kfree(&mut buddy, black_box(handle)).expect("slab benchmark free failed");
    }
    let free = start.elapsed();
    assert!(slab.get_cache_stats().iter().all(|stats| stats.2 == 0), "slab benchmark left live objects");
    black_box(slab.get_cache_stats());
    AllocationSample { allocate, free, completed_ops: SLAB_ITERATIONS * 2, peak_pages }
}

pub fn run_mm_benchmark() -> MmBenchResult {
    println!("\n========== [1/3] 内存子系统性能基准测试 ==========");
    let buddy = collect_samples(buddy_sample);
    let slab = collect_samples(slab_sample);
    let buddy_allocate = median(buddy.iter().map(|sample| sample.allocate).collect());
    let buddy_free = median(buddy.iter().map(|sample| sample.free).collect());
    let buddy_ops_per_sec = median_f64(buddy.iter().map(AllocationSample::ops_per_sec).collect());
    let slab_allocate = median(slab.iter().map(|sample| sample.allocate).collect());
    let slab_free = median(slab.iter().map(|sample| sample.free).collect());
    let slab_ops_per_sec = median_f64(slab.iter().map(AllocationSample::ops_per_sec).collect());
    let slab_pages = median(slab.iter().map(|sample| sample.peak_pages).collect());
    let naive_bytes = SLAB_ITERATIONS * PAGE_SIZE;
    let slab_bytes = slab_pages * PAGE_SIZE;
    let memory_saved_ratio = (1.0 - slab_bytes as f64 / naive_bytes as f64) * 100.0;

    println!("  [Buddy Allocator]");
    println!("    - 每轮 {BUDDY_ITERATIONS} 次分配与 {BUDDY_ITERATIONS} 次回收，阶数 0..3；每轮校验页数恢复。");
    println!("    - 分配耗时中位数: {buddy_allocate:.2?}，回收耗时中位数: {buddy_free:.2?}");
    println!("    - 有效吞吐中位数: {buddy_ops_per_sec:.0} ops/sec");
    println!("  [Slab Object Cache]");
    println!("    - 每轮 {SLAB_ITERATIONS} 个 {OBJECT_SIZE}B 对象分配与回收；每轮校验无存活对象。");
    println!("    - 分配耗时中位数: {slab_allocate:.2?}，回收耗时中位数: {slab_free:.2?}");
    println!("    - 有效吞吐中位数: {slab_ops_per_sec:.0} ops/sec");
    println!("    - 按对象各占一页比较: {:.2}MB → {:.2}KB 模拟物理页，节约 {memory_saved_ratio:.2}%（不含宿主机元数据）。", naive_bytes as f64 / 1024.0 / 1024.0, slab_bytes as f64 / 1024.0);

    MmBenchResult {
        buddy_alloc_time_us: buddy_allocate.as_micros(),
        buddy_free_time_us: buddy_free.as_micros(),
        buddy_ops_per_sec,
        slab_alloc_time_us: slab_allocate.as_micros(),
        slab_free_time_us: slab_free.as_micros(),
        slab_ops_per_sec,
        memory_saved_ratio,
    }
}
