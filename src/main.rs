//! Mini-OS Kernel: 具备高性能内存分配、CFS 进程调度与虚拟文件系统的微型操作系统内核

use mini_os_kernel::benchmark::run_all_benchmarks;
use mini_os_kernel::fs;
use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::shell::KernelShell;
use std::env;

fn run_kernel_demo() {
    println!("===============================================================");
    println!("          Mini-OS Kernel 全功能演示与性能特性验证             ");
    println!("===============================================================");

    // 1. 内核初始化 (64MB 虚拟内存，4096 块存储，128 块页缓存)
    println!("\n[1/5] 引导初始化内核子系统...");
    let mut kernel = Kernel::new(16384, 4096, 128);
    println!("  - CPU: VirtualCpu (Ring0/Ring3, 上下文切换支持)");
    println!("  - 内存: 16384 页 (64MB), 伙伴系统 Buddy Allocator (Order 0..10) + SLAB 缓存池");
    println!("  - 调度: 完全公平调度器 CFS (B-Tree 就绪树, 40 级 Nice 权重, 纳秒级 vruntime)");
    println!("  - 文件: VFS + Inode/Direct Blocks + LRU 读写缓冲缓存 (Buffer Cache)");

    // 2. 内存分配验证
    println!("\n[2/5] 验证物理页伙伴系统与 Slab 对象分配...");
    let pfn = kernel.mm.buddy.allocate_pages(2).unwrap();
    println!("  - 成功向 Buddy 申请 Order 2 (4 页 = 16KB) 连续物理内存，PFN = {}", pfn);
    kernel.mm.buddy.free_pages(pfn).unwrap();
    println!("  - 成功释放该页块，伙伴系统自动执行位运算合并");

    let slab_h1 = kernel.mm.slab.kmalloc(&mut kernel.mm.buddy, 64).unwrap();
    let slab_h2 = kernel.mm.slab.kmalloc(&mut kernel.mm.buddy, 64).unwrap();
    println!("  - 成功从 kmalloc-64 Slab 池分配两个 64B 结构 (句柄: {}, {})", slab_h1, slab_h2);
    kernel.mm.slab.kfree(&mut kernel.mm.buddy, slab_h1).unwrap();
    kernel.mm.slab.kfree(&mut kernel.mm.buddy, slab_h2).unwrap();
    println!("  - 成功归还 Slab 对象，零外部碎片化");

    // 3. 文件系统读写与页缓存验证
    println!("\n[3/5] 验证文件系统创建、写入、读取与页缓存...");
    let fd = kernel.vfs.open("/home/test.txt", fs::O_CREAT | fs::O_RDWR).unwrap();
    let sample_text = b"Hello Mini-OS Kernel! Buffer Cache provides extreme I/O speed.\n";
    let _ = kernel.vfs.write(fd, sample_text);
    let _ = kernel.vfs.close(fd);

    let fd_read = kernel.vfs.open("/home/test.txt", fs::O_RDONLY).unwrap();
    let mut read_buf = vec![0u8; 128];
    let n = kernel.vfs.read(fd_read, &mut read_buf).unwrap();
    let _ = kernel.vfs.close(fd_read);
    println!("  - 读取 /home/test.txt 内容: {}", String::from_utf8_lossy(&read_buf[..n]).trim());
    println!("  - Buffer Cache 当前命中率: {:.2}%", kernel.vfs.cache.hit_rate());

    // 4. 多进程并发与 CFS 调度模拟
    println!("\n[4/5] 验证多进程并发创建与 CFS 完全公平调度...");
    let p1 = kernel.pm.spawn("compute_task_A", -5, 60);
    let p2 = kernel.pm.spawn("io_worker_B",      0, 60);
    let p3 = kernel.pm.spawn("background_task", 5, 60);
    println!("  - 创建进程 P1 (PID {}, nice=-5), P2 (PID {}, nice=0), P3 (PID {}, nice=5)", p1, p2, p3);

    println!("  - 执行 15 个时钟周期的调度推进:");
    let logs = kernel.step(15);
    for log in logs {
        println!("    {}", log);
    }

    // 5. 性能基准测试
    println!("\n[5/5] 执行各子系统量化性能测试套件...");
    run_all_benchmarks();
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 {
        match args[1].as_str() {
            "--bench" | "-b" => {
                run_all_benchmarks();
                return;
            }
            "--demo" | "-d" => {
                run_kernel_demo();
                return;
            }
            "--help" | "-h" => {
                println!("Mini-OS Kernel 运行选项:");
                println!("  (无参数)       - 启动交互式内核 Shell 控制台");
                println!("  --demo, -d    - 运行内核端到端功能演示与性能验证");
                println!("  --bench, -b   - 直接执行内存、调度、文件系统性能基准测试");
                return;
            }
            _ => {}
        }
    }

    // 默认进入全功能交互式 Shell
    let mut kernel = Kernel::new(16384, 4096, 128);
    let mut shell = KernelShell::new(&mut kernel);
    shell.run_interactive();
}
