//! 内存管理子系统 (Memory Management Subsystem)

pub mod buddy;
pub mod page_table;
pub mod slab;
pub mod tlb;

pub use buddy::{BuddyAllocator, BuddyStats, MAX_ORDER, PAGE_SIZE};
pub use page_table::{
    AccessType, AddressSpace, FLAG_COW, FLAG_DEMAND, FLAG_DIRTY, FLAG_PRESENT, FLAG_USER,
    FLAG_WRITABLE,
};
pub use slab::SlabAllocator;
pub use tlb::{Tlb, TlbEntry};

pub struct MemoryManager {
    pub buddy: BuddyAllocator,
    pub slab: SlabAllocator,
    pub kernel_space: AddressSpace,
    pub page_ref_count: Vec<u16>,
    ram: Vec<u8>, // 通过受检查的物理内存接口访问，禁止替换缓冲区
}

impl MemoryManager {
    pub fn new(total_pages: usize) -> Self {
        let ram_bytes = total_pages * PAGE_SIZE;
        Self {
            buddy: BuddyAllocator::new(total_pages),
            slab: SlabAllocator::new(),
            kernel_space: AddressSpace::new(64),
            page_ref_count: vec![0u16; total_pages],
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
            let count = 1 << order;
            for i in 0..count {
                if let Some(r) = self.page_ref_count.get_mut(pfn + i) {
                    *r = 1;
                }
            }
            Ok(pfn)
        } else {
            self.buddy.free_pages(pfn)?;
            Err("Physical memory access out of bounds")
        }
    }

    pub fn get_page_ref(&self, pfn: usize) -> u16 {
        self.page_ref_count.get(pfn).copied().unwrap_or(0)
    }

    pub fn inc_page_ref(&mut self, pfn: usize) {
        if let Some(ref_count) = self.page_ref_count.get_mut(pfn) {
            *ref_count = ref_count.saturating_add(1);
        }
    }

    pub fn dec_page_ref(&mut self, pfn: usize) -> Result<u16, &'static str> {
        if let Some(ref_count) = self.page_ref_count.get_mut(pfn) {
            if *ref_count > 0 {
                *ref_count -= 1;
                Ok(*ref_count)
            } else {
                Err("Page reference count underflow")
            }
        } else {
            Err("Invalid physical page number")
        }
    }

    pub fn free_page_ref_counted(&mut self, pfn: usize) -> Result<(), &'static str> {
        let remaining = self.dec_page_ref(pfn)?;
        if remaining == 0 {
            self.buddy.free_pages(pfn)?;
        }
        Ok(())
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
        if data.is_empty() {
            return Ok(0);
        }
        let end = vaddr
            .checked_add(data.len())
            .ok_or("Virtual address overflow")?;
        if end as u64 > 0x1_0000_0000 {
            return Err("Virtual address out of 32-bit bounds");
        }

        // Validate the entire destination before touching bytes or materializing pages.
        let mut plans = Vec::new();
        for page_va in ((vaddr / PAGE_SIZE * PAGE_SIZE)..end).step_by(PAGE_SIZE) {
            let entry = *space.page_directory.get_entry_mut(page_va)
                .ok_or("Page fault: virtual address not mapped")?;
            if !entry.is_present() && !entry.is_demand() {
                return Err("Page fault: virtual address not mapped");
            }
            if is_user && !entry.is_user() {
                return Err("Page fault: user access to supervisor page denied");
            }
            if !entry.is_writable() && !(entry.is_present() && entry.is_cow()) {
                return Err("Page fault: write permission denied (read-only page)");
            }
            if entry.is_present() {
                let page_end = entry.pfn.checked_add(1).and_then(|pfn| pfn.checked_mul(PAGE_SIZE))
                    .ok_or("Physical address overflow")?;
                if page_end > self.ram.len() {
                    return Err("Physical memory access out of bounds");
                }
                if entry.is_cow() && self.get_page_ref(entry.pfn) == 0 {
                    return Err("Invalid COW page reference count");
                }
            }
            plans.push((page_va, entry));
        }

        // Reserve every Demand/COW page before publishing any mapping. Allocation failure
        // leaves both the old mappings and all destination bytes intact.
        let mut staged = Vec::new();
        let reservation = (|| -> Result<(), &'static str> {
            for &(page_va, entry) in &plans {
                if !entry.is_present() || (entry.is_cow() && self.get_page_ref(entry.pfn) > 1) {
                    let pfn = self.allocate_pages_zeroed(0)?;
                    staged.push((page_va, entry, pfn));
                    if entry.is_present() {
                        self.copy_page(entry.pfn, pfn)?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = reservation {
            for &(_, _, pfn) in &staged {
                self.free_page_ref_counted(pfn)?;
            }
            return Err(error);
        }
        for &(page_va, entry, pfn) in &staged {
            let flags = (entry.flags & !(FLAG_COW | FLAG_DEMAND)) | FLAG_PRESENT | FLAG_WRITABLE;
            if entry.is_present() {
                self.free_page_ref_counted(entry.pfn)?;
            }
            space.page_directory.map_page(page_va, pfn, flags);
            space.tlb.invalidate(page_va / PAGE_SIZE);
            space.page_faults += 1;
        }
        for &(page_va, entry) in &plans {
            if entry.is_present() && entry.is_cow() && self.get_page_ref(entry.pfn) == 1
                && space.page_directory.get_entry_mut(page_va).is_some_and(|e| e.is_cow())
            {
                let mapped = space.page_directory.get_entry_mut(page_va).unwrap();
                mapped.flags = (mapped.flags & !FLAG_COW) | FLAG_WRITABLE;
                space.tlb.invalidate(page_va / PAGE_SIZE);
                space.page_faults += 1;
            }
        }
        let mut bytes_written = 0;

        while bytes_written < data.len() {
            let curr_va = vaddr + bytes_written;
            let page_offset = curr_va % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_offset).min(data.len() - bytes_written);

            let paddr = match space.translate_checked(curr_va, AccessType::Write, is_user) {
                Ok(pa) => pa,
                Err(err) => {
                    if space.handle_page_fault(self, curr_va, AccessType::Write, is_user)? {
                        space.translate_checked(curr_va, AccessType::Write, is_user)?
                    } else {
                        return Err(err);
                    }
                }
            };
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
        let end = vaddr
            .checked_add(buf.len())
            .ok_or("Virtual address overflow")?;
        if end as u64 > 0x1_0000_0000 {
            return Err("Virtual address out of 32-bit bounds");
        }
        let mut bytes_read = 0;

        while bytes_read < buf.len() {
            let curr_va = vaddr + bytes_read;
            let page_offset = curr_va % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_offset).min(buf.len() - bytes_read);

            let paddr = match space.translate_checked(curr_va, AccessType::Read, is_user) {
                Ok(pa) => pa,
                Err(err) => {
                    if space.handle_page_fault(self, curr_va, AccessType::Read, is_user)? {
                        space.translate_checked(curr_va, AccessType::Read, is_user)?
                    } else {
                        return Err(err);
                    }
                }
            };
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
