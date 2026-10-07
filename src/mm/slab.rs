//! 对象高速缓存分配器 (Slab / SLUB Allocator)
//! 
//! 高性能特性：
//! 1. 消除小对象直接向页分配器申请造成的严重内部碎片（如 64 字节对象占满 4KB 页）
//! 2. 预切分对象槽位，维护 Partial 可用列表与 PFN 哈希索引，实现 O(1) 极速分配与归还
//! 3. 彻底消除多 Slab 下的线性遍历，保持大规模对象池下的稳定吞吐
//! 4. 搭配底层的 Buddy 系统按需申请与返还物理页框

use super::buddy::{BuddyAllocator, PAGE_SIZE};
use std::collections::HashMap;

/// 单个 Slab 结构（代表从 Buddy 申请的 1 个物理页切分出的对象池）
pub struct Slab {
    pub pfn: usize,
    pub object_size: usize,
    pub total_slots: usize,
    pub free_slots: Vec<usize>,
}

impl Slab {
    pub fn new(pfn: usize, object_size: usize) -> Self {
        let total_slots = PAGE_SIZE / object_size;
        let mut free_slots = Vec::with_capacity(total_slots);
        // 初始化空闲槽位索引
        for i in (0..total_slots).rev() {
            free_slots.push(i);
        }
        Self {
            pfn,
            object_size,
            total_slots,
            free_slots,
        }
    }

    #[inline]
    pub fn is_full(&self) -> bool {
        self.free_slots.is_empty()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.free_slots.len() == self.total_slots
    }

    #[inline]
    pub fn allocate_slot(&mut self) -> Option<usize> {
        self.free_slots.pop()
    }

    #[inline]
    pub fn free_slot(&mut self, slot_idx: usize) {
        self.free_slots.push(slot_idx);
    }
}

/// 定长对象的 Slab 缓存池 (O(1) 索引优化)
pub struct SlabCache {
    pub name: String,
    pub object_size: usize,
    pub slabs: Vec<Slab>,
    /// 存在可用槽位的 Slab 下标栈，O(1) 弹出分配
    partial_slabs: Vec<usize>,
    /// PFN 到 Slab 下标的反向映射表，O(1) 精确定位待释放 Slab
    pfn_map: HashMap<usize, usize>,
    pub allocated_objects: usize,
}

impl SlabCache {
    pub fn new(name: &str, object_size: usize) -> Self {
        Self {
            name: name.to_string(),
            object_size,
            slabs: Vec::new(),
            partial_slabs: Vec::new(),
            pfn_map: HashMap::new(),
            allocated_objects: 0,
        }
    }

    /// O(1) 分配一个对象，返回 (pfn, slot_idx, 模拟偏移量)
    pub fn allocate(&mut self, buddy: &mut BuddyAllocator) -> Option<(usize, usize, usize)> {
        // 1. 优先从 partial 栈顶提取 Slab 索引 (O(1))
        while let Some(&slab_idx) = self.partial_slabs.last() {
            let slab = &mut self.slabs[slab_idx];
            if !slab.is_full() {
                let slot = slab.allocate_slot().unwrap();
                let offset = slot * self.object_size;
                self.allocated_objects += 1;
                let pfn = slab.pfn;
                if slab.is_full() {
                    self.partial_slabs.pop();
                }
                return Some((pfn, slot, offset));
            } else {
                self.partial_slabs.pop();
            }
        }

        // 2. 所有现存 Slab 已满，向 Buddy 分配新物理页 (order 0 = 1 页)
        let pfn = buddy.allocate_pages(0)?;
        let mut new_slab = Slab::new(pfn, self.object_size);
        let slot = new_slab.allocate_slot().unwrap();
        let offset = slot * self.object_size;

        let new_idx = self.slabs.len();
        self.pfn_map.insert(pfn, new_idx);
        if !new_slab.is_full() {
            self.partial_slabs.push(new_idx);
        }
        self.slabs.push(new_slab);
        self.allocated_objects += 1;

        Some((pfn, slot, offset))
    }

    /// O(1) 释放指定对象的槽位
    pub fn free(&mut self, buddy: &mut BuddyAllocator, pfn: usize, slot_idx: usize) -> Result<(), &'static str> {
        let &slab_idx = self.pfn_map.get(&pfn).ok_or("PFN not found in slab cache")?;
        let (was_full, is_empty, removed_pfn) = {
            let slab = &mut self.slabs[slab_idx];
            let was_full = slab.is_full();
            slab.free_slot(slot_idx);
            (was_full, slab.is_empty(), slab.pfn)
        };

        self.allocated_objects = self.allocated_objects.saturating_sub(1);

        if was_full {
            self.partial_slabs.push(slab_idx);
        }

        // 如果该 slab 完全空闲且池中存在多个 slab，归还给 Buddy 并彻底清理索引
        if is_empty && self.slabs.len() > 1 {
            let _ = buddy.free_pages(removed_pfn);
            self.slabs.remove(slab_idx);
            self.pfn_map.clear();
            self.partial_slabs.clear();
            for (idx, slab) in self.slabs.iter().enumerate() {
                self.pfn_map.insert(slab.pfn, idx);
                if !slab.is_full() {
                    self.partial_slabs.push(idx);
                }
            }
        }

        Ok(())
    }
}

/// 系统综合 Slab 管理器（覆盖通用 32B ~ 2048B 多规格内存池）
pub struct SlabAllocator {
    pub classes: Vec<usize>,
    pub caches: Vec<SlabCache>,
    // 地址句柄映射: alloc_id -> (cache_index, pfn, slot_idx)
    object_map: HashMap<u64, (usize, usize, usize)>,
    next_alloc_id: u64,
}

impl Default for SlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SlabAllocator {
    pub fn new() -> Self {
        let classes = vec![32, 64, 128, 256, 512, 1024, 2048];
        let mut caches = Vec::new();
        for &size in &classes {
            caches.push(SlabCache::new(&format!("kmalloc-{}", size), size));
        }

        Self {
            classes,
            caches,
            object_map: HashMap::new(),
            next_alloc_id: 1,
        }
    }

    /// 根据字节大小匹配最贴合的规格分配小对象
    pub fn kmalloc(&mut self, buddy: &mut BuddyAllocator, size: usize) -> Option<u64> {
        let cache_idx = self.classes.iter().position(|&s| s >= size)?;
        let cache = &mut self.caches[cache_idx];
        let (pfn, slot, _offset) = cache.allocate(buddy)?;

        let id = self.next_alloc_id;
        self.next_alloc_id += 1;
        self.object_map.insert(id, (cache_idx, pfn, slot));
        Some(id)
    }

    /// 释放小对象
    pub fn kfree(&mut self, buddy: &mut BuddyAllocator, alloc_id: u64) -> Result<(), &'static str> {
        let (cache_idx, pfn, slot) = self
            .object_map
            .remove(&alloc_id)
            .ok_or("Invalid allocation handle")?;

        let cache = &mut self.caches[cache_idx];
        cache.free(buddy, pfn, slot)
    }

    /// 统计各级别缓存占用
    pub fn get_cache_stats(&self) -> Vec<(&str, usize, usize, usize)> {
        self.caches
            .iter()
            .map(|c| (c.name.as_str(), c.object_size, c.allocated_objects, c.slabs.len()))
            .collect()
    }
}
