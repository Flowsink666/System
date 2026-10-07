//! 核心内核聚合模块 (Kernel Core Orchestrator)
//! 
//! 统筹虚拟 CPU、时钟、内存管理系统、进程调度系统与文件系统的全生命周期协同

use crate::arch::{PrivilegeLevel, VirtualCpu, VirtualTimer};
use crate::fs::{VirtualFileSystem, O_CREAT, O_RDWR, O_TRUNC};
use crate::mm::MemoryManager;
use crate::sched::{ProcessManager, ProcessState};
use crate::syscall::{SyscallArgs, SyscallDispatcher};

pub struct KernelStats {
    pub total_ram_pages: usize,
    pub free_ram_pages: usize,
    pub buddy_alloc_requests: u64,
    pub buddy_merges: u64,
    pub buddy_splits: u64,
    pub external_frag_ratio: f64,
    pub tlb_hits: u64,
    pub tlb_misses: u64,
    pub tlb_hit_rate: f64,
    pub buffer_cache_hits: u64,
    pub buffer_cache_misses: u64,
    pub buffer_cache_hit_rate: f64,
    pub buffer_cache_writebacks: u64,
    pub runnable_tasks: usize,
    pub total_tasks: usize,
    pub context_switches: u64,
    pub cpu_cycles: u64,
    pub timer_ticks: u64,
}

pub struct Kernel {
    pub cpu: VirtualCpu,
    pub timer: VirtualTimer,
    pub mm: MemoryManager,
    pub pm: ProcessManager,
    pub vfs: VirtualFileSystem,
}

impl Kernel {
    /// 初始化内核
    pub fn new(total_ram_pages: usize, total_disk_blocks: usize, cache_capacity: usize) -> Self {
        let mut kernel = Self {
            cpu: VirtualCpu::new(),
            timer: VirtualTimer::new(1000), // 1000ns 每 tick
            mm: MemoryManager::new(total_ram_pages),
            pm: ProcessManager::new(1000),
            vfs: VirtualFileSystem::new(total_disk_blocks, cache_capacity),
        };

        kernel.bootstrap_filesystem();
        kernel.bootstrap_processes();

        kernel
    }

    /// 引导启动文件系统：建立系统标准目录及欢迎文件
    fn bootstrap_filesystem(&mut self) {
        let _ = self.vfs.mkdir("/bin");
        let _ = self.vfs.mkdir("/etc");
        let _ = self.vfs.mkdir("/tmp");
        let _ = self.vfs.mkdir("/home");
        let _ = self.vfs.mkdir("/proc");

        // 写入初始系统说明与配置文件
        if let Ok(fd) = self.vfs.open("/etc/os-release", O_CREAT | O_RDWR | O_TRUNC) {
            let info = b"NAME=\"Mini-OS Kernel\"\nVERSION=\"1.0.0\"\nID=rust_micro_os\n";
            let _ = self.vfs.write(fd, info);
            let _ = self.vfs.close(fd);
        }

        if let Ok(fd) = self.vfs.open("/etc/motd", O_CREAT | O_RDWR | O_TRUNC) {
            let motd = b"Welcome to Mini-OS Kernel (CFS Scheduler, Buddy+Slab MM, Extent/LRU VFS)!\n";
            let _ = self.vfs.write(fd, motd);
            let _ = self.vfs.close(fd);
        }

        // 刷盘以确保一致性
        let _ = self.vfs.sync();
    }

    /// 引导启动进程：创建 PID 1 (init / systemd 角色)
    fn bootstrap_processes(&mut self) {
        // 创建初始 init 进程，nice = 0，持续 burst 周期
        let init_pid = self.pm.spawn("system_init", 0, 1000);
        self.pm.current_pid = Some(init_pid);
        // 为 init 进程映射初始虚拟代码段和数据段
        if let Some(proc) = self.pm.processes.get_mut(&init_pid) {
            let _ = proc
                .address_space
                .allocate_and_map(&mut self.mm.buddy, 0x1000, 0x5);
            let _ = proc
                .address_space
                .allocate_and_map(&mut self.mm.buddy, 0x8000, 0x3);
        }
    }

