//! 内核交互式控制台与监视器 (Interactive Kernel Shell & Monitor)

use crate::benchmark::run_all_benchmarks;
use crate::fs::{InodeType, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC};
use crate::kernel::Kernel;
use std::io::{self, BufRead, Write};

pub struct KernelShell<'a> {
    kernel: &'a mut Kernel,
    current_working_dir: String,
}

impl<'a> KernelShell<'a> {
    pub fn new(kernel: &'a mut Kernel) -> Self {
        Self {
            kernel,
            current_working_dir: "/".to_string(),
        }
    }

    /// 路径规范化 (支持相对路径、'.' 与 '..')
    pub fn normalize_path(&self, raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return self.current_working_dir.clone();
        }

        let full_path = if trimmed.starts_with('/') {
            trimmed.to_string()
        } else if self.current_working_dir == "/" {
            format!("/{}", trimmed)
        } else {
            format!("{}/{}", self.current_working_dir, trimmed)
        };

        let mut stack = Vec::new();
        for seg in full_path.split('/') {
            if seg.is_empty() || seg == "." {
                continue;
            } else if seg == ".." {
                stack.pop();
            } else {
                stack.push(seg);
            }
        }

        if stack.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", stack.join("/"))
        }
    }

    /// 启动交互式命令行循环
    pub fn run_interactive(&mut self) {
        println!("===============================================================");
        println!("         Mini-OS Kernel 交互式控制台 (Interactive Shell)         ");
        println!(" 输入 'help' 查看所有可用指令，输入 'bench' 执行性能基准评测。  ");
        println!("===============================================================");

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        loop {
            print!("mini-os:{}$ ", self.current_working_dir);
            let _ = stdout.flush();

            let mut input = String::new();
            if stdin.lock().read_line(&mut input).is_err() {
                break;
            }

            let trimmed = input.trim();
            if trimmed.is_empty() {
                continue;
            }

            self.execute_line(trimmed);
        }
    }

    /// 执行单个指令
    pub fn execute_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let cmd = parts[0];
        let args = &parts[1..];

        match cmd {
            "help" => self.cmd_help(),
            "status" => self.cmd_status(),
            "ps" | "top" => self.cmd_ps(),
            "spawn" => self.cmd_spawn(args),
            "kill" => self.cmd_kill(args),
            "step" => self.cmd_step(args),
            "run" => self.cmd_run(args),
            "meminfo" => self.cmd_meminfo(),
            "cd" => self.cmd_cd(args),
            "pwd" => println!("{}", self.current_working_dir),
            "ls" => self.cmd_ls(args),
            "cat" => self.cmd_cat(args),
            "touch" => self.cmd_touch(args),
            "write" => self.cmd_write(args),
            "mkdir" => self.cmd_mkdir(args),
            "rm" => self.cmd_rm(args),
            "sync" => self.cmd_sync(),
            "bench" => run_all_benchmarks(),
            "clear" => {
                print!("\x1B[2J\x1B[1;1H");
                let _ = io::stdout().flush();
            }
            "exit" | "quit" => {
                println!("正在关闭 Mini-OS 内核虚拟机... 再见！");
                std::process::exit(0);
            }
            _ => println!("未知命令: '{}'。输入 'help' 查看支持的命令。", cmd),
        }
    }

    fn cmd_help(&self) {
        println!("\nMini-OS 内核支持命令清单:");
        println!("  status                - 查看内核整体运行状态、CPU 周期与内存/缓存概况");
        println!("  ps / top              - 查看系统进程表 (PID, Nice, 状态, 虚拟运行时间等)");
        println!("  spawn <name> [nice] [burst] - 创建新进程 (默认 nice=0, burst=100)");
        println!("  kill <pid>            - 终止指定 PID 进程");
        println!("  step [ticks]          - 单步/多步推动时钟周期并执行 CFS 调度 (默认 1)");
        println!("  run <ticks>           - 连续推进指定周期数 (例如 run 50)");
        println!("  meminfo               - 查看 Buddy 系统各阶空闲块、Slab 缓存及 TLB 统计");
        println!("  cd [path]             - 切换当前工作目录 (支持相对路径与 '..')");
        println!("  pwd                   - 打印当前工作目录");
        println!("  ls [path]             - 列出指定目录下的文件与子目录");
        println!("  cat <path>            - 读取并打印文件内容");
        println!("  touch <path>          - 创建空文件");
        println!("  write <path> <text>   - 向文件写入文本内容");
        println!("  mkdir <path>          - 创建新目录");
        println!("  rm <path>             - 删除文件或空目录");
        println!("  sync                  - 刷新所有页缓存脏块到物理块存储");
        println!("  bench                 - 运行全套性能基准测试 (内存、调度、文件 I/O)");
        println!("  clear                 - 清除终端屏幕");
        println!("  exit / quit           - 退出内核\n");
    }

    fn cmd_status(&self) {
        let stats = self.kernel.get_stats();
        println!("\n========== Mini-OS 内核运行状态 ==========");
        println!("  CPU 累计执行周期:      {}", stats.cpu_cycles);
        println!("  时钟中断 (Ticks):       {}", stats.timer_ticks);
        println!("  当前就绪任务 / 总任务:  {} / {}", stats.runnable_tasks, stats.total_tasks);
        println!("  CFS 累计上下文切换:     {}", stats.context_switches);
        println!("  物理内存总页数 / 空闲:  {} / {} (4KB/页)", stats.total_ram_pages, stats.free_ram_pages);
        println!("  Buddy 外部碎片率:       {:.2}%", stats.external_frag_ratio * 100.0);
        println!("  TLB 地址转换命中率:     {:.2}% ({} hits / {} misses)", stats.tlb_hit_rate, stats.tlb_hits, stats.tlb_misses);
        println!("  Buffer Cache 命中率:    {:.2}% ({} hits / {} misses, {} writebacks)", 
            stats.buffer_cache_hit_rate, stats.buffer_cache_hits, stats.buffer_cache_misses, stats.buffer_cache_writebacks);
        println!("==========================================\n");
    }

    fn cmd_ps(&self) {
        println!("\n{:<5} {:<6} {:<18} {:<12} {:<5} {:<6} {:<12} {:<10}", 
            "PID", "PPID", "NAME", "STATE", "NICE", "WEIGHT", "VRUNTIME(ns)", "BURST_REM");
        println!("{}", "-".repeat(80));

        let mut procs: Vec<_> = self.kernel.pm.processes.values().collect();
        procs.sort_by_key(|p| p.pid);

        for p in procs {
            let state_str = format!("{:?}", p.state);
            let is_curr = if self.kernel.pm.current_pid == Some(p.pid) { "*" } else { " " };
            println!("{}{:<4} {:<6} {:<18} {:<12} {:<5} {:<6} {:<12} {:<10}",
                is_curr, p.pid, p.ppid, p.name, state_str, p.nice, p.weight, p.vruntime, p.cpu_burst_remaining);
        }
        println!("(* 表示当前正在 CPU 上执行的任务)\n");
    }

    fn cmd_spawn(&mut self, args: &[&str]) {
        if args.is_empty() {
            println!("用法: spawn <name> [nice] [burst]");
            return;
        }
        let name = args[0];
        let nice: i8 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let burst: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100);

        let pid = self.kernel.pm.spawn(name, nice, burst);
        println!("成功创建进程: PID {}, 名称='{}', nice={}, cpu_burst={}", pid, name, nice, burst);
    }

    fn cmd_kill(&mut self, args: &[&str]) {
        if args.is_empty() {
            println!("用法: kill <pid>");
            return;
        }
        let pid: usize = match args[0].parse() {
            Ok(p) => p,
            Err(_) => {
                println!("无效的 PID 数字");
                return;
            }
        };

        match self.kernel.pm.kill(pid) {
            Ok(_) => println!("已成功终止进程 PID {}", pid),
            Err(e) => println!("终止进程失败: {}", e),
        }
    }

    fn cmd_step(&mut self, args: &[&str]) {
        let ticks: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
        let logs = self.kernel.step(ticks);
        for line in logs {
            println!("  [SCHED] {}", line);
        }
    }

    fn cmd_run(&mut self, args: &[&str]) {
        let ticks: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(20);
        let logs = self.kernel.step(ticks);
        let total = logs.len();
        if total <= 10 {
            for line in logs {
                println!("  [SCHED] {}", line);
            }
        } else {
            for line in &logs[..5] {
                println!("  [SCHED] {}", line);
            }
            println!("  ... [省略中间 {} 条调度日志] ...", total - 10);
            for line in &logs[total - 5..] {
                println!("  [SCHED] {}", line);
            }
        }
        println!("完成 {} 个周期的调度推进。", ticks);
    }

    fn cmd_meminfo(&self) {
        println!("\n========== 物理内存与伙伴系统 (Buddy Allocator) ==========");
        println!("  总页数:      {} (4KB 每页)", self.kernel.mm.buddy.total_pages);
        println!("  已分配页数:  {}", self.kernel.mm.buddy.stats.allocated_pages);
        println!("  空闲页数:    {}", self.kernel.mm.buddy.stats.free_pages);
        println!("  分配请求总数: {}", self.kernel.mm.buddy.stats.alloc_requests);
        println!("  释放请求总数: {}", self.kernel.mm.buddy.stats.free_requests);
        println!("  伙伴合并次数: {}", self.kernel.mm.buddy.stats.merges_count);
        println!("  伙伴拆分次数: {}", self.kernel.mm.buddy.stats.splits_count);
        println!("  外部碎片率:  {:.2}%", self.kernel.mm.buddy.external_fragmentation_ratio() * 100.0);
        println!("  各阶可用块分布 (Order 0..10):");
        for (order, count) in self.kernel.mm.buddy.free_blocks_by_order() {
            let block_kb = (1 << order) * 4;
            println!("    - Order {:2} ({:4} KB): {:5} 个空闲块", order, block_kb, count);
        }

        println!("\n========== 对象高速缓存分配器 (Slab Allocator) ==========");
        println!("{:<16} {:<12} {:<14} {:<10}", "CACHE NAME", "OBJ SIZE", "ACTIVE OBJS", "SLAB PAGES");
        println!("{}", "-".repeat(56));
        for (name, size, objs, slabs) in self.kernel.mm.slab.get_cache_stats() {
            println!("{:<16} {:<12} {:<14} {:<10}", name, size, objs, slabs);
        }
        println!();
    }

    fn cmd_cd(&mut self, args: &[&str]) {
        let target = args.first().unwrap_or(&"/");
        let path = self.normalize_path(target);
        match self.kernel.vfs.stat(&path) {
            Ok(st) => {
                if st.inode_type == InodeType::Directory {
                    self.current_working_dir = path;
                } else {
                    println!("cd 错误: '{}' 不是目录", target);
                }
            }
            Err(e) => println!("cd 错误: {}", e),
        }
    }

    fn cmd_ls(&self, args: &[&str]) {
        let raw = args.first().unwrap_or(&"");
        let path = self.normalize_path(raw);
        match self.kernel.vfs.list_dir(&path) {
            Ok(entries) => {
                println!("\n目录内容 [{}]:", path);
                println!("{:<20} {:<10} {:<10} {:<10} {:<8}", "NAME", "TYPE", "INODE", "SIZE(B)", "BLOCKS");
                println!("{}", "-".repeat(60));
                for (name, stat) in entries {
                    let type_str = format!("{:?}", stat.inode_type);
                    println!("{:<20} {:<10} {:<10} {:<10} {:<8}", name, type_str, stat.inode_id, stat.size, stat.blocks_used);
                }
                println!();
            }
            Err(e) => println!("ls 错误: {}", e),
        }
    }

    fn cmd_cat(&mut self, args: &[&str]) {
        if args.is_empty() {
            println!("用法: cat <path>");
            return;
        }
        let path = self.normalize_path(args[0]);
        match self.kernel.vfs.open(&path, O_RDONLY) {
            Ok(fd) => {
                let mut content = vec![0u8; 4096];
                match self.kernel.vfs.read(fd, &mut content) {
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&content[..n]);
                        print!("{}", text);
                        if !text.ends_with('\n') {
                            println!();
                        }
                    }
                    Err(e) => println!("读取错误: {}", e),
                }
                let _ = self.kernel.vfs.close(fd);
            }
            Err(e) => println!("打开文件失败: {}", e),
        }
    }

    fn cmd_touch(&mut self, args: &[&str]) {
        if args.is_empty() {
            println!("用法: touch <path>");
            return;
        }
        let path = self.normalize_path(args[0]);
        match self.kernel.vfs.open(&path, O_CREAT | O_RDWR) {
            Ok(fd) => {
                let _ = self.kernel.vfs.close(fd);
                println!("已创建/更新文件: {}", path);
            }
            Err(e) => println!("touch 错误: {}", e),
        }
    }

    fn cmd_write(&mut self, args: &[&str]) {
        if args.len() < 2 {
            println!("用法: write <path> <content...>");
            return;
        }
        let path = self.normalize_path(args[0]);
        let content = args[1..].join(" ") + "\n";

        match self.kernel.vfs.open(&path, O_CREAT | O_RDWR | O_TRUNC) {
            Ok(fd) => {
                match self.kernel.vfs.write(fd, content.as_bytes()) {
                    Ok(n) => println!("成功写入 {} 字节到 {}", n, path),
                    Err(e) => println!("写入失败: {}", e),
                }
                let _ = self.kernel.vfs.close(fd);
            }
            Err(e) => println!("打开文件失败: {}", e),
        }
    }

    fn cmd_mkdir(&mut self, args: &[&str]) {
        if args.is_empty() {
            println!("用法: mkdir <path>");
            return;
        }
        let path = self.normalize_path(args[0]);
        match self.kernel.vfs.mkdir(&path) {
            Ok(inode) => println!("已成功创建目录 {} (Inode {})", path, inode),
            Err(e) => println!("mkdir 错误: {}", e),
        }
    }

    fn cmd_rm(&mut self, args: &[&str]) {
        if args.is_empty() {
            println!("用法: rm <path>");
            return;
        }
        let path = self.normalize_path(args[0]);
        match self.kernel.vfs.unlink(&path) {
            Ok(_) => println!("已成功删除 {}", path),
            Err(e) => println!("rm 错误: {}", e),
        }
    }

    fn cmd_sync(&mut self) {
        match self.kernel.vfs.sync() {
            Ok(_) => println!("缓冲缓存已同步刷入磁盘介质 (Sync completed)."),
            Err(e) => println!("Sync 失败: {}", e),
        }
    }
}
