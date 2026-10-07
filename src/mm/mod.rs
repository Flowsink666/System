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
    ram: Vec<u8>, // 通过受检查的物理内存接口访问，禁止替换缓冲区
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

    /// 分配并清零完整页块。Buddy 只管理页号，不持有 RAM 指针。
    pub fn allocate_pages_zeroed(&mut self, order: usize) -> Result<usize, &'static str> {
        let pfn = self
            .buddy
            .allocate_pages(order)
            .ok_or("Out of physical memory")?;
        let range = pfn.checked_mul(PAGE_SIZE).and_then(|start| {
            PAGE_SIZE
                .checked_shl(order as u32)
                .and_then(|bytes| start.checked_add(bytes))
                .map(|end| start..end)
        });
        if let Some(bytes) = range.and_then(|range| self.ram.get_mut(range)) {
            bytes.fill(0);
            Ok(pfn)
        } else {
            self.buddy.free_pages(pfn)?;
            Err("Physical memory access out of bounds")
        }
    }

    pub(crate) fn copy_page(&mut self, source: usize, target: usize) -> Result<(), &'static str> {
        let source_start = source
            .checked_mul(PAGE_SIZE)
            .ok_or("Physical address overflow")?;
        let source_end = source_start
            .checked_add(PAGE_SIZE)
            .ok_or("Physical address overflow")?;
        let target_start = target
            .checked_mul(PAGE_SIZE)
            .ok_or("Physical address overflow")?;
        let target_end = target_start
            .checked_add(PAGE_SIZE)
            .ok_or("Physical address overflow")?;
        if source_end > self.ram.len() || target_end > self.ram.len() {
            return Err("Physical memory access out of bounds");
        }
        self.ram.copy_within(source_start..source_end, target_start);
        Ok(())
    }

    /// 物理内存写入
    pub fn write_physical(&mut self, paddr: usize, data: &[u8]) -> Result<(), &'static str> {
        let end_paddr = paddr
            .checked_add(data.len())
            .ok_or("Physical address overflow")?;
        if end_paddr > self.ram.len() {
            return Err("Physical memory access out of bounds");
        }
        self.ram[paddr..end_paddr].copy_from_slice(data);
        Ok(())
    }

    /// 物理内存读取
    pub fn read_physical(&self, paddr: usize, buf: &mut [u8]) -> Result<(), &'static str> {
        let end_paddr = paddr
            .checked_add(buf.len())
            .ok_or("Physical address overflow")?;
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
        let _ = vaddr
            .checked_add(data.len())
            .ok_or("Virtual address overflow")?;
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
        self.write_virtual_checked(space, vaddr, data, false)
            .map(|_| ())
    }

    /// 跨页安全的虚拟内存读取：逐页检查与读取
    pub fn read_virtual_checked(
        &mut self,
        space: &mut AddressSpace,
        vaddr: usize,
        buf: &mut [u8],
        is_user: bool,
    ) -> Result<usize, &'static str> {
        let _ = vaddr
            .checked_add(buf.len())
            .ok_or("Virtual address overflow")?;
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
        self.read_virtual_checked(space, vaddr, buf, false)
            .map(|_| ())
    }
}
