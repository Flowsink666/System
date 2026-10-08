//! Block-device/cache timings with checked reads and identical final contents.
//! Host elapsed time and the fixed I/O-cycle model are reported separately.

use super::{collect_samples, median, median_f64};
use crate::fs::{BufferCache, VirtualBlockDevice, BLOCK_SIZE};
use std::hint::black_box;
use std::time::{Duration, Instant};

pub struct FsBenchResult {
    pub cached_time_us: u128,
    pub uncached_time_us: u128,
    pub host_cached_to_direct_ratio: f64,
    pub cache_hit_rate: f64,
    pub simulated_io_speedup: f64,
    pub io_cycles_saved: u64,
}

const TOTAL_BLOCKS: usize = 4096;
const CACHE_CAPACITY: usize = 128;
const IO_OPERATIONS: usize = 4000;
const HOT_BLOCKS: usize = 64;

struct Operation {
    block: usize,
    write: bool,
    bytes: [u8; BLOCK_SIZE], // Write payload or the expected result of a read.
}

struct Workload {
    operations: Vec<Operation>,
    final_blocks: Vec<[u8; BLOCK_SIZE]>,
    read_count: usize,
}

impl Workload {
    fn new() -> Self {
        let mut blocks = vec![[0; BLOCK_SIZE]; TOTAL_BLOCKS];
        let mut operations = Vec::with_capacity(IO_OPERATIONS);
        let mut read_count = 0;
        for index in 0..IO_OPERATIONS {
            let block = if index % 10 < 8 { index % HOT_BLOCKS } else { (index * 7) % TOTAL_BLOCKS };
            let write = index % 2 != 0;
            let bytes = if write {
                let bytes = std::array::from_fn(|offset| ((index + offset * 17) % 251) as u8);
                blocks[block] = bytes;
                bytes
            } else {
                read_count += 1;
                blocks[block]
            };
            operations.push(Operation { block, write, bytes });
        }
        Self { operations, final_blocks: blocks, read_count }
    }

    fn check(&self, dev: &VirtualBlockDevice, reads: &[[u8; BLOCK_SIZE]]) {
        assert_eq!(reads.len(), self.read_count);
        for (operation, actual) in self.operations.iter().filter(|operation| !operation.write).zip(reads) {
            assert_eq!(actual, &operation.bytes, "incorrect benchmark read at block {}", operation.block);
        }
        assert_eq!(dev.blocks, self.final_blocks, "benchmark final device contents differ from reference");
    }
}

struct IoSample {
    duration: Duration,
    cycles: u64,
    device_ops: u64,
    hits: u64,
    misses: u64,
    writebacks: u64,
}

struct PairSample {
    direct: IoSample,
    cached: IoSample,
}

fn direct_sample(workload: &Workload) -> IoSample {
    let mut dev = VirtualBlockDevice::new(TOTAL_BLOCKS);
    let mut reads = vec![[0; BLOCK_SIZE]; workload.read_count];
    let mut read_index = 0;
    let start = Instant::now();
    for operation in &workload.operations {
        if operation.write {
            dev.write_block(operation.block, black_box(&operation.bytes)).expect("direct benchmark write failed");
        } else {
            dev.read_block(operation.block, &mut reads[read_index]).expect("direct benchmark read failed");
            black_box(&reads[read_index]);
            read_index += 1;
        }
    }
    let duration = start.elapsed();
    workload.check(&dev, &reads);
    IoSample {
        duration,
        cycles: dev.stats.simulated_io_cycles,
        device_ops: dev.stats.read_ops + dev.stats.write_ops,
        hits: 0,
        misses: 0,
        writebacks: 0,
    }
}

