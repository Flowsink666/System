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

#[derive(Clone)]
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
    pub fn read_block(
        &mut self,
        block_idx: usize,
        buf: &mut [u8; BLOCK_SIZE],
    ) -> Result<(), &'static str> {
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
    pub fn write_block(
        &mut self,
        block_idx: usize,
        buf: &[u8; BLOCK_SIZE],
    ) -> Result<(), &'static str> {
        if block_idx >= self.num_blocks {
            return Err("Block index out of bounds");
        }
        self.blocks[block_idx].copy_from_slice(buf);
        self.stats.write_ops += 1;
        self.stats.bytes_written += BLOCK_SIZE as u64;
        self.stats.simulated_io_cycles += 600; // 模拟写闪存延迟
        Ok(())
    }

    /// 将所有块导出为扁平连续字节流
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.num_blocks * BLOCK_SIZE);
        for block in &self.blocks {
            bytes.extend_from_slice(block);
        }
        bytes
    }

    /// 从扁平字节流恢复虚拟块设备
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(BLOCK_SIZE) {
            return Err("Byte stream length must be a non-zero multiple of BLOCK_SIZE (512)");
        }
        let num_blocks = bytes.len() / BLOCK_SIZE;
        let mut blocks = Vec::with_capacity(num_blocks);
        let (chunks, _) = bytes.as_chunks::<BLOCK_SIZE>();
        for chunk in chunks {
            blocks.push(*chunk);
        }
        Ok(Self {
            blocks,
            num_blocks,
            stats: BlockDevStats::default(),
        })
    }

    /// 保存磁盘镜像到宿主机文件系统
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let bytes = self.to_bytes();
        std::fs::write(path, bytes)
    }

    /// 从宿主机文件系统加载磁盘镜像
    pub fn load_from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
