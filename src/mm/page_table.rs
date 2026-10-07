//! 多级页表与虚拟内存管理 (Multi-Level Page Tables & Virtual Memory)
//!
//! 高性能与高可靠特性：
//! 1. 两级树形页表，重映射时自动释放旧物理页，杜绝物理页泄漏
//! 2. 结合 TLB 快表实现单周期地址转换与精确权限校验 (读/写/用户态)
//! 3. 完整的地址空间销毁回收机制 (destroy)，彻底释放进程物理内存
//! 4. 支持进程 Fork 时的页表与内存页完整深拷贝

use super::buddy::{BuddyAllocator, PAGE_SIZE};
use super::tlb::{Tlb, TlbEntry};

pub const FLAG_PRESENT: u8 = 1 << 0;
pub const FLAG_WRITABLE: u8 = 1 << 1;
pub const FLAG_USER: u8 = 1 << 2;
pub const FLAG_ACCESSED: u8 = 1 << 3;
pub const FLAG_DIRTY: u8 = 1 << 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Execute,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PageTableEntry {
    pub pfn: usize,
    pub flags: u8,
}

impl PageTableEntry {
    #[inline]
    pub fn is_present(&self) -> bool {
        (self.flags & FLAG_PRESENT) != 0
    }

    #[inline]
    pub fn is_writable(&self) -> bool {
        (self.flags & FLAG_WRITABLE) != 0
    }

    #[inline]
    pub fn is_user(&self) -> bool {
        (self.flags & FLAG_USER) != 0
    }
}

/// 二级页表 (1024 项，每项映射 4KB，共覆盖 4MB)
pub struct PageTableLevel2 {
    pub entries: [PageTableEntry; 1024],
}

impl Default for PageTableLevel2 {
    fn default() -> Self {
        Self::new()
    }
}

impl PageTableLevel2 {
    pub fn new() -> Self {
        Self {
            entries: [PageTableEntry::default(); 1024],
        }
    }
}

/// 一级页目录 (Page Directory，1024 项，共覆盖 4GB 虚拟地址空间)
pub struct PageDirectory {
    pub tables: Vec<Option<Box<PageTableLevel2>>>,
}

impl Default for PageDirectory {
    fn default() -> Self {
        Self::new()
    }
}

impl PageDirectory {
    pub fn new() -> Self {
        Self {
            tables: (0..1024).map(|_| None).collect(),
        }
    }

    /// 映射虚拟页到物理页框。如果该页之前已被映射，返回原先的旧 PFN 供调用者释放
    pub fn map_page(&mut self, va: usize, pfn: usize, flags: u8) -> Option<usize> {
        if va > 0xFFFF_FFFF {
            return None;
        }

        let pd_idx = (va >> 22) & 0x3FF;
        let pt_idx = (va >> 12) & 0x3FF;

        if self.tables[pd_idx].is_none() {
            self.tables[pd_idx] = Some(Box::new(PageTableLevel2::new()));
        }

        let table = self.tables[pd_idx].as_mut().unwrap();
        let old_pfn = if table.entries[pt_idx].is_present() {
            Some(table.entries[pt_idx].pfn)
        } else {
            None
        };

        table.entries[pt_idx] = PageTableEntry {
            pfn,
            flags: flags | FLAG_PRESENT,
        };

        old_pfn
    }

    /// 解除虚拟地址映射
    pub fn unmap_page(&mut self, va: usize) -> Option<usize> {
        if va > 0xFFFF_FFFF {
            return None;
        }

        let pd_idx = (va >> 22) & 0x3FF;
        let pt_idx = (va >> 12) & 0x3FF;

        if let Some(table) = self.tables[pd_idx].as_mut() {
            let entry = &mut table.entries[pt_idx];
            if entry.is_present() {
                let pfn = entry.pfn;
                *entry = PageTableEntry::default();
                return Some(pfn);
            }
        }
        None
    }

    /// 遍历多级页表进行纯软件地址转换 (Page Walk)
    pub fn walk(&self, va: usize) -> Option<(usize, u8)> {
        if va > 0xFFFF_FFFF {
            return None;
        }

        let pd_idx = (va >> 22) & 0x3FF;
        let pt_idx = (va >> 12) & 0x3FF;

        let table = self.tables[pd_idx].as_ref()?;
        let entry = &table.entries[pt_idx];
        if entry.is_present() {
            Some((entry.pfn, entry.flags))
        } else {
            None
        }
    }
}

/// 虚拟内存空间 (Address Space)
pub struct AddressSpace {
    pub page_directory: PageDirectory,
    pub tlb: Tlb,
    pub page_faults: u64,
}

impl AddressSpace {
    pub fn new(tlb_capacity: usize) -> Self {
        Self {
            page_directory: PageDirectory::new(),
            tlb: Tlb::new(tlb_capacity),
            page_faults: 0,
        }
    }