fn cached_sample(workload: &Workload) -> IoSample {
    let mut dev = VirtualBlockDevice::new(TOTAL_BLOCKS);
    let mut cache = BufferCache::new(CACHE_CAPACITY);
    let mut reads = vec![[0; BLOCK_SIZE]; workload.read_count];
    let mut read_index = 0;
    let start = Instant::now();
    for operation in &workload.operations {
        if operation.write {
            cache.write_block(&mut dev, operation.block, black_box(&operation.bytes)).expect("cached benchmark write failed");
        } else {
            cache.read_block(&mut dev, operation.block, &mut reads[read_index]).expect("cached benchmark read failed");
            black_box(&reads[read_index]);
            read_index += 1;
        }
    }
    cache.sync(&mut dev).expect("cached benchmark writeback failed");
    let duration = start.elapsed();
    workload.check(&dev, &reads);
    IoSample {
        duration,
        cycles: dev.stats.simulated_io_cycles,
        device_ops: dev.stats.read_ops + dev.stats.write_ops,
        hits: cache.hits,
        misses: cache.misses,
        writebacks: cache.writebacks,
    }
}

pub fn run_fs_benchmark() -> FsBenchResult {
    println!("\n========== [3/3] 块设备与块缓存性能基准测试 ==========");
    let workload = Workload::new();
    let mut round = 0;
    let samples = collect_samples(|| {
        // Alternate timing order to reduce systematic first/second-run effects.
        round += 1;
        if round % 2 == 0 {
            let cached = cached_sample(&workload);
            PairSample { direct: direct_sample(&workload), cached }
        } else {
            let direct = direct_sample(&workload);
            PairSample { direct, cached: cached_sample(&workload) }
        }
    });
    let direct_time = median(samples.iter().map(|sample| sample.direct.duration).collect());
    let cached_time = median(samples.iter().map(|sample| sample.cached.duration).collect());
    let host_cached_to_direct_ratio = median_f64(samples.iter().map(|sample| sample.cached.duration.as_secs_f64() / sample.direct.duration.as_secs_f64()).collect());
    let direct_cycles = median(samples.iter().map(|sample| sample.direct.cycles).collect());
    let cached_cycles = median(samples.iter().map(|sample| sample.cached.cycles).collect());
    let direct_ops = median(samples.iter().map(|sample| sample.direct.device_ops).collect());
    let cached_ops = median(samples.iter().map(|sample| sample.cached.device_ops).collect());
    let hits = median(samples.iter().map(|sample| sample.cached.hits).collect());
    let misses = median(samples.iter().map(|sample| sample.cached.misses).collect());
    let writebacks = median(samples.iter().map(|sample| sample.cached.writebacks).collect());
    let cache_hit_rate = hits as f64 / (hits + misses) as f64 * 100.0;
    let io_cycles_saved = direct_cycles.saturating_sub(cached_cycles);
    let simulated_io_speedup = direct_cycles as f64 / cached_cycles.max(1) as f64;

    println!("    - 每轮 {IO_OPERATIONS} 次读写，80% 访问集中在 {HOT_BLOCKS} 个热点块；缓存容量 {CACHE_CAPACITY}。");
    println!("    - 每轮校验 {} 次读取及所有 {TOTAL_BLOCKS} 块最终内容；直接与缓存使用相同初态和负载。", workload.read_count);
    println!("    - 计时不含初始化与内容校验，带缓存计时包含最后的脏块刷新。");
    println!("  [宿主机实际执行时间]");
    println!("    - 直接访问中位数: {direct_time:.2?}，带缓存中位数: {cached_time:.2?}");
    println!("    - 缓存/直接耗时比中位数: {host_cached_to_direct_ratio:.2}x（越小越快）");
    println!("  [固定 I/O 周期模型：读取 500、写入 600 周期/块]");
    println!("    - 直接访问: {direct_ops} 次设备操作，{direct_cycles} 模拟周期");
    println!("    - 带缓存: {cached_ops} 次设备操作，{cached_cycles} 模拟周期");
    println!("    - 命中 {hits} 次，未命中 {misses} 次，回写 {writebacks} 次；命中率 {cache_hit_rate:.2}%");
    println!("    - 模型周期收益: {simulated_io_speedup:.2}x，减少 {io_cycles_saved} 周期");

    FsBenchResult {
        cached_time_us: cached_time.as_micros(),
        uncached_time_us: direct_time.as_micros(),
        host_cached_to_direct_ratio,
        cache_hit_rate,
        simulated_io_speedup,
        io_cycles_saved,
    }
}
