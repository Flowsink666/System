//! 内存管理子系统 (Memory Management Subsystem)

pub mod buddy;
pub mod page_table;
pub mod slab;
pub mod tlb;

pub use buddy::{BuddyAllocator, BuddyStats, MAX_ORDER, PAGE_SIZE};
pub use page_table::{
    AccessType, AddressSpace, FLAG_DIRTY, FLAG_PRESENT, FLAG_USER, FLAG_WRITABLE,
};
pub use slab::SlabAllocator;
pub use tlb::{Tlb, TlbEntry};

pub struct MemoryManager {
    pub buddy: BuddyAllocator,
    pub slab: SlabAllocator,
    pub kernel_space: AddressSpace,
    pub ram: Vec<u8>, // 模拟物理内存存储
}

impl MemoryManager {
    pub fn new(total_pages: usize) -> Self {
        let ram_bytes = total_pages * PAGE_SIZE;
        Self {
            buddy: BuddyAllocator::new(total_pages),
            slab: SlabAllocator::new(),
            kernel_space: AddressSpace::new(64),
            ram: vec![0u8; ram_bytes],
        }
    }

    /// 物理内存写入
    pub fn write_physical(&mut self, paddr: usize, data: &[u8]) -> Result<(), &'static str> {
        let end_paddr = paddr.checked_add(data.len()).ok_or("Physical address overflow")?;
        if end_paddr > self.ram.len() {
            return Err("Physical memory access out of bounds");
        }
        self.ram[paddr..end_paddr].copy_from_slice(data);
        Ok(())
    }

    /// 物理内存读取
    pub fn read_physical(&self, paddr: usize, buf: &mut [u8]) -> Result<(), &'static str> {
        let end_paddr = paddr.checked_add(buf.len()).ok_or("Physical address overflow")?;
        if end_paddr > self.ram.len() {
            return Err("Physical memory access out of bounds");
        }
        buf.copy_from_slice(&self.ram[paddr..end_paddr]);
        Ok(())
    }

    /// 跨页安全的虚拟内存写入：
    /// 1. 逐页拆分写入，正确处理物理上不连续的相邻虚拟页
    /// 2. 严格检查写权限与特权级
    /// 3. 地址算术防溢出
    pub fn write_virtual_checked(
        &mut self,
        space: &mut AddressSpace,
        vaddr: usize,
        data: &[u8],
        is_user: bool,
    ) -> Result<usize, &'static str> {
        let _ = vaddr.checked_add(data.len()).ok_or("Virtual address overflow")?;
        let mut bytes_written = 0;

        while bytes_written < data.len() {
            let curr_va = vaddr + bytes_written;
            let page_offset = curr_va % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_offset).min(data.len() - bytes_written);

            let paddr = space.translate_checked(curr_va, AccessType::Write, is_user)?;
            self.write_physical(paddr, &data[bytes_written..bytes_written + chunk])?;

            bytes_written += chunk;
        }

        Ok(bytes_written)
    }

    /// 兼容接口：内核特权写入
    pub fn write_virtual(
        &mut self,
        space: &mut AddressSpace,
        vaddr: usize,
        data: &[u8],
    ) -> Result<(), &'static str> {
        self.write_virtual_checked(space, vaddr, data, false).map(|_| ())
    }

    /// 跨页安全的虚拟内存读取：逐页检查与读取
    pub fn read_virtual_checked(
        &mut self,
        space: &mut AddressSpace,
        vaddr: usize,
        buf: &mut [u8],
        is_user: bool,
    ) -> Result<usize, &'static str> {
        let _ = vaddr.checked_add(buf.len()).ok_or("Virtual address overflow")?;
        let mut bytes_read = 0;

        while bytes_read < buf.len() {
            let curr_va = vaddr + bytes_read;
            let page_offset = curr_va % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_offset).min(buf.len() - bytes_read);

            let paddr = space.translate_checked(curr_va, AccessType::Read, is_user)?;
            self.read_physical(paddr, &mut buf[bytes_read..bytes_read + chunk])?;

            bytes_read += chunk;
        }

        Ok(bytes_read)
    }

    /// 兼容接口：内核特权读取
    pub fn read_virtual(
        &mut self,
        space: &mut AddressSpace,
        vaddr: usize,
        buf: &mut [u8],
    ) -> Result<(), &'static str> {
        self.read_virtual_checked(space, vaddr, buf, false).map(|_| ())
    }
}
