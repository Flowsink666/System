//! 虚拟块存储设备 (Virtual Block Device)
//! 
//! 模拟物理硬盘/闪存存储介质，具备块读写延迟统计功能

pub const BLOCK_SIZE: usize = 512;

#[derive(Debug, Clone, Default)]
pub struct BlockDevStats {
    pub read_ops: u64,
    pub write_ops: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub simulated_io_cycles: u64,
}

pub struct VirtualBlockDevice {
    pub blocks: Vec<[u8; BLOCK_SIZE]>,
    pub num_blocks: usize,
    pub stats: BlockDevStats,
}

impl VirtualBlockDevice {
    pub fn new(num_blocks: usize) -> Self {
        Self {
            blocks: vec![[0u8; BLOCK_SIZE]; num_blocks],
            num_blocks,
            stats: BlockDevStats::default(),
        }
    }

    /// 读取物理块数据（模拟较慢的底层 I/O 访问）
    pub fn read_block(&mut self, block_idx: usize, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), &'static str> {
        if block_idx >= self.num_blocks {
            return Err("Block index out of bounds");
        }
        buf.copy_from_slice(&self.blocks[block_idx]);
        self.stats.read_ops += 1;
        self.stats.bytes_read += BLOCK_SIZE as u64;
        self.stats.simulated_io_cycles += 500; // 模拟磁盘寻道与传输延迟
        Ok(())
    }

    /// 写入物理块数据
    pub fn write_block(&mut self, block_idx: usize, buf: &[u8; BLOCK_SIZE]) -> Result<(), &'static str> {
        if block_idx >= self.num_blocks {
            return Err("Block index out of bounds");
        }
        self.blocks[block_idx].copy_from_slice(buf);
        self.stats.write_ops += 1;
        self.stats.bytes_written += BLOCK_SIZE as u64;
        self.stats.simulated_io_cycles += 600; // 模拟写闪存延迟
        Ok(())
    }
}
