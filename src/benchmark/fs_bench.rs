//! 文件系统与缓冲缓存性能基准测试 (VFS & Buffer Cache Benchmarks)

use crate::fs::{BufferCache, VirtualBlockDevice, BLOCK_SIZE};
use std::time::Instant;

pub struct FsBenchResult {
    pub cached_time_us: u128,
    pub uncached_time_us: u128,
    pub cache_hit_rate: f64,
    pub simulated_io_speedup: f64,
    pub io_cycles_saved: u64,
}

pub fn run_fs_benchmark() -> FsBenchResult {
    println!("\n========== [3/3] 文件系统与块缓冲缓存性能基准测试 ==========");

    let total_blocks = 4096; // 2MB 虚拟块设备
    let cache_capacity = 128; // 128 块的高速缓存 (64KB)
    let io_operations = 4_000;

    // 访问模式模拟：80% 访问集中在 20% 的热点数据块 (局部性原理 80/20 规律)
    let hot_blocks = 64;

    // 1. 无缓存 (Direct Disk I/O)
    let mut dev_uncached = VirtualBlockDevice::new(total_blocks);
    let mut dummy_buf = [0x55u8; BLOCK_SIZE];

    let start_uncached = Instant::now();
    for i in 0..io_operations {
        let block_idx = if i % 10 < 8 {
            i % hot_blocks
        } else {
            (i * 7) % total_blocks
        };

        if i % 2 == 0 {
            let _ = dev_uncached.read_block(block_idx, &mut dummy_buf);
        } else {
            let _ = dev_uncached.write_block(block_idx, &dummy_buf);
        }
    }
    let uncached_duration = start_uncached.elapsed();
    let uncached_cycles = dev_uncached.stats.simulated_io_cycles;

    // 2. 有高速缓冲 (Buffer Cache with LRU & Write-Back)
    let mut dev_cached = VirtualBlockDevice::new(total_blocks);
    let mut cache = BufferCache::new(cache_capacity);

    let start_cached = Instant::now();
    for i in 0..io_operations {
        let block_idx = if i % 10 < 8 {
            i % hot_blocks
        } else {
            (i * 7) % total_blocks
        };

        if i % 2 == 0 {
            let _ = cache.read_block(&mut dev_cached, block_idx, &mut dummy_buf);
        } else {
            let _ = cache.write_block(&mut dev_cached, block_idx, &dummy_buf);
        }
    }
    // 刷回脏数据
    let _ = cache.sync(&mut dev_cached);
    let cached_duration = start_cached.elapsed();
    let cached_cycles = dev_cached.stats.simulated_io_cycles;

    let hit_rate = cache.hit_rate();
    let cycles_saved = uncached_cycles.saturating_sub(cached_cycles);
    let simulated_io_speedup = uncached_cycles as f64 / cached_cycles.max(1) as f64;

    println!("  [Page/Buffer Cache 读写加速比与延迟评测]");
    println!("    - 测试规模: {} 次随机块读写 (符合 80/20 热点局部性)", io_operations);
    println!("    - 无缓存 (Direct I/O): 磁盘底层操作 {} 次, 模拟 I/O 周期: {}", 
        dev_uncached.stats.read_ops + dev_uncached.stats.write_ops, uncached_cycles);
    println!("    - 带缓存 (Buffer Cache): 命中 {} 次, 未命中 {} 次, 脏块回写 {} 次", 
        cache.hits, cache.misses, cache.writebacks);
    println!("    - 缓存命中率: {:.2}%", hit_rate);
    println!("    - 削减模拟磁盘 I/O 周期: {} 周期 (削减 {:.2}%)", 
        cycles_saved, (cycles_saved as f64 / uncached_cycles as f64) * 100.0);
    println!("    - 底层模拟 I/O 加速比: {:.2}x 周期收益", simulated_io_speedup);
    println!("    - 宿主机物理执行耗时: 直接内存访问 {:.2?} vs 带缓存 {:.2?} (含 LRU 簿记开销)", 
        uncached_duration, cached_duration);

    FsBenchResult {
        cached_time_us: cached_duration.as_micros(),
        uncached_time_us: uncached_duration.as_micros(),
        cache_hit_rate: hit_rate,
        simulated_io_speedup,
        io_cycles_saved: cycles_saved,
    }
}
