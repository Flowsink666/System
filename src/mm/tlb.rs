//! 快表缓存 (Translation Lookaside Buffer - TLB)
//! 
//! 高性能特性：
//! 1. 模拟硬件 MMU 的虚拟地址转物理页号高速缓存
//! 2. 避免每次访问内存都发生耗时的多级页表遍历 (Page Table Walk)
//! 3. 采用 LRU (最近最少使用) 淘汰策略，保持高局部性命中率
//! 4. 统计命中与未命中次数，量化性能提升

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlbEntry {
    pub vpn: usize,      // 虚拟页号 (Virtual Page Number)
    pub pfn: usize,      // 物理页帧号 (Physical Frame Number)
    pub flags: u8,       // 读/写/执行等权限标志
}

pub struct Tlb {
    pub capacity: usize,
    entries: Vec<Option<TlbEntry>>,
    lru_order: VecDeque<usize>, // 记录 entries 的下标顺序
    pub hits: u64,
    pub misses: u64,
}

impl Tlb {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: vec![None; capacity],
            lru_order: VecDeque::with_capacity(capacity),
            hits: 0,
            misses: 0,
        }
    }

    /// 查询虚拟页号对应的物理页帧
    pub fn lookup(&mut self, vpn: usize) -> Option<TlbEntry> {
        for (idx, entry_opt) in self.entries.iter().enumerate() {
            if let Some(entry) = entry_opt {
                if entry.vpn == vpn {
                    self.hits += 1;
                    // 更新 LRU 顺序
                    if let Some(pos) = self.lru_order.iter().position(|&x| x == idx) {
                        self.lru_order.remove(pos);
                    }
                    self.lru_order.push_back(idx);
                    return Some(*entry);
                }
            }
        }

        self.misses += 1;
        None
    }

    /// 插入或更新 TLB 项
    pub fn insert(&mut self, entry: TlbEntry) {
        // 如果已经存在，直接更新
        for (idx, entry_opt) in self.entries.iter_mut().enumerate() {
            if let Some(e) = entry_opt {
                if e.vpn == entry.vpn {
                    *e = entry;
                    if let Some(pos) = self.lru_order.iter().position(|&x| x == idx) {
                        self.lru_order.remove(pos);
                    }
                    self.lru_order.push_back(idx);
                    return;
                }
            }
        }

        // 寻找空闲槽位 (None) 或根据 LRU 淘汰最久未访问项
        let target_idx = if let Some(free_idx) = self.entries.iter().position(|e| e.is_none()) {
            self.lru_order.push_back(free_idx);
            free_idx
        } else {
            // 缓存已满，淘汰最久未访问的
            let evicted = self.lru_order.pop_front().unwrap();
            self.lru_order.push_back(evicted);
            evicted
        };

        self.entries[target_idx] = Some(entry);
    }

    /// 使特定虚拟页号失效
    pub fn invalidate(&mut self, vpn: usize) {
        for (idx, entry_opt) in self.entries.iter_mut().enumerate() {
            if let Some(e) = entry_opt {
                if e.vpn == vpn {
                    *entry_opt = None;
                    if let Some(pos) = self.lru_order.iter().position(|&x| x == idx) {
                        self.lru_order.remove(pos);
                    }
                    break;
                }
            }
        }
    }

    /// 清空整表（如进程切换导致地址空间变更时调用）
    pub fn flush(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = None;
        }
        self.lru_order.clear();
    }

    /// 计算命中率 (Hit Rate %)
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f64 / total as f64) * 100.0
        }
    }
}