    /// 虚拟地址转换并严格校验权限 (读/写/用户态权限)
    pub fn translate_checked(
        &mut self,
        va: usize,
        access: AccessType,
        is_user: bool,
    ) -> Result<usize, &'static str> {
        if va > 0xFFFF_FFFF {
            self.page_faults += 1;
            return Err("Page fault: virtual address out of 32-bit bounds");
        }

        let vpn = va / PAGE_SIZE;
        let offset = va % PAGE_SIZE;

        // 1. TLB 极速查询
        let (pfn, flags) = if let Some(entry) = self.tlb.lookup(vpn) {
            (entry.pfn, entry.flags)
        } else if let Some((pfn, flags)) = self.page_directory.walk(va) {
            self.tlb.insert(TlbEntry { vpn, pfn, flags });
            (pfn, flags)
        } else {
            self.page_faults += 1;
            return Err("Page fault: virtual address not mapped");
        };

        // 2. 检查权限
        if (flags & FLAG_PRESENT) == 0 {
            return Err("Page fault: page not present");
        }

        if access == AccessType::Write && (flags & FLAG_WRITABLE) == 0 {
            return Err("Page fault: write permission denied (read-only page)");
        }

        if is_user && (flags & FLAG_USER) == 0 {
            return Err("Page fault: user access to supervisor page denied");
        }

        Ok((pfn * PAGE_SIZE) + offset)
    }

    /// 兼容接口：通用读取转换
    pub fn translate(&mut self, va: usize) -> Result<usize, &'static str> {
        self.translate_checked(va, AccessType::Read, false)
    }

    /// 按需调页与映射分配。若该虚拟地址已有旧映射，自动将旧物理页归还 Buddy，杜绝泄露
    pub fn allocate_and_map(
        &mut self,
        mm: &mut super::MemoryManager,
        va: usize,
        flags: u8,
    ) -> Result<usize, &'static str> {
        if va > 0xFFFF_FFFF {
            return Err("Virtual address out of 32-bit bounds");
        }

        let effective_flags = flags | FLAG_PRESENT;
        let pfn = mm.allocate_pages_zeroed(0)?;
        if let Some(old_pfn) = self.page_directory.map_page(va, pfn, effective_flags) {
            // 归还被覆盖的旧物理页框
            let _ = mm.buddy.free_pages(old_pfn);
        }
        let vpn = va / PAGE_SIZE;
        self.tlb.insert(TlbEntry {
            vpn,
            pfn,
            flags: effective_flags,
        });
        Ok(pfn)
    }

    /// 解除映射并归还物理页
    pub fn unmap_and_free(
        &mut self,
        buddy: &mut BuddyAllocator,
        va: usize,
    ) -> Result<(), &'static str> {
        if va > 0xFFFF_FFFF {
            return Err("Virtual address out of 32-bit bounds");
        }

        let vpn = va / PAGE_SIZE;
        self.tlb.invalidate(vpn);
        if let Some(pfn) = self.page_directory.unmap_page(va) {
            buddy.free_pages(pfn)?;
        }
        Ok(())
    }

    /// 完整销毁整个地址空间并回收所有物理页框 (进程退出时调用)
    pub fn destroy(&mut self, buddy: &mut BuddyAllocator) {
        self.tlb.flush();
        for pd_entry in self.page_directory.tables.iter_mut() {
            if let Some(table) = pd_entry.take() {
                for entry in table.entries.iter() {
                    if entry.is_present() {
                        let _ = buddy.free_pages(entry.pfn);
                    }
                }
            }
        }
    }

    /// 深度复制地址空间及其物理页数据 (Fork 时调用)
    pub fn fork_clone(&self, mm: &mut super::MemoryManager) -> Result<AddressSpace, &'static str> {
        let mut new_space = AddressSpace::new(self.tlb.capacity);

        for (pd_idx, table_opt) in self.page_directory.tables.iter().enumerate() {
            if let Some(table) = table_opt {
                for (pt_idx, entry) in table.entries.iter().enumerate() {
                    if entry.is_present() {
                        let va = (pd_idx << 22) | (pt_idx << 12);
                        let parent_pfn = entry.pfn;

                        // 为子进程分配新的物理页框
                        let child_pfn = match mm.allocate_pages_zeroed(0) {
                            Ok(pfn) => pfn,
                            Err(e) => {
                                new_space.destroy(&mut mm.buddy);
                                return Err(e);
                            }
                        };
                        new_space
                            .page_directory
                            .map_page(va, child_pfn, entry.flags);

                        if let Err(e) = mm.copy_page(parent_pfn, child_pfn) {
                            new_space.destroy(&mut mm.buddy);
                            return Err(e);
                        }
                    }
                }
            }
        }

        Ok(new_space)
    }
}
