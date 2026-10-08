//! 写时复制 (COW Fork) 与按需缺页异常处理专项测试
//! (Copy-on-Write & Demand Paging Verification Suite)

use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::mm::{FLAG_COW, FLAG_USER, FLAG_WRITABLE};

#[test]
fn test_cow_fork_shares_physical_pages_initially() {
    let mut kernel = Kernel::new(32, 64, 8);
    let parent_pid = 1;

    // 父进程分配并写入 0x8000 页面
    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    parent_proc
        .address_space
        .allocate_and_map(&mut kernel.mm, 0x8000, FLAG_USER | FLAG_WRITABLE)
        .expect("allocate 0x8000 failed");
    kernel
        .mm
        .write_virtual(&mut parent_proc.address_space, 0x8000, b"shared_data")
        .expect("write failed");

    let (parent_pfn, parent_flags) = parent_proc.address_space.page_directory.walk(0x8000).unwrap();
    assert_eq!(parent_flags & FLAG_WRITABLE, FLAG_WRITABLE);
    assert_eq!(kernel.mm.get_page_ref(parent_pfn), 1);

    let free_pages_before = kernel.mm.buddy.stats.free_pages;

    // 执行 COW Fork
    let child_pid = kernel.fork_process_cow(parent_pid).expect("fork_cow failed");

    // COW Fork 不消耗新的物理页框
    let free_pages_after = kernel.mm.buddy.stats.free_pages;
    assert_eq!(
        free_pages_before, free_pages_after,
        "COW Fork must not allocate new physical pages immediately"
    );

    // 检查父进程该页：写权限被清除，打上 FLAG_COW
    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    let (_, parent_new_flags) = parent_proc.address_space.page_directory.walk(0x8000).unwrap();
    assert_eq!(parent_new_flags & FLAG_WRITABLE, 0);
    assert_eq!(parent_new_flags & FLAG_COW, FLAG_COW);

    // 检查子进程该页：映射到相同的物理页框，打上 FLAG_COW
    let child_proc = kernel.pm.processes.get_mut(&child_pid).unwrap();
    let (child_pfn, child_flags) = child_proc.address_space.page_directory.walk(0x8000).unwrap();
    assert_eq!(child_pfn, parent_pfn);
    assert_eq!(child_flags & FLAG_WRITABLE, 0);
    assert_eq!(child_flags & FLAG_COW, FLAG_COW);

    // 物理页引用计数递增为 2
    assert_eq!(kernel.mm.get_page_ref(parent_pfn), 2);

    // 父子进程读取该页数据均一致
    let mut buf = [0u8; 11];
    kernel
        .mm
        .read_virtual(&mut child_proc.address_space, 0x8000, &mut buf)
        .expect("child read failed");
    assert_eq!(&buf, b"shared_data");
}

#[test]
fn test_cow_write_triggers_page_fault_and_splits_pages() {
    let mut kernel = Kernel::new(32, 64, 8);
    let parent_pid = 1;

    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    parent_proc
        .address_space
        .allocate_and_map(&mut kernel.mm, 0x8000, FLAG_USER | FLAG_WRITABLE)
        .unwrap();
    kernel
        .mm
        .write_virtual(&mut parent_proc.address_space, 0x8000, b"parent_init")
        .unwrap();
    let (parent_pfn, _) = parent_proc.address_space.page_directory.walk(0x8000).unwrap();

    let child_pid = kernel.fork_process_cow(parent_pid).unwrap();

    let free_before_write = kernel.mm.buddy.stats.free_pages;

    // 子进程执行写入，透明触发写时复制 (Page Fault)
    let child_proc = kernel.pm.processes.get_mut(&child_pid).unwrap();
    kernel
        .mm
        .write_virtual(&mut child_proc.address_space, 0x8000, b"child_update")
        .expect("write_virtual with COW must succeed via page fault handler");

    // 物理页按需分裂，分配了 1 个新页
    let free_after_write = kernel.mm.buddy.stats.free_pages;
    assert_eq!(free_before_write - 1, free_after_write);

    // 检查子进程：映射到新物理页，恢复了可写权限，去除了 FLAG_COW
    let child_proc = kernel.pm.processes.get_mut(&child_pid).unwrap();
    let (child_new_pfn, child_new_flags) = child_proc.address_space.page_directory.walk(0x8000).unwrap();
    assert_ne!(child_new_pfn, parent_pfn);
    assert_eq!(child_new_flags & FLAG_WRITABLE, FLAG_WRITABLE);
    assert_eq!(child_new_flags & FLAG_COW, 0);
    assert_eq!(kernel.mm.get_page_ref(child_new_pfn), 1);
    assert_eq!(kernel.mm.get_page_ref(parent_pfn), 1);

    // 数据严格隔离验证
    let mut child_buf = [0u8; 12];
    kernel
        .mm
        .read_virtual(&mut child_proc.address_space, 0x8000, &mut child_buf)
        .unwrap();
    assert_eq!(&child_buf, b"child_update");

    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    let mut parent_buf = [0u8; 11];
    kernel
        .mm
        .read_virtual(&mut parent_proc.address_space, 0x8000, &mut parent_buf)
        .unwrap();
    assert_eq!(&parent_buf, b"parent_init");
}

