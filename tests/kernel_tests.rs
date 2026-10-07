use mini_os_kernel::fs::{self, VirtualFileSystem};
use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::mm::{
    AddressSpace, BuddyAllocator, MemoryManager, SlabAllocator, FLAG_PRESENT, FLAG_WRITABLE,
    PAGE_SIZE,
};
use mini_os_kernel::sched::{CfsScheduler, ProcessManager, ProcessState};
use mini_os_kernel::syscall::{SyscallArgs, SYS_GETPID, SYS_KFREE, SYS_KMALLOC};

#[test]
fn test_buddy_allocator_basic_and_coalesce() {
    let mut buddy = BuddyAllocator::new(1024);

    // 分配 1 页 (order 0) 与 4 页 (order 2)
    let pfn0 = buddy.allocate_pages(0).expect("allocate order 0");
    let pfn2 = buddy.allocate_pages(2).expect("allocate order 2");

    assert_eq!(buddy.stats.allocated_pages, 1 + 4);
    assert_eq!(buddy.stats.free_pages, 1024 - 5);

    // 释放并验证伙伴合并
    buddy.free_pages(pfn0).expect("free pfn0");
    buddy.free_pages(pfn2).expect("free pfn2");

    assert_eq!(buddy.stats.allocated_pages, 0);
    assert_eq!(buddy.stats.free_pages, 1024);
    assert!(buddy.stats.merges_count > 0);
}

#[test]
fn test_slab_allocator_reuse() {
    let mut buddy = BuddyAllocator::new(1024);
    let mut slab = SlabAllocator::new();

    // 分配 50 个 64 字节对象
    let mut handles = Vec::new();
    for _ in 0..50 {
        let h = slab.kmalloc(&mut buddy, 64).expect("slab alloc");
        handles.push(h);
    }

    // 释放全部
    for h in handles {
        slab.kfree(&mut buddy, h).expect("slab free");
    }

    // 再次分配，验证槽位重复利用
    let h_new = slab.kmalloc(&mut buddy, 64).expect("slab reuse");
    slab.kfree(&mut buddy, h_new).expect("slab free");
}

#[test]
fn test_page_table_and_tlb() {
    let mut buddy = BuddyAllocator::new(1024);
    let mut space = AddressSpace::new(16);

    let va = 0x0040_1000;
    let pfn = space.allocate_and_map(&mut buddy, va, 0x3).expect("map page");

    // 清空 TLB 以测试多级页表遍历 (Page Table Walk) 与 TLB 填充
    space.tlb.flush();
    assert_eq!(space.tlb.hits, 0);
    assert_eq!(space.tlb.misses, 0);

    // 首次转换，TLB 未命中，走多级页表并回填 TLB
    let pa1 = space.translate(va).expect("translate va");
    assert_eq!(pa1, pfn * PAGE_SIZE);
    assert_eq!(space.tlb.misses, 1);
    assert_eq!(space.tlb.hits, 0);

    // 第二次转换，TLB 极速命中
    let pa2 = space.translate(va).expect("translate va from tlb");
    assert_eq!(pa2, pa1);
    assert_eq!(space.tlb.hits, 1);

    // 解除映射
    space.unmap_and_free(&mut buddy, va).expect("unmap");
    assert!(space.translate(va).is_err());
}

#[test]
fn test_cfs_scheduler_weights_and_order() {
    let mut cfs = CfsScheduler::new(20, 2, 1000);

    // 两个任务：PID 10 (nice -10, weight 9548), PID 20 (nice 10, weight 110)
    cfs.enqueue_task(10, 0, 9548);
    cfs.enqueue_task(20, 0, 110);

    // 首个被选中的任务
    let first = cfs.pick_next_task().expect("pick task");
    assert_eq!(first, 10);
}

