//! 索引节点与多级数据块映射 (Inode & Direct/Indirect Block Allocation)
//! 
//! 高性能特性：
//! 1. 支持 12 个直接块 (Direct Blocks) 与 1 个一级间接块 (Indirect Block)
//! 2. 单文件最大容量扩展至 140 块 (70KB)
//! 3. 支持块资源的精准回收与释放，彻底消除磁盘存储泄露

use super::block_dev::{VirtualBlockDevice, BLOCK_SIZE};
use super::buffer_cache::BufferCache;

pub const DIRECT_BLOCKS_COUNT: usize = 12;
pub const PTRS_PER_INDIRECT_BLOCK: usize = BLOCK_SIZE / 4; // 512B / 4B = 128 个块指针
pub const MAX_FILE_BLOCKS: usize = DIRECT_BLOCKS_COUNT + PTRS_PER_INDIRECT_BLOCK;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    Regular,
    Directory,
    Device,
    Pipe,
}

#[derive(Debug, Clone)]
pub struct Inode {
    pub id: usize,
    pub inode_type: InodeType,
    pub size: usize,
    pub direct_blocks: [usize; DIRECT_BLOCKS_COUNT],
    pub direct_blocks_used: usize,
    pub indirect_block: Option<usize>,
    pub indirect_blocks_used: usize,
    pub permissions: u16,
    pub created_at: u64,
    pub modified_at: u64,
}

impl Inode {
    pub fn new(id: usize, inode_type: InodeType, permissions: u16) -> Self {
        Self {
            id,
            inode_type,
            size: 0,
            direct_blocks: [0; DIRECT_BLOCKS_COUNT],
            direct_blocks_used: 0,
            indirect_block: None,
            indirect_blocks_used: 0,
            permissions,
            created_at: 0,
            modified_at: 0,
        }
    }

