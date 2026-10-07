//! 物理页伙伴系统 (Buddy Allocator)
//! 
//! 高性能特性：
//! 1. 按 2 的幂次方对齐管理物理页框 (Page Frames)
//! 2. 位运算 O(1) 伙伴地址计算：buddy_pfn = pfn ^ (1 << order)
//! 3. 递归合并伙伴块，极大减轻外部碎片化
//! 4. 多阶链表分配，单次分配/释放复杂度为 O(log MAX_ORDER)

use std::collections::HashSet;

pub const PAGE_SIZE: usize = 4096;
pub const MAX_ORDER: usize = 10; // 最大块为 2^10 * 4KB = 4MB

#[derive(Debug, Clone)]
pub struct BuddyStats {
    pub total_pages: usize,
    pub allocated_pages: usize,
    pub free_pages: usize,
    pub alloc_requests: u64,
    pub free_requests: u64,
    pub merges_count: u64,
    pub splits_count: u64,
}

pub struct BuddyAllocator {
    pub total_pages: usize,
    /// 每个 order 维护一组空闲块的起始页帧号 (Page Frame Number, PFN)
    free_lists: Vec<HashSet<usize>>,
    /// 跟踪已分配块的 order，用于释放时确认大小: pfn -> order
    allocated_blocks: std::collections::HashMap<usize, usize>,
    /// 统计信息
    pub stats: BuddyStats,
}

impl BuddyAllocator {
    pub fn new(total_pages: usize) -> Self {
        let mut free_lists = Vec::with_capacity(MAX_ORDER + 1);
        for _ in 0..=MAX_ORDER {
            free_lists.push(HashSet::new());
        }

        let mut allocator = Self {
            total_pages,
            free_lists,
            allocated_blocks: std::collections::HashMap::new(),
            stats: BuddyStats {
                total_pages,
                allocated_pages: 0,
                free_pages: total_pages,
                alloc_requests: 0,
                free_requests: 0,
                merges_count: 0,
                splits_count: 0,
            },
        };

        // 初始化时将所有内存按最大可容纳的 2^order 块加入 free_lists
        let mut curr_pfn = 0;
        let mut remaining = total_pages;

        while remaining > 0 {
            let mut order = MAX_ORDER;
            while (1 << order) > remaining || (curr_pfn % (1 << order)) != 0 {
                if order == 0 {
                    break;
                }
                order -= 1;
            }

            allocator.free_lists[order].insert(curr_pfn);
            curr_pfn += 1 << order;
            remaining -= 1 << order;
        }

        allocator
    }

    /// 分配指定 order (2^order 页) 的连续物理内存
    pub fn allocate_pages(&mut self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        self.stats.alloc_requests += 1;

        // 寻找满足要求的最小可用 order
        let mut target_order = order;
        while target_order <= MAX_ORDER && self.free_lists[target_order].is_empty() {
            target_order += 1;
        }

        if target_order > MAX_ORDER {
            // 物理内存不足
            return None;
        }

        // 取出一个块
        let pfn = *self.free_lists[target_order].iter().next().unwrap();
        self.free_lists[target_order].remove(&pfn);

        // 如果取出的块比需求大，则逐级拆分 (Split)
        let current_block = pfn;
        while target_order > order {
            target_order -= 1;
            self.stats.splits_count += 1;
            // 拆分为两个对半的伙伴块，高半部分放入 free_list
            let buddy = current_block + (1 << target_order);
            self.free_lists[target_order].insert(buddy);
        }

        let allocated_count = 1 << order;
        self.stats.allocated_pages += allocated_count;
        self.stats.free_pages -= allocated_count;
        self.allocated_blocks.insert(current_block, order);

        Some(current_block)
    }

    /// 释放物理页块并递归合并 (Coalescing)
    pub fn free_pages(&mut self, pfn: usize) -> Result<(), &'static str> {
        let order = match self.allocated_blocks.remove(&pfn) {
            Some(ord) => ord,
            None => return Err("Invalid free: PFN not allocated or double free"),
        };

        self.stats.free_requests += 1;
        let page_count = 1 << order;
        self.stats.allocated_pages -= page_count;
        self.stats.free_pages += page_count;

        let mut curr_pfn = pfn;
        let mut curr_order = order;

        // 尝试向上合并伙伴
        while curr_order < MAX_ORDER {
            let buddy_pfn = curr_pfn ^ (1 << curr_order);

            // 检查伙伴块是否在当前 order 的空闲列表中且对齐
            if self.free_lists[curr_order].contains(&buddy_pfn) {
                // 合并伙伴
                self.free_lists[curr_order].remove(&buddy_pfn);
                curr_pfn = curr_pfn.min(buddy_pfn);
                curr_order += 1;
                self.stats.merges_count += 1;
            } else {
                break;
            }
        }

        self.free_lists[curr_order].insert(curr_pfn);
        Ok(())
    }

    /// 获取每个阶数当前的空闲块数量
    pub fn free_blocks_by_order(&self) -> Vec<(usize, usize)> {
        self.free_lists
            .iter()
            .enumerate()
            .map(|(order, set)| (order, set.len()))
            .collect()
    }

    /// 估算当前物理内存外部碎片率：
    /// 统计散落在小于最大阶 (Order < MAX_ORDER) 的空闲页所占比例
    /// 当所有空闲页均为最大连续块时碎片率为 0.0%，当大块被频繁拆解细碎时碎片率接近 1.0%
    pub fn external_fragmentation_ratio(&self) -> f64 {
        if self.stats.free_pages == 0 {
            return 0.0;
        }

        let non_max_free_pages: usize = self
            .free_lists
            .iter()
            .enumerate()
            .take(MAX_ORDER)
            .map(|(order, set)| set.len() * (1 << order))
            .sum();

        non_max_free_pages as f64 / self.stats.free_pages as f64
    }
}