#[test]
fn test_process_manager_multitasking() {
    let mut pm = ProcessManager::new(1000);

    let p1 = pm.spawn("task1", 0, 10);
    let p2 = pm.spawn("task2", 0, 10);

    let mut step_count = 0;
    while pm.processes.values().any(|p| p.state != ProcessState::Terminated) {
        let _ = pm.schedule_step(2);
        step_count += 1;
        if step_count > 100 {
            break;
        }
    }

    assert_eq!(pm.processes.get(&p1).unwrap().state, ProcessState::Terminated);
    assert_eq!(pm.processes.get(&p2).unwrap().state, ProcessState::Terminated);
}

#[test]
fn test_filesystem_hierarchy_and_cache() {
    let mut vfs = VirtualFileSystem::new(512, 16);

    // 创建层级目录
    vfs.mkdir("/var").expect("mkdir /var");
    vfs.mkdir("/var/log").expect("mkdir /var/log");

    // 创建文件并写入
    let fd = vfs.open("/var/log/syslog.log", fs::O_CREAT | fs::O_RDWR).expect("create file");
    let test_data = b"Kernel boot: initial check passed OK.\n";
    let written = vfs.write(fd, test_data).expect("write file");
    assert_eq!(written, test_data.len());
    vfs.close(fd).expect("close fd");

    // 重新打开并读取
    let fd_read = vfs.open("/var/log/syslog.log", fs::O_RDONLY).expect("open file");
    let mut read_buf = vec![0u8; 128];
    let read_bytes = vfs.read(fd_read, &mut read_buf).expect("read file");
    assert_eq!(&read_buf[..read_bytes], test_data);
    vfs.close(fd_read).expect("close fd");

    // 验证 Buffer Cache 命中
    assert!(vfs.cache.hits > 0);
}

#[test]
fn test_syscall_dispatch() {
    let mut kernel = Kernel::new(1024, 512, 16);

    // 测试 SYS_GETPID
    let args_pid = SyscallArgs { num: SYS_GETPID, arg0: 0, arg1: 0, arg2: 0, arg3: 0 };
    let res_pid = kernel.syscall(args_pid).expect("syscall getpid");
    assert!(res_pid >= 0);

    // 测试 SYS_KMALLOC & SYS_KFREE
    let args_alloc = SyscallArgs { num: SYS_KMALLOC, arg0: 128, arg1: 0, arg2: 0, arg3: 0 };
    let alloc_handle = kernel.syscall(args_alloc).expect("syscall kmalloc");
    assert!(alloc_handle > 0);

    let args_free = SyscallArgs { num: SYS_KFREE, arg0: alloc_handle as usize, arg1: 0, arg2: 0, arg3: 0 };
    let res_free = kernel.syscall(args_free).expect("syscall kfree");
    assert_eq!(res_free, 0);
}

#[test]
fn test_indirect_blocks_and_disk_reclamation() {
    let mut vfs = VirtualFileSystem::new(512, 32);
    let initial_free = vfs.free_blocks.len();

    // 写入跨越直接块 (12块 = 6KB) 的大文件 (20块 = 10,240 字节)
    let large_size = 20 * 512;
    let data: Vec<u8> = (0..large_size).map(|i| (i % 251) as u8).collect();

    let fd = vfs.open("/large_file.bin", fs::O_CREAT | fs::O_RDWR).expect("create large file");
    let written = vfs.write(fd, &data).expect("write large file");
    assert_eq!(written, large_size);
    vfs.close(fd).expect("close fd");

    // 确认消耗了 20 个数据块 + 1 个间接索引块 = 21 块
    assert_eq!(vfs.free_blocks.len(), initial_free - 21);

    // 读取并验证完整性
    let fd_read = vfs.open("/large_file.bin", fs::O_RDONLY).expect("open large file");
    let mut read_buf = vec![0u8; large_size];
    let n = vfs.read(fd_read, &mut read_buf).expect("read large file");
    assert_eq!(n, large_size);
    assert_eq!(read_buf, data);
    vfs.close(fd_read).expect("close fd");

    // 删除大文件，验证所有 21 个块被 100% 完整回收，零泄露
    vfs.unlink("/large_file.bin").expect("unlink large file");
    assert_eq!(vfs.free_blocks.len(), initial_free);
}