    /// 复制进程及其完整地址空间 (Fork)
    pub fn fork_process(&mut self, parent_pid: usize) -> Result<usize, &'static str> {
        let child_pid = self.pm.fork_pcb(parent_pid)?;
        let parent_space = &self.pm.processes.get(&parent_pid).unwrap().address_space;
        let cloned_space = parent_space.fork_clone(&mut self.mm.buddy, &mut self.mm.ram)?;
        if let Some(child_proc) = self.pm.processes.get_mut(&child_pid) {
            child_proc.address_space = cloned_space;
        }
        Ok(child_pid)
    }

    /// 终止进程并完整回收其地址空间、物理页框与文件句柄
    pub fn terminate_process(&mut self, pid: usize, exit_code: i32) -> Result<(), &'static str> {
        let ppid = {
            let proc = self.pm.processes.get_mut(&pid).ok_or("Process not found")?;
            proc.address_space.destroy(&mut self.mm.buddy);
            for fd_entry in proc.fd_table.values() {
                let _ = self.vfs.close(fd_entry.vfs_fd);
            }
            proc.state = ProcessState::Zombie;
            proc.exit_code = exit_code;
            proc.ppid
        };

        // 唤醒可能正在 waitpid 等待的父进程
        if ppid > 0 {
            self.pm.wake(ppid);
        }

        // 若为孤儿进程或 init，彻底移除
        if ppid == 0 || pid == 1 {
            self.pm.processes.remove(&pid);
        }

        if self.pm.current_pid == Some(pid) {
            self.pm.current_pid = None;
        }

        Ok(())
    }

    /// 触发一个或多个时钟周期，驱动 CPU 与调度器协同执行
    pub fn step(&mut self, ticks: u64) -> Vec<String> {
        let mut log = Vec::new();

        for _ in 0..ticks {
            // 1. 定时器步进，唤醒休眠进程
            let awakened = self.timer.tick();
            for pid in awakened {
                self.pm.wake(pid);
                log.push(format!("[Timer] Awakened sleeping process PID {}", pid));
            }

            // 2. 推进调度器
            let (current_pid, event, context_switched) = self.pm.schedule_step(1);

            // 3. CPU 硬件级上下文切换执行链
            if context_switched {
                if let Some(pid) = current_pid {
                    if let Some(proc) = self.pm.processes.get(&pid) {
                        self.cpu.switch_to(&proc.context, PrivilegeLevel::Ring3User);
                    }
                } else {
                    self.cpu.privilege = PrivilegeLevel::Ring0Kernel;
                }
            }

            // 4. 步进指令并保存现场
            if let Some(pid) = current_pid {
                self.cpu.step(10); // 假定每调度周期运行 10 条指令
                if let Some(proc) = self.pm.processes.get_mut(&pid) {
                    proc.context = self.cpu.context;
                    // 模拟指令执行访问虚拟内存，触发 TLB / 缺页测试
                    let _ = proc.address_space.translate(0x1000);
                }
            }

            log.push(event.display(&self.pm.processes));
        }

        log
    }

    /// 执行系统调用
    pub fn syscall(&mut self, args: SyscallArgs) -> Result<isize, &'static str> {
        self.cpu.privilege = PrivilegeLevel::Ring0Kernel;
        let res = SyscallDispatcher::dispatch(args, &mut self.mm, &mut self.pm, &mut self.vfs);
        self.cpu.privilege = PrivilegeLevel::Ring3User;
        res
    }

    /// 搜集系统状态统计
    pub fn get_stats(&self) -> KernelStats {
        let tlb_hits: u64 = self
            .pm
            .processes
            .values()
            .map(|p| p.address_space.tlb.hits)
            .sum();
        let tlb_misses: u64 = self
            .pm
            .processes
            .values()
            .map(|p| p.address_space.tlb.misses)
            .sum();
        let tlb_total = tlb_hits + tlb_misses;
        let tlb_hit_rate = if tlb_total == 0 {
            0.0
        } else {
            (tlb_hits as f64 / tlb_total as f64) * 100.0
        };

        KernelStats {
            total_ram_pages: self.mm.buddy.total_pages,
            free_ram_pages: self.mm.buddy.stats.free_pages,
            buddy_alloc_requests: self.mm.buddy.stats.alloc_requests,
            buddy_merges: self.mm.buddy.stats.merges_count,
            buddy_splits: self.mm.buddy.stats.splits_count,
            external_frag_ratio: self.mm.buddy.external_fragmentation_ratio(),
            tlb_hits,
            tlb_misses,
            tlb_hit_rate,
            buffer_cache_hits: self.vfs.cache.hits,
            buffer_cache_misses: self.vfs.cache.misses,
            buffer_cache_hit_rate: self.vfs.cache.hit_rate(),
            buffer_cache_writebacks: self.vfs.cache.writebacks,
            runnable_tasks: self.pm.scheduler.runnable_count(),
            total_tasks: self.pm.processes.len(),
            context_switches: self.cpu.context_switches, // 使用真正的硬件 CPU 上下文切换计数！
            cpu_cycles: self.cpu.cycles,
            timer_ticks: self.timer.current_tick,
        }
    }
}
