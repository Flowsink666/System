//! 多级页表与虚拟内存管理 (Multi-Level Page Tables & Virtual Memory)
//!
//! 高性能与高可靠特性：
//! 1. 两级树形页表，重映射时自动释放旧物理页，杜绝物理页泄漏
//! 2. 结合 TLB 快表实现单周期地址转换与精确权限校验 (读/写/用户态)
//! 3. 完整的地址空间销毁回收机制 (destroy)，彻底释放进程物理内存
//! 4. 支持进程 Fork 时的页表与内存页完整深拷贝

use super::buddy::PAGE_SIZE;
use super::tlb::{Tlb, TlbEntry};

pub const FLAG_PRESENT: u8 = 1 << 0;
pub const FLAG_WRITABLE: u8 = 1 << 1;
pub const FLAG_USER: u8 = 1 << 2;
pub const FLAG_ACCESSED: u8 = 1 << 3;
pub const FLAG_DIRTY: u8 = 1 << 4;
pub const FLAG_COW: u8 = 1 << 5;
pub const FLAG_DEMAND: u8 = 1 << 6;

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

    #[inline]
    pub fn is_cow(&self) -> bool {
        (self.flags & FLAG_COW) != 0
    }

    #[inline]
    pub fn is_demand(&self) -> bool {
        (self.flags & FLAG_DEMAND) != 0
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
            let pfn = entry.is_present().then_some(entry.pfn);
            *entry = PageTableEntry::default();
            return pfn;
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

    /// 获取虚拟地址对应的可变页表项
    pub fn get_entry_mut(&mut self, va: usize) -> Option<&mut PageTableEntry> {
        if va > 0xFFFF_FFFF {
            return None;
        }
        let pd_idx = (va >> 22) & 0x3FF;
        let pt_idx = (va >> 12) & 0x3FF;
        let table = self.tables[pd_idx].as_mut()?;
        Some(&mut table.entries[pt_idx])
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
            mm.free_page_ref_counted(old_pfn)?;
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
        mm: &mut super::MemoryManager,
        va: usize,
    ) -> Result<(), &'static str> {
        if va > 0xFFFF_FFFF {
            return Err("Virtual address out of 32-bit bounds");
        }

        let vpn = va / PAGE_SIZE;
        self.tlb.invalidate(vpn);
        if let Some(pfn) = self.page_directory.unmap_page(va) {
            mm.free_page_ref_counted(pfn)?;
        }
        Ok(())
    }

    /// 完整销毁整个地址空间并回收所有物理页框 (进程退出时调用)
    pub fn destroy(&mut self, mm: &mut super::MemoryManager) {
        self.destroy_with_mm(mm);
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
                                new_space.destroy(mm);
                                return Err(e);
                            }
                        };
                        new_space
                            .page_directory
                            .map_page(va, child_pfn, if entry.is_cow() {
                                (entry.flags & !FLAG_COW) | FLAG_WRITABLE
                            } else { entry.flags });

                        if let Err(e) = mm.copy_page(parent_pfn, child_pfn) {
                            new_space.destroy(mm);
                            return Err(e);
                        }
                    } else if entry.is_demand() {
                        let va = (pd_idx << 22) | (pt_idx << 12);
                        new_space.map_demand_zero(va, entry.flags)?;
                    }
                }
            }
        }

        Ok(new_space)
    }

    /// 注册按需调页虚拟内存区 (Demand Zero Paging)
    /// 仅预留页表项与权限，推迟物理页分配直至首次访问触发缺页中断
    pub fn map_demand_zero(&mut self, va: usize, flags: u8) -> Result<(), &'static str> {
        if va > 0xFFFF_FFFF {
            return Err("Virtual address out of 32-bit bounds");
        }
        let pd_idx = (va >> 22) & 0x3FF;
        let pt_idx = (va >> 12) & 0x3FF;
        if self.page_directory.tables[pd_idx].is_none() {
            self.page_directory.tables[pd_idx] = Some(Box::new(PageTableLevel2::new()));
        }
        let table = self.page_directory.tables[pd_idx].as_mut().unwrap();
        if table.entries[pt_idx].is_present() || table.entries[pt_idx].is_demand() {
            return Err("Virtual page already mapped");
        }
        table.entries[pt_idx] = PageTableEntry {
            pfn: 0,
            flags: flags | FLAG_DEMAND, // 不含 FLAG_PRESENT
        };
        let vpn = va / PAGE_SIZE;
        self.tlb.invalidate(vpn);
        Ok(())
    }

    /// 写时复制克隆地址空间 (Copy-On-Write Fork)
    /// 父子进程共享已有物理页框，所有可写页清除写权限并标记 FLAG_COW
    pub fn fork_cow(&mut self, mm: &mut super::MemoryManager) -> Result<AddressSpace, &'static str> {
        let mut child_space = AddressSpace::new(self.tlb.capacity);

        for (pd_idx, table_opt) in self.page_directory.tables.iter_mut().enumerate() {
            if let Some(table) = table_opt {
                for (pt_idx, entry) in table.entries.iter_mut().enumerate() {
                    let va = (pd_idx << 22) | (pt_idx << 12);
                    let vpn = va / PAGE_SIZE;

                    if entry.is_present() {
                        let child_flags = if entry.is_writable() {
                            // 父进程与子进程均清除写权限，标记为 COW
                            entry.flags = (entry.flags & !FLAG_WRITABLE) | FLAG_COW;
                            self.tlb.invalidate(vpn);
                            entry.flags
                        } else {
                            entry.flags
                        };

                        // 递增物理页的共享引用计数
                        mm.inc_page_ref(entry.pfn);

                        // 映射到子进程页表
                        child_space.page_directory.map_page(va, entry.pfn, child_flags);
                    } else if entry.is_demand() {
                        // 继承按需调页配置
                        if child_space.page_directory.tables[pd_idx].is_none() {
                            child_space.page_directory.tables[pd_idx] =
                                Some(Box::new(PageTableLevel2::new()));
                        }
                        let ctable = child_space.page_directory.tables[pd_idx].as_mut().unwrap();
                        ctable.entries[pt_idx] = *entry;
                    }
                }
            }
        }

        Ok(child_space)
    }

    /// 缺页异常处理机制 (Page Fault Handler)
    /// 返回 Ok(true) 表示成功捕获并由缺页中断例程解决（如 COW 写时拷贝、Demand 按需加载）
    /// 返回 Ok(false) 表示此异常不是由内核缺页例程管理的合法缺页
    pub fn handle_page_fault(
        &mut self,
        mm: &mut super::MemoryManager,
        va: usize,
        access: AccessType,
        is_user: bool,
    ) -> Result<bool, &'static str> {
        if va > 0xFFFF_FFFF {
            return Ok(false);
        }

        let pd_idx = (va >> 22) & 0x3FF;
        let pt_idx = (va >> 12) & 0x3FF;
        let vpn = va / PAGE_SIZE;

        let table = match self.page_directory.tables[pd_idx].as_mut() {
            Some(t) => t,
            None => return Ok(false),
        };
        let entry = &mut table.entries[pt_idx];

        // 检查用户态访问权限
        if is_user && !entry.is_user() {
            return Ok(false);
        }

        // 场景 1：按需调页 (Demand Zero Paging) - 页面尚未分配物理页 (未 Present，标记为 Demand)
        if entry.is_demand() && !entry.is_present() {
            if access == AccessType::Write && !entry.is_writable() {
                return Ok(false);
            }
            let new_pfn = mm.allocate_pages_zeroed(0)?;
            let flags = (entry.flags & !FLAG_DEMAND) | FLAG_PRESENT;
            entry.pfn = new_pfn;
            entry.flags = flags;
            self.tlb.invalidate(vpn);
            self.tlb.insert(TlbEntry {
                vpn,
                pfn: new_pfn,
                flags,
            });
            self.page_faults += 1;
            return Ok(true);
        }

        // 场景 2：写时复制 (Copy-on-Write) - 写操作访问标记为 COW 的有效页
        if access == AccessType::Write && entry.is_present() && entry.is_cow() {
            let old_pfn = entry.pfn;
            let ref_count = mm.get_page_ref(old_pfn);

            if ref_count > 1 {
                // 存在多个进程共享此物理页：分配新物理页并拷贝数据
                let new_pfn = mm.allocate_pages_zeroed(0)?;
                if let Err(error) = mm.copy_page(old_pfn, new_pfn) {
                    mm.free_page_ref_counted(new_pfn)?;
                    return Err(error);
                }
                mm.dec_page_ref(old_pfn)?;

                let updated_flags = (entry.flags & !FLAG_COW) | FLAG_WRITABLE;
                entry.pfn = new_pfn;
                entry.flags = updated_flags;

                self.tlb.invalidate(vpn);
                self.tlb.insert(TlbEntry {
                    vpn,
                    pfn: new_pfn,
                    flags: updated_flags,
                });
            } else {
                // 仅剩当前进程独占该物理页：无需物理拷贝，直接就地升级为可写
                let updated_flags = (entry.flags & !FLAG_COW) | FLAG_WRITABLE;
                entry.flags = updated_flags;

                self.tlb.invalidate(vpn);
                self.tlb.insert(TlbEntry {
                    vpn,
                    pfn: old_pfn,
                    flags: updated_flags,
                });
            }

            self.page_faults += 1;
            return Ok(true);
        }

        Ok(false)
    }

    /// 配合物理页引用计数的地址空间回收 (释放或递减物理页)
    pub fn destroy_with_mm(&mut self, mm: &mut super::MemoryManager) {
        self.tlb.flush();
        for pd_entry in self.page_directory.tables.iter_mut() {
            if let Some(table) = pd_entry.take() {
                for entry in table.entries.iter() {
                    if entry.is_present() {
                        let _ = mm.free_page_ref_counted(entry.pfn);
                    }
                }
            }
        }
    }
}