#[test]
fn test_cfs_no_weight_leak() {
    let mut cfs = CfsScheduler::new(20, 2, 1000);

    // 多次入队与调度出队
    cfs.enqueue_task(1, 0, 1024);
    cfs.enqueue_task(2, 0, 2048);
    assert_eq!(cfs.total_weight, 1024 + 2048);

    let next1 = cfs.pick_next_task().expect("pick");
    assert_eq!(next1, 1);
    // 出队后总权重必须扣除
    assert_eq!(cfs.total_weight, 2048);

    let next2 = cfs.pick_next_task().expect("pick");
    assert_eq!(next2, 2);
    // 全部出队后总权重必须归零，无泄漏
    assert_eq!(cfs.total_weight, 0);
}

#[test]
fn test_cross_page_virtual_memory_access_and_permissions() {
    let mut mm = MemoryManager::new(1024);
    let mut space = AddressSpace::new(16);

    // 映射两个相邻虚拟页 (0x1000 与 0x2000)，但分配两个不同的物理页
    let pfn1 = space.allocate_and_map(&mut mm.buddy, 0x1000, FLAG_PRESENT | FLAG_WRITABLE).unwrap();
    let pfn2 = space.allocate_and_map(&mut mm.buddy, 0x2000, FLAG_PRESENT | FLAG_WRITABLE).unwrap();

    // 跨页写入：从 0x1FFE 开始写 6 个字节 (最后两字节落在 0x1000 页，后 4 字节落在 0x2000 页)
    let payload = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    let written = mm.write_virtual_checked(&mut space, 0x1FFE, &payload, false).unwrap();
    assert_eq!(written, 6);

    // 验证物理 RAM 中对应位置确实被精准写入
    assert_eq!(&mm.ram[pfn1 * PAGE_SIZE + 4094..pfn1 * PAGE_SIZE + 4096], &[0xAA, 0xBB]);
    assert_eq!(&mm.ram[pfn2 * PAGE_SIZE..pfn2 * PAGE_SIZE + 4], &[0xCC, 0xDD, 0xEE, 0xFF]);

    // 跨页读取验证
    let mut read_buf = [0u8; 6];
    let read_n = mm.read_virtual_checked(&mut space, 0x1FFE, &mut read_buf, false).unwrap();
    assert_eq!(read_n, 6);
    assert_eq!(read_buf, payload);

    // 测试只读页权限拦截
    let _ = space.allocate_and_map(&mut mm.buddy, 0x3000, FLAG_PRESENT).unwrap(); // 无 FLAG_WRITABLE
    let write_res = mm.write_virtual_checked(&mut space, 0x3000, &[1, 2, 3], false);
    assert!(write_res.is_err()); // 必须返回权限拒绝错误
}

#[test]
fn test_remapping_and_address_space_destroy() {
    let mut mm = MemoryManager::new(1024);
    let mut space = AddressSpace::new(16);
    let initial_free = mm.buddy.stats.free_pages;

    // 分配并映射 0x4000
    space.allocate_and_map(&mut mm.buddy, 0x4000, FLAG_PRESENT | FLAG_WRITABLE).unwrap();
    assert_eq!(mm.buddy.stats.free_pages, initial_free - 1);

    // 重复映射同一虚拟页 0x4000：旧物理页必须被自动回收，不发生泄漏
    space.allocate_and_map(&mut mm.buddy, 0x4000, FLAG_PRESENT | FLAG_WRITABLE).unwrap();
    assert_eq!(mm.buddy.stats.free_pages, initial_free - 1);

    // 销毁地址空间：所有物理页 100% 归还
    space.destroy(&mut mm.buddy);
    assert_eq!(mm.buddy.stats.free_pages, initial_free);
}

