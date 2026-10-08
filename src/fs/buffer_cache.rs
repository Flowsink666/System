//! LRU block cache with delayed writeback. Stable node indices make hits,
//! eviction and invalidation independent of cache capacity.

use super::block_dev::{BLOCK_SIZE, VirtualBlockDevice};
use std::collections::HashMap;

#[derive(Clone)]
pub struct CachedBlock {
    pub block_idx: usize,
    pub data: [u8; BLOCK_SIZE],
    pub is_dirty: bool,
    lru_slot: usize,
}

#[derive(Clone)]
struct LruNode {
    block_idx: usize,
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Clone)]
pub struct BufferCache {
    pub capacity: usize,
    pub cache: HashMap<usize, CachedBlock>,
    nodes: Vec<LruNode>,
    free_nodes: Vec<usize>,
    oldest: Option<usize>,
    newest: Option<usize>,
    pub hits: u64,
    pub misses: u64,
    pub writebacks: u64,
}

impl BufferCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            cache: HashMap::with_capacity(capacity),
            nodes: Vec::with_capacity(capacity),
            free_nodes: Vec::new(),
            oldest: None,
            newest: None,
            hits: 0,
            misses: 0,
            writebacks: 0,
        }
    }

    pub fn read_block(
        &mut self,
        dev: &mut VirtualBlockDevice,
        block_idx: usize,
        out_buf: &mut [u8; BLOCK_SIZE],
    ) -> Result<(), &'static str> {
        if let Some(cached) = self.cache.get(&block_idx) {
            self.hits += 1;
            out_buf.copy_from_slice(&cached.data);
            let slot = cached.lru_slot;
            self.touch_lru(slot);
            return Ok(());
        }

        self.misses += 1;
        let mut disk_data = [0u8; BLOCK_SIZE];
        dev.read_block(block_idx, &mut disk_data)?;
        if self.capacity > 0 {
            self.ensure_capacity(dev)?;
            self.insert(block_idx, disk_data, false);
        }
        out_buf.copy_from_slice(&disk_data);
        Ok(())
    }

    pub fn write_block(
        &mut self,
        dev: &mut VirtualBlockDevice,
        block_idx: usize,
        in_buf: &[u8; BLOCK_SIZE],
    ) -> Result<(), &'static str> {
        if block_idx >= dev.num_blocks || block_idx >= dev.blocks.len() {
            return Err("Block index out of bounds");
        }
        if let Some(cached) = self.cache.get_mut(&block_idx) {
            self.hits += 1;
            cached.data.copy_from_slice(in_buf);
            cached.is_dirty = true;
            let slot = cached.lru_slot;
            self.touch_lru(slot);
            return Ok(());
        }

        self.misses += 1;
        if self.capacity == 0 {
            return dev.write_block(block_idx, in_buf);
        }
        self.ensure_capacity(dev)?;
        self.insert(block_idx, *in_buf, true);
        Ok(())
    }

    pub fn sync(&mut self, dev: &mut VirtualBlockDevice) -> Result<(), &'static str> {
        for cached in self.cache.values_mut() {
            if cached.is_dirty {
                dev.write_block(cached.block_idx, &cached.data)?;
                cached.is_dirty = false;
                self.writebacks += 1;
            }
        }
        Ok(())
    }

    fn insert(&mut self, block_idx: usize, data: [u8; BLOCK_SIZE], is_dirty: bool) {
        let node = LruNode {
            block_idx,
            prev: self.newest,
            next: None,
        };
        let slot = if let Some(slot) = self.free_nodes.pop() {
            self.nodes[slot] = node;
            slot
        } else {
            let slot = self.nodes.len();
            self.nodes.push(node);
            slot
        };
        if let Some(newest) = self.newest {
            self.nodes[newest].next = Some(slot);
        } else {
            self.oldest = Some(slot);
        }
        self.newest = Some(slot);
        self.cache.insert(
            block_idx,
            CachedBlock {
                block_idx,
                data,
                is_dirty,
                lru_slot: slot,
            },
        );
    }

    fn detach(&mut self, slot: usize) {
        let (prev, next) = (self.nodes[slot].prev, self.nodes[slot].next);
        if let Some(prev) = prev {
            self.nodes[prev].next = next;
        } else {
            self.oldest = next;
        }
        if let Some(next) = next {
            self.nodes[next].prev = prev;
        } else {
            self.newest = prev;
        }
    }

    fn touch_lru(&mut self, slot: usize) {
        if self.newest == Some(slot) {
            return;
        }
        self.detach(slot);
        self.nodes[slot].prev = self.newest;
        self.nodes[slot].next = None;
        if let Some(newest) = self.newest {
            self.nodes[newest].next = Some(slot);
        } else {
            self.oldest = Some(slot);
        }
        self.newest = Some(slot);
    }

    // A failed write leaves both the dirty block and its position intact.
    fn ensure_capacity(&mut self, dev: &mut VirtualBlockDevice) -> Result<(), &'static str> {
        while self.cache.len() >= self.capacity {
            let slot = self.oldest.ok_or("Invalid cache LRU state")?;
            let block_idx = self.nodes[slot].block_idx;
            let block = self
                .cache
                .get(&block_idx)
                .ok_or("Invalid cache LRU state")?;
            if block.is_dirty {
                dev.write_block(block_idx, &block.data)?;
                self.writebacks += 1;
            }
            self.invalidate(block_idx);
        }
        Ok(())
    }

    pub fn invalidate(&mut self, block_idx: usize) {
        if let Some(cached) = self.cache.remove(&block_idx) {
            self.detach(cached.lru_slot);
            self.free_nodes.push(cached.lru_slot);
        }
    }

    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64 * 100.0
        }
    }
}
