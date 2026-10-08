//! 虚拟内存区域 (VMA) 与 mmap/munmap 及微指令内存访存专项测试
//! (VMA, Memory Mapping & Micro-VM Load/Store Verification Suite)

use mini_os_kernel::arch::{ProgramBuilder, Register};
use mini_os_kernel::fs::{O_CREAT, O_RDWR, O_TRUNC};
use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::syscall::{
    SyscallArgs, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE, SYS_MMAP, SYS_MUNMAP,
};

#[test]
fn test_anonymous_mmap_demand_paging_and_munmap() {
    let mut kernel = Kernel::new(32, 64, 8);
    let pid = 1;

    let free_pages_before = kernel.mm.buddy.stats.free_pages;

    // 申请 2 页 (8192 字节) 匿名映射内存
    let mmap_addr = kernel
        .mmap(pid, 0, 8192, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0)
        .expect("anonymous mmap failed");

    assert!(mmap_addr >= 0x2000_0000);
    assert_eq!(mmap_addr % 4096, 0);

    // 检查 VMA 列表中是否已注册该区域
    let proc = kernel.pm.processes.get(&pid).unwrap();
    let vma = proc.find_vma(mmap_addr).expect("VMA must be registered");
    assert_eq!(vma.start_va, mmap_addr);
    assert_eq!(vma.end_va, mmap_addr + 8192);

    // 此时属于按需调页，尚未真正分配物理页
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages_before);

    // 写入第一个页面，触发按需分配物理页 1
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    kernel
        .mm
        .write_virtual(&mut proc.address_space, mmap_addr, b"page_one_data")
        .expect("write page 1 failed");
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages_before - 1);

    // 写入第二个页面，触发按需分配物理页 2
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    kernel
        .mm
        .write_virtual(&mut proc.address_space, mmap_addr + 4096, b"page_two_data")
        .expect("write page 2 failed");
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages_before - 2);

    // 读取验证
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    let mut buf1 = [0u8; 13];
    let mut buf2 = [0u8; 13];
    kernel
        .mm
        .read_virtual(&mut proc.address_space, mmap_addr, &mut buf1)
        .unwrap();
    kernel
        .mm
        .read_virtual(&mut proc.address_space, mmap_addr + 4096, &mut buf2)
        .unwrap();
    assert_eq!(&buf1, b"page_one_data");
    assert_eq!(&buf2, b"page_two_data");

    // 解除映射 munmap
    kernel.munmap(pid, mmap_addr, 8192).expect("munmap failed");

    // 物理页已完整回收回 Buddy
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages_before);

    // VMA 列表中不再存在该区域
    let proc = kernel.pm.processes.get(&pid).unwrap();
    assert!(proc.find_vma(mmap_addr).is_none());
}

#[test]
fn test_mmap_syscall_dispatch() {
    let mut kernel = Kernel::new(32, 64, 8);

    // 通过系统调用分发 SYS_MMAP
    let ret = kernel
        .syscall(SyscallArgs {
            num: SYS_MMAP,
            arg0: 0,
            arg1: 4096,
            arg2: (PROT_READ | PROT_WRITE) as usize,
            arg3: MAP_ANONYMOUS as usize,
        })
        .expect("sys_mmap failed");

    let mmap_addr = ret as usize;
    assert!(mmap_addr >= 0x2000_0000);

    // 通过系统调用分发 SYS_MUNMAP
    let unmap_ret = kernel
        .syscall(SyscallArgs {
            num: SYS_MUNMAP,
            arg0: mmap_addr,
            arg1: 4096,
            arg2: 0,
            arg3: 0,
        })
        .expect("sys_munmap failed");

    assert_eq!(unmap_ret, 0);
}