    /// 获取文件占用的逻辑块号对应的物理块号
    pub fn get_block_index(
        &self,
        cache: &mut BufferCache,
        dev: &mut VirtualBlockDevice,
        logical_block: usize,
    ) -> Option<usize> {
        if logical_block < self.direct_blocks_used {
            Some(self.direct_blocks[logical_block])
        } else if logical_block < DIRECT_BLOCKS_COUNT + self.indirect_blocks_used {
            let indirect_pblock = self.indirect_block?;
            let idx_in_indirect = logical_block - DIRECT_BLOCKS_COUNT;

            let mut block_data = [0u8; BLOCK_SIZE];
            if cache.read_block(dev, indirect_pblock, &mut block_data).is_ok() {
                let offset = idx_in_indirect * 4;
                let pblock = u32::from_le_bytes([
                    block_data[offset],
                    block_data[offset + 1],
                    block_data[offset + 2],
                    block_data[offset + 3],
                ]) as usize;
                Some(pblock)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// 为文件追加一个新的物理数据块 (支持直接块与一级间接块扩展)
    pub fn append_block(
        &mut self,
        cache: &mut BufferCache,
        dev: &mut VirtualBlockDevice,
        free_blocks: &mut Vec<usize>,
        physical_block: usize,
    ) -> Result<(), &'static str> {
        if self.direct_blocks_used < DIRECT_BLOCKS_COUNT {
            self.direct_blocks[self.direct_blocks_used] = physical_block;
            self.direct_blocks_used += 1;
            Ok(())
        } else if self.indirect_blocks_used < PTRS_PER_INDIRECT_BLOCK {
            // 如果还未分配间接块索引节点自身，先分配一个索引物理块
            if self.indirect_block.is_none() {
                let indirect_pblock = free_blocks.pop().ok_or("Disk full: no block for indirect table")?;
                let empty_table = [0u8; BLOCK_SIZE];
                cache.write_block(dev, indirect_pblock, &empty_table)?;
                self.indirect_block = Some(indirect_pblock);
            }

            let indirect_pblock = self.indirect_block.unwrap();
            let mut block_data = [0u8; BLOCK_SIZE];
            cache.read_block(dev, indirect_pblock, &mut block_data)?;

            let offset = self.indirect_blocks_used * 4;
            let bytes = (physical_block as u32).to_le_bytes();
            block_data[offset..offset + 4].copy_from_slice(&bytes);
            cache.write_block(dev, indirect_pblock, &block_data)?;

            self.indirect_blocks_used += 1;
            Ok(())
        } else {
            Err("Max file size reached (140 blocks limit)")
        }
    }

    /// 从文件读取数据（经由 BufferCache）
    pub fn read_bytes(
        &self,
        cache: &mut BufferCache,
        dev: &mut VirtualBlockDevice,
        offset: usize,
        buf: &mut [u8],
    ) -> Result<usize, &'static str> {
        if offset >= self.size {
            return Ok(0);
        }

        let bytes_to_read = buf.len().min(self.size - offset);
        let mut bytes_read = 0;

        while bytes_read < bytes_to_read {
            let current_pos = offset + bytes_read;
            let logical_block = current_pos / BLOCK_SIZE;
            let block_offset = current_pos % BLOCK_SIZE;

            let physical_block = self
                .get_block_index(cache, dev, logical_block)
                .ok_or("Invalid logical block in file")?;

            let mut block_data = [0u8; BLOCK_SIZE];
            cache.read_block(dev, physical_block, &mut block_data)?;

            let chunk = (BLOCK_SIZE - block_offset).min(bytes_to_read - bytes_read);
            buf[bytes_read..bytes_read + chunk]
                .copy_from_slice(&block_data[block_offset..block_offset + chunk]);
            bytes_read += chunk;
        }

        Ok(bytes_read)
    }

    /// 向文件写入数据（经由 BufferCache，按需申请物理块并零初始化）
    pub fn write_bytes(
        &mut self,
        cache: &mut BufferCache,
        dev: &mut VirtualBlockDevice,
        free_blocks: &mut Vec<usize>,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, &'static str> {
        // 如果写入位置超过当前文件大小，零填充中间空洞 (Sparse file filling)
        if offset > self.size {
            let hole_len = offset - self.size;
            let zeroes = vec![0u8; hole_len];
            let old_size = self.size;
            self.write_bytes(cache, dev, free_blocks, old_size, &zeroes)?;
        }

        let mut bytes_written = 0;

        while bytes_written < data.len() {
            let current_pos = offset + bytes_written;
            let logical_block = current_pos / BLOCK_SIZE;
            let block_offset = current_pos % BLOCK_SIZE;

            // 如果该逻辑块还未分配物理块，分配新物理块
            let physical_block = match self.get_block_index(cache, dev, logical_block) {
                Some(pblock) => pblock,
                None => {
                    let new_pblock = free_blocks.pop().ok_or("Disk full: no free data blocks")?;
                    if let Err(e) = self.append_block(cache, dev, free_blocks, new_pblock) {
                        free_blocks.push(new_pblock); // 失败时归还，消除泄漏
                        return Err(e);
                    }
                    // 新块必须零初始化，防止脏数据残留
                    let zero_block = [0u8; BLOCK_SIZE];
                    cache.write_block(dev, new_pblock, &zero_block)?;
                    new_pblock
                }
            };

            let mut block_data = [0u8; BLOCK_SIZE];
            // 读取现有块内容
            let _ = cache.read_block(dev, physical_block, &mut block_data);

            let chunk = (BLOCK_SIZE - block_offset).min(data.len() - bytes_written);
            block_data[block_offset..block_offset + chunk]
                .copy_from_slice(&data[bytes_written..bytes_written + chunk]);

            cache.write_block(dev, physical_block, &block_data)?;

            bytes_written += chunk;
            if current_pos + chunk > self.size {
                self.size = current_pos + chunk;
            }
        }

        Ok(bytes_written)
    }

    /// 释放该 Inode 占用的所有底层物理块（包括直接块、间接块索引以及数据块）
    pub fn release_all_blocks(
        &mut self,
        cache: &mut BufferCache,
        dev: &mut VirtualBlockDevice,
    ) -> Vec<usize> {
        let mut freed = Vec::new();

        // 收集直接块
        for i in 0..self.direct_blocks_used {
            freed.push(self.direct_blocks[i]);
            self.direct_blocks[i] = 0;
        }
        self.direct_blocks_used = 0;

        // 收集间接块内的数据块及间接块自身
        if let Some(indirect_pblock) = self.indirect_block {
            let mut block_data = [0u8; BLOCK_SIZE];
            if cache.read_block(dev, indirect_pblock, &mut block_data).is_ok() {
                for i in 0..self.indirect_blocks_used {
                    let offset = i * 4;
                    let pblock = u32::from_le_bytes([
                        block_data[offset],
                        block_data[offset + 1],
                        block_data[offset + 2],
                        block_data[offset + 3],
                    ]) as usize;
                    freed.push(pblock);
                }
            }
            freed.push(indirect_pblock);
            self.indirect_block = None;
            self.indirect_blocks_used = 0;
        }

        self.size = 0;
        freed
    }
}