#[test]
fn test_vfs_unlink_non_empty_directory_atomicity() {
    let mut vfs = VirtualFileSystem::new(512, 16);

    vfs.mkdir("/dir").unwrap();
    let fd = vfs.open("/dir/file.txt", fs::O_CREAT | fs::O_RDWR).unwrap();
    vfs.close(fd).unwrap();

    // 尝试删除非空目录，预期失败
    let res = vfs.unlink("/dir");
    assert!(res.is_err());

    // 校验原子性：失败后 /dir 和 /dir/file.txt 必须完整保留，绝不变成孤儿
    let stat_dir = vfs.stat("/dir");
    assert!(stat_dir.is_ok());
    let stat_file = vfs.stat("/dir/file.txt");
    assert!(stat_file.is_ok());
}

#[test]
fn test_vfs_file_permissions_and_append() {
    let mut vfs = VirtualFileSystem::new(512, 16);

    let fd_write = vfs.open("/test_mode.txt", fs::O_CREAT | fs::O_WRONLY).unwrap();
    vfs.write(fd_write, b"initial ").unwrap();
    vfs.close(fd_write).unwrap();

    // 以只读模式打开，尝试写入必须报错
    let fd_ro = vfs.open("/test_mode.txt", fs::O_RDONLY).unwrap();
    let write_err = vfs.write(fd_ro, b"bad write");
    assert!(write_err.is_err());
    vfs.close(fd_ro).unwrap();

    // 以追加模式打开写入，自动动态移动到末尾
    let fd_app = vfs.open("/test_mode.txt", fs::O_WRONLY | fs::O_APPEND).unwrap();
    vfs.write(fd_app, b"appended").unwrap();
    vfs.close(fd_app).unwrap();

    let fd_read = vfs.open("/test_mode.txt", fs::O_RDONLY).unwrap();
    let mut buf = vec![0u8; 64];
    let n = vfs.read(fd_read, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"initial appended");
    vfs.close(fd_read).unwrap();
}

#[test]
fn test_fork_memory_isolation() {
    let mut kernel = Kernel::new(1024, 512, 16);

    // 获取 init 进程 (PID 1)
    let parent_pid = 1;
    // 写入父进程虚拟内存 0x8000
    kernel.mm.write_virtual(&mut kernel.pm.processes.get_mut(&parent_pid).unwrap().address_space, 0x8000, b"PARENT").unwrap();

    // Fork 子进程
    let child_pid = kernel.fork_process(parent_pid).unwrap();

    // 验证子进程初始读取到父进程数据
    let mut child_buf = [0u8; 6];
    kernel.mm.read_virtual(&mut kernel.pm.processes.get_mut(&child_pid).unwrap().address_space, 0x8000, &mut child_buf).unwrap();
    assert_eq!(&child_buf, b"PARENT");

    // 修改子进程虚拟内存
    kernel.mm.write_virtual(&mut kernel.pm.processes.get_mut(&child_pid).unwrap().address_space, 0x8000, b"CHILD!").unwrap();

    // 验证父进程内存保持不变（完全隔离）
    let mut parent_buf = [0u8; 6];
    kernel.mm.read_virtual(&mut kernel.pm.processes.get_mut(&parent_pid).unwrap().address_space, 0x8000, &mut parent_buf).unwrap();
    assert_eq!(&parent_buf, b"PARENT");
}

#[test]
fn test_tlb_invalidation_and_slot_reuse() {
    let mut tlb = mini_os_kernel::mm::Tlb::new(4);

    tlb.insert(mini_os_kernel::mm::TlbEntry { vpn: 1, pfn: 10, flags: 1 });
    tlb.insert(mini_os_kernel::mm::TlbEntry { vpn: 2, pfn: 20, flags: 1 });
    tlb.insert(mini_os_kernel::mm::TlbEntry { vpn: 3, pfn: 30, flags: 1 });

    // 使条目 1 失效
    tlb.invalidate(1);

    // 插入新条目 4，必须复用空出的槽位，不得破坏条目 2 和 3
    tlb.insert(mini_os_kernel::mm::TlbEntry { vpn: 4, pfn: 40, flags: 1 });

    assert!(tlb.lookup(2).is_some());
    assert!(tlb.lookup(3).is_some());
    assert!(tlb.lookup(4).is_some());
    assert!(tlb.lookup(1).is_none());
}