#[test]
fn test_micro_vm_load_store_mem_on_mmap() {
    let mut kernel = Kernel::new(32, 64, 8);
    let pid = 1;

    // 1. 构造微指令程序：
    //    MovImm(Rax, 0x3000_0000)
    //    MovImm(Rbx, 0xCAFE_BABE)
    //    StoreMem(Rbx, Rax, 0)    -> 将 0xCAFE_BABE 写入 [Rax + 0] (触发按需缺页并写入)
    //    LoadMem(Rcx, Rax, 0)     -> 将 [Rax + 0] 加载到 Rcx
    let mut builder = ProgramBuilder::new();
    builder.mov_imm(Register::Rax, 0x3000_0000);
    builder.mov_imm(Register::Rbx, 0xCAFE_BABE);
    builder.store_mem(Register::Rbx, Register::Rax, 0);
    builder.load_mem(Register::Rcx, Register::Rax, 0);
    let prog = builder.finish();

    // 2. 先加载程序进进程空间
    kernel
        .load_program_into_process(pid, &prog)
        .expect("load_program failed");

    // 3. 申请 4KB 匿名 mmap 内存 (0x3000_0000)
    let mmap_addr = kernel
        .mmap(pid, 0x3000_0000, 4096, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0)
        .expect("mmap failed");

    // 4. 执行微指令
    kernel.step(5);

    // 5. 验证寄存器中的值已成功被 LoadMem 装载为 0xCAFE_BABE
    let proc = kernel.pm.processes.get(&pid).unwrap();
    assert_eq!(proc.context.get_reg(Register::Rcx), 0xCAFE_BABE);

    // 6. 验证物理内存中确实写入了 0xCAFE_BABE 的小端字节
    let mut mem_check = [0u8; 8];
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&pid).unwrap().address_space,
            mmap_addr,
            &mut mem_check,
        )
        .unwrap();
    assert_eq!(u64::from_le_bytes(mem_check), 0xCAFE_BABE);
}

#[test]
fn test_file_backed_mmap_private() {
    let mut kernel = Kernel::new(32, 64, 8);
    let pid = 1;

    // 在 VFS 中创建测试文件
    let fd = kernel
        .vfs
        .open("/etc/test_config.dat", O_CREAT | O_RDWR | O_TRUNC)
        .expect("open file failed");
    let file_content = b"Kernel mmap file backing test data!";
    kernel.vfs.write(fd, file_content).expect("write failed");

    // 进行文件私有映射
    let mmap_addr = kernel
        .mmap(pid, 0, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, Some(fd), 0)
        .expect("file-backed mmap failed");

    // 校验内存中的预读数据与文件完全一致
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    let mut read_buf = vec![0u8; file_content.len()];
    kernel
        .mm
        .read_virtual(&mut proc.address_space, mmap_addr, &mut read_buf)
        .expect("read from mmap failed");
    assert_eq!(&read_buf, file_content);

    // 修改 mmap 内存
    kernel
        .mm
        .write_virtual(&mut proc.address_space, mmap_addr, b"Modified private data")
        .expect("write mmap failed");

    let mut mod_buf = [0u8; 21];
    kernel
        .mm
        .read_virtual(&mut proc.address_space, mmap_addr, &mut mod_buf)
        .unwrap();
    assert_eq!(&mod_buf, b"Modified private data");
}

#[test]
fn test_mmap_cow_fork_isolation() {
    let mut kernel = Kernel::new(32, 64, 8);
    let parent_pid = 1;

    // 父进程分配 mmap 内存并写入初始值
    let mmap_addr = kernel
        .mmap(parent_pid, 0x4000_0000, 4096, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0)
        .unwrap();
    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    kernel
        .mm
        .write_virtual(&mut parent_proc.address_space, mmap_addr, b"parent_mmap_data")
        .unwrap();

    // 执行 COW Fork
    let child_pid = kernel.fork_process_cow(parent_pid).expect("fork_cow failed");

    // 子进程对 mmap 内存写入新数据 (触发写时复制)
    let child_proc = kernel.pm.processes.get_mut(&child_pid).unwrap();
    kernel
        .mm
        .write_virtual(&mut child_proc.address_space, mmap_addr, b"child_mmap_mutated")
        .unwrap();

    // 校验父子进程的 mmap 内存相互隔离
    let child_proc = kernel.pm.processes.get_mut(&child_pid).unwrap();
    let mut child_buf = [0u8; 18];
    kernel
        .mm
        .read_virtual(&mut child_proc.address_space, mmap_addr, &mut child_buf)
        .unwrap();
    assert_eq!(&child_buf, b"child_mmap_mutated");

    let parent_proc = kernel.pm.processes.get_mut(&parent_pid).unwrap();
    let mut parent_buf = [0u8; 16];
    kernel
        .mm
        .read_virtual(&mut parent_proc.address_space, mmap_addr, &mut parent_buf)
        .unwrap();
    assert_eq!(&parent_buf, b"parent_mmap_data");
}