#[test]
fn test_cow_sole_owner_in_place_upgrade() {
    let mut kernel = Kernel::new(32, 64, 8);
    let parent_pid = 1;

    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    parent_proc
        .address_space
        .allocate_and_map(&mut kernel.mm, 0x8000, FLAG_USER | FLAG_WRITABLE)
        .unwrap();
    kernel
        .mm
        .write_virtual(&mut parent_proc.address_space, 0x8000, b"original")
        .unwrap();
    let (pfn, _) = parent_proc.address_space.page_directory.walk(0x8000).unwrap();

    let child_pid = kernel.fork_process_cow(parent_pid).unwrap();

    // 终止子进程，此时物理页引用计数回到 1
    kernel.terminate_process(child_pid, 0).expect("terminate failed");
    assert_eq!(kernel.mm.get_page_ref(pfn), 1);

    let free_before = kernel.mm.buddy.stats.free_pages;

    // 父进程写入：作为唯一所有者，不应再分配新物理页，而是就地升级为可写
    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    kernel
        .mm
        .write_virtual(&mut parent_proc.address_space, 0x8000, b"upgraded")
        .unwrap();

    let free_after = kernel.mm.buddy.stats.free_pages;
    assert_eq!(free_before, free_after, "Sole owner COW write must not allocate new page");

    let (parent_pfn_after, flags_after) = parent_proc.address_space.page_directory.walk(0x8000).unwrap();
    assert_eq!(parent_pfn_after, pfn);
    assert_eq!(flags_after & FLAG_WRITABLE, FLAG_WRITABLE);
    assert_eq!(flags_after & FLAG_COW, 0);
}

#[test]
fn test_cow_page_reclamation_on_process_exit() {
    let mut kernel = Kernel::new(32, 64, 8);
    let parent_pid = 1;

    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    parent_proc
        .address_space
        .allocate_and_map(&mut kernel.mm, 0x8000, FLAG_USER | FLAG_WRITABLE)
        .unwrap();
    let (pfn, _) = parent_proc.address_space.page_directory.walk(0x8000).unwrap();

    let free_with_page = kernel.mm.buddy.stats.free_pages;

    // Fork 出子进程 1 和 2
    let child1 = kernel.fork_process_cow(parent_pid).unwrap();
    let child2 = kernel.fork_process_cow(parent_pid).unwrap();

    assert_eq!(kernel.mm.get_page_ref(pfn), 3);

    // 终止 child1: 引用变为 2，物理页未归还
    kernel.terminate_process(child1, 0).unwrap();
    assert_eq!(kernel.mm.get_page_ref(pfn), 2);
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_with_page);

    // 终止 child2: 引用变为 1，物理页未归还
    kernel.terminate_process(child2, 0).unwrap();
    assert_eq!(kernel.mm.get_page_ref(pfn), 1);
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_with_page);

    // 终止 parent: 引用归零，物理页正式释放回 Buddy
    kernel.terminate_process(parent_pid, 0).unwrap();
    assert_eq!(kernel.mm.get_page_ref(pfn), 0);
    // 释放了 0x1000 代码页、0x8000 数据页等，空闲页增加
    assert!(kernel.mm.buddy.stats.free_pages > free_with_page);
}

#[test]
fn test_demand_paging_page_fault() {
    let mut kernel = Kernel::new(32, 64, 8);
    let pid = 1;

    let free_before = kernel.mm.buddy.stats.free_pages;

    // 注册按需分配页 (尚未分配物理页框)
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    proc.address_space
        .map_demand_zero(0xA000, FLAG_USER | FLAG_WRITABLE)
        .expect("map_demand_zero failed");

    // 此时尚未消耗 Buddy 物理页框
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_before);

    // 首次读取：触发 Demand Paging 缺页中断，分配并零化物理页
    let mut read_buf = [1u8; 16];
    let n = kernel
        .mm
        .read_virtual_checked(&mut proc.address_space, 0xA000, &mut read_buf, true)
        .expect("read demand page failed");
    assert_eq!(n, 16);
    assert_eq!(read_buf, [0u8; 16], "Demand page must be initialized to zero");

    // 此时已分配 1 个物理页
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_before - 1);

    // 写入新数据
    kernel
        .mm
        .write_virtual_checked(&mut proc.address_space, 0xA000, b"demand_success", true)
        .expect("write to demand page failed");

    let mut verify_buf = [0u8; 14];
    kernel
        .mm
        .read_virtual_checked(&mut proc.address_space, 0xA000, &mut verify_buf, true)
        .unwrap();
    assert_eq!(&verify_buf, b"demand_success");
}
