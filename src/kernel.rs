//! 核心内核聚合模块 (Kernel Core Orchestrator)
//!
//! 统筹虚拟 CPU、时钟、内存管理系统、进程调度系统与文件系统的全生命周期协同

use crate::arch::{Instruction, PrivilegeLevel, VirtualCpu, VirtualTimer};
use crate::fs::{O_CREAT, O_RDWR, O_TRUNC, VirtualFileSystem};
use crate::mm::MemoryManager;
use crate::sched::{
    BlockedReason, FileDescriptorEntry, FileDescriptorType, ProcessManager, ProcessState,
    ScheduleEvent,
};
use crate::syscall::{SyscallArgs, SyscallDispatcher};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelEvent {
    Awakened { pid: usize },
    Schedule(ScheduleEvent),
}

pub struct StepResult {
    pub awakened: Vec<usize>,
    pub schedule: ScheduleEvent,
}

impl KernelEvent {
    pub fn display(self, kernel: &Kernel) -> String {
        match self {
            Self::Awakened { pid } => format!("[Timer] Awakened sleeping process PID {pid}"),
            Self::Schedule(event) => event.display(&kernel.pm.processes),
        }
    }
}

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
    pub pm: ProcessManager,
    pub mm: Box<MemoryManager>,
    pub vfs: VirtualFileSystem,
    pub console_stdin: VecDeque<u8>,
    pub console_stdout: Vec<u8>,
    loaded_pid: Option<usize>,
}

impl Kernel {
    /// 初始化内核
    pub fn new(total_ram_pages: usize, total_disk_blocks: usize, cache_capacity: usize) -> Self {
        let mut kernel = Self {
            cpu: VirtualCpu::new(),
            timer: VirtualTimer::new(1000), // 1000ns 每 tick
            pm: ProcessManager::new(1000),
            mm: Box::new(MemoryManager::new(total_ram_pages)),
            vfs: VirtualFileSystem::new(total_disk_blocks, cache_capacity),
            console_stdin: VecDeque::new(),
            console_stdout: Vec::new(),
            loaded_pid: None,
        };

        kernel.bootstrap_filesystem();
        kernel.bootstrap_processes();

        kernel
    }

    /// 从已格式化或已持久化的块设备冷启动恢复内核
    pub fn from_disk(
        total_ram_pages: usize,
        dev: crate::fs::VirtualBlockDevice,
        cache_capacity: usize,
    ) -> Result<Self, &'static str> {
        let vfs = crate::fs::mount_filesystem(dev, cache_capacity)?;
        let mut kernel = Self {
            cpu: VirtualCpu::new(),
            timer: VirtualTimer::new(1000),
            pm: ProcessManager::new(1000),
            mm: Box::new(MemoryManager::new(total_ram_pages)),
            vfs,
            console_stdin: VecDeque::new(),
            console_stdout: Vec::new(),
            loaded_pid: None,
        };
        kernel.bootstrap_processes();
        Ok(kernel)
    }

    /// 将内核文件系统状态完整持久化提交到底层磁盘
    pub fn commit_disk(&mut self) -> Result<(), &'static str> {
        self.vfs.commit_to_disk()
    }

    /// Replace the filesystem only when no task or monitor still references it.
    pub fn load_filesystem(&mut self, dev: crate::fs::VirtualBlockDevice) -> Result<(), &'static str> {
        if !self.vfs.open_files.is_empty() || self.pm.processes.values().any(|proc| {
            proc.fd_table.values().any(|fd| matches!(fd.descriptor_type(), FileDescriptorType::VfsFile(_)))
                || proc.vma_list.iter().any(|vma| vma.backing_file.is_some())
        }) {
            return Err("Filesystem is busy: close file handles and file mappings before loading an image");
        }
        let replacement = crate::fs::mount_filesystem(dev, self.vfs.cache.capacity)?;
        self.vfs = replacement;
        Ok(())
    }

    pub fn spawn_executable(&mut self, path: &str, nice: i8, burst: u64) -> Result<usize, &'static str> {
        let pid = self.pm.spawn(path, nice, burst);
        if let Err(error) = self.exec_process(pid, path) {
            self.terminate_process(pid, -1)?;
            return Err(error);
        }
        Ok(pid)
    }

    fn ensure_live_process(&self, pid: usize) -> Result<(), &'static str> {
        let proc = self.pm.processes.get(&pid).ok_or("Process not found")?;
        if matches!(proc.state, ProcessState::Zombie | ProcessState::Terminated) {
            return Err("Process has already exited");
        }
        Ok(())
    }

    pub fn console_write(&mut self, data: &[u8]) {
        self.console_stdout.extend_from_slice(data);
    }

    pub fn console_read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.console_stdin.len());
        for (slot, byte) in buf.iter_mut().take(to_read).zip(self.console_stdin.drain(..to_read)) {
            *slot = byte;
        }
        to_read
    }

    /// 统一关闭或释放描述符所引用的底层对象 (VFS 文件、Pipe 读写端、Console)
    pub fn close_fd_entry(&mut self, entry: FileDescriptorEntry) -> Result<(), &'static str> {
        match entry.descriptor_type() {
            FileDescriptorType::VfsFile(vfs_fd) => {
                self.vfs.close(vfs_fd)?;
            }
            FileDescriptorType::PipeRead(pipe_id) => {
                let (wake_writers, should_remove) = if let Some(pipe) = self.pm.pipes.get_mut(&pipe_id) {
                    (pipe.close_reader(), pipe.is_dead())
                } else {
                    (false, false)
                };
                if wake_writers {
                    self.pm.wake_pipe_writers(pipe_id);
                }
                if should_remove {
                    self.pm.pipes.remove(&pipe_id);
                }
            }
            FileDescriptorType::PipeWrite(pipe_id) => {
                let (wake_readers, should_remove) = if let Some(pipe) = self.pm.pipes.get_mut(&pipe_id) {
                    (pipe.close_writer(), pipe.is_dead())
                } else {
                    (false, false)
                };
                if wake_readers {
                    self.pm.wake_pipe_readers(pipe_id);
                }
                if should_remove {
                    self.pm.pipes.remove(&pipe_id);
                }
            }
            FileDescriptorType::Stdin
            | FileDescriptorType::Stdout
            | FileDescriptorType::Stderr => {}
        }
        Ok(())
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
            let motd =
                b"Welcome to Mini-OS Kernel (CFS Scheduler, Buddy+Slab MM, Extent/LRU VFS)!\n";
            let _ = self.vfs.write(fd, motd);
            let _ = self.vfs.close(fd);
        }

        // 写入预置二进制程序 /bin/hello (代码 64B + 紧跟的数据段 25B)
        let mut builder = crate::arch::ProgramBuilder::new();
        builder.mov_imm(crate::arch::Register::Rax, crate::syscall::SYS_WRITE as u32);
        builder.mov_imm(crate::arch::Register::Rdi, 1);
        builder.mov_imm(crate::arch::Register::Rsi, 0x1040);
        builder.mov_imm(crate::arch::Register::Rdx, 25);
        builder.syscall();
        builder.mov_imm(crate::arch::Register::Rax, crate::syscall::SYS_EXIT as u32);
        builder.mov_imm(crate::arch::Register::Rdi, 0);
        builder.syscall();
        let mut hello_bin = builder.finish();
        hello_bin.extend_from_slice(b"Hello from Mini-OS Exec!\n");

        if let Ok(fd) = self.vfs.open("/bin/hello", O_CREAT | O_RDWR | O_TRUNC) {
            let _ = self.vfs.write(fd, &hello_bin);
            let _ = self.vfs.close(fd);
        }

        // 刷盘以确保一致性
        let _ = self.vfs.sync();
        let _ = self.vfs.commit_to_disk();
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
                .allocate_and_map(&mut self.mm, 0x1000, 0x5);
            let _ = proc
                .address_space
                .allocate_and_map(&mut self.mm, 0x8000, 0x7);
            self.cpu.context = proc.context;
            self.cpu.privilege = PrivilegeLevel::Ring3User;
            self.loaded_pid = Some(init_pid);
        }
    }

    /// 复制进程及其完整地址空间 (Fork)
    pub fn fork_process(&mut self, parent_pid: usize) -> Result<usize, &'static str> {
        self.ensure_live_process(parent_pid)?;
        let parent_space = &self
            .pm
            .processes
            .get(&parent_pid)
            .ok_or("Parent process not found")?
            .address_space;
        let cloned_space = parent_space.fork_clone(&mut self.mm)?;
        let child_pid = match self.pm.fork_pcb(parent_pid) {
            Ok(pid) => pid,
            Err(e) => {
                let mut space = cloned_space;
                space.destroy(&mut self.mm);
                return Err(e);
            }
        };
        if let Some(child_proc) = self.pm.processes.get_mut(&child_pid) {
            for fd_entry in child_proc.fd_table.values() {
                match fd_entry.descriptor_type() {
                    FileDescriptorType::VfsFile(vfs_fd) => {
                        let _ = self.vfs.dup_fd(vfs_fd);
                    }
                    FileDescriptorType::PipeRead(pipe_id) => {
                        if let Some(pipe) = self.pm.pipes.get_mut(&pipe_id) {
                            pipe.readers_count += 1;
                        }
                    }
                    FileDescriptorType::PipeWrite(pipe_id) => {
                        if let Some(pipe) = self.pm.pipes.get_mut(&pipe_id) {
                            pipe.writers_count += 1;
                        }
                    }
                    _ => {}
                }
            }
            child_proc.address_space = cloned_space;
            child_proc.context.rax = 0;
        }
        Ok(child_pid)
    }

    /// 基于写时复制 (COW) 复制进程及其地址空间 (延迟分配与物理页共享)
    pub fn fork_process_cow(&mut self, parent_pid: usize) -> Result<usize, &'static str> {
        self.ensure_live_process(parent_pid)?;
        let parent_proc = self
            .pm
            .processes
            .get_mut(&parent_pid)
            .ok_or("Parent process not found")?;
        let cloned_space = parent_proc.address_space.fork_cow(&mut self.mm)?;

        let child_pid = match self.pm.fork_pcb(parent_pid) {
            Ok(pid) => pid,
            Err(e) => {
                let mut space = cloned_space;
                space.destroy_with_mm(&mut self.mm);
                return Err(e);
            }
        };

        if let Some(child_proc) = self.pm.processes.get_mut(&child_pid) {
            for fd_entry in child_proc.fd_table.values() {
                match fd_entry.descriptor_type() {
                    FileDescriptorType::VfsFile(vfs_fd) => {
                        let _ = self.vfs.dup_fd(vfs_fd);
                    }
                    FileDescriptorType::PipeRead(pipe_id) => {
                        if let Some(pipe) = self.pm.pipes.get_mut(&pipe_id) {
                            pipe.readers_count += 1;
                        }
                    }
                    FileDescriptorType::PipeWrite(pipe_id) => {
                        if let Some(pipe) = self.pm.pipes.get_mut(&pipe_id) {
                            pipe.writers_count += 1;
                        }
                    }
                    _ => {}
                }
            }
            child_proc.address_space = cloned_space;
            child_proc.context.rax = 0;
        }

        Ok(child_pid)
    }

    /// 终止进程并完整回收其地址空间、物理页框与文件句柄
    pub fn terminate_process(&mut self, pid: usize, exit_code: i32) -> Result<(), &'static str> {
        if self.pm.processes.get(&pid).is_some_and(|proc| proc.state == ProcessState::Zombie) {
            return Ok(());
        }
        let fds: Vec<FileDescriptorEntry> = {
            let proc = self.pm.processes.get_mut(&pid).ok_or("Process not found")?;
            proc.address_space.destroy_with_mm(&mut self.mm);
            proc.vma_list.clear();
            proc.bytecode_mode = false;
            std::mem::take(&mut proc.fd_table).into_values().collect()
        };
        for fd_entry in fds {
            let _ = self.close_fd_entry(fd_entry);
        }

        let (ppid, vruntime, weight) = {
            let proc = self.pm.processes.get_mut(&pid).ok_or("Process not found")?;
            proc.state = ProcessState::Zombie;
            proc.exit_code = exit_code;
            (proc.ppid, proc.vruntime, proc.weight)
        };

        // 从调度器就绪队列中出队
        self.pm.scheduler.dequeue_task(pid, vruntime, weight);

        // 唤醒可能正在 waitpid 等待的父进程
        self.pm.wake(ppid, BlockedReason::WaitingChild { pid });
        self.timer.cancel_sleep(pid);

        // 若为孤儿进程或 init，彻底移除
        if ppid == 0 || pid == 1 {
            self.pm.processes.remove(&pid);
        }

        if self.pm.current_pid == Some(pid) {
            self.pm.current_pid = None;
        }
        if self.loaded_pid == Some(pid) {
            self.load_cpu_context(None);
        }

        Ok(())
    }

    /// 虚拟内存映射分配 (mmap)
    #[allow(clippy::too_many_arguments)]
    pub fn mmap(
        &mut self,
        pid: usize,
        hint_addr: usize,
        length: usize,
        prot: u8,
        flags: u32,
        fd: Option<usize>,
        offset: usize,
    ) -> Result<usize, &'static str> {
        self.ensure_live_process(pid)?;
        if length == 0 {
            return Err("Invalid mmap length: cannot be zero");
        }
        let page_size = crate::mm::PAGE_SIZE;
        let aligned_len = length.checked_add(page_size - 1).ok_or("mmap length overflow")?
            / page_size * page_size;

        let start_va = {
            let proc = self.pm.processes.get(&pid).ok_or("Process not found")?;
            if hint_addr != 0 {
                if !hint_addr.is_multiple_of(crate::mm::PAGE_SIZE) {
                    return Err("mmap hint_addr must be page-aligned");
                }
                hint_addr
            } else {
                proc.find_free_vma_gap(aligned_len).ok_or("Out of virtual address space")?
            }
        };
        let end_va = start_va.checked_add(aligned_len).ok_or("Virtual address overflow")?;
        if end_va as u64 > 0x1_0000_0000 {
            return Err("Virtual address out of 32-bit bounds");
        }

        // 构造页表权限标志
        let mut page_flags = crate::mm::FLAG_USER;
        if prot & crate::syscall::PROT_WRITE != 0 {
            page_flags |= crate::mm::FLAG_WRITABLE;
        }

        let is_anonymous = (flags & crate::syscall::MAP_ANONYMOUS) != 0;
        if !is_anonymous && flags & crate::syscall::MAP_SHARED != 0 {
            return Err("Shared file mappings are not supported");
        }
        let backing_file = if !is_anonymous {
            let vfs_fd = fd.ok_or("File mapping requires a file descriptor")?;
            let file_handle = self.vfs.open_files.get(&vfs_fd).ok_or("Bad file descriptor")?;
            let inode = self.vfs.inodes.get(&file_handle.inode_id).ok_or("File inode not found")?;
            if inode.inode_type != crate::fs::InodeType::Regular {
                return Err("File mapping requires a regular file");
            }
            if file_handle.flags & 3 == crate::fs::O_WRONLY {
                return Err("File mapping requires read access");
            }
            let file_size = inode.size;
            Some(crate::sched::VmaBackingFile {
                vfs_fd,
                file_offset: offset,
                file_len: file_size,
            })
        } else {
            None
        };

        let vma = crate::sched::Vma {
            start_va,
            end_va,
            flags: page_flags,
            name: if is_anonymous { "anon_mmap".into() } else { "file_mmap".into() },
            backing_file: backing_file.clone(),
        };

        let proc = self.pm.processes.get_mut(&pid).ok_or("Process not found")?;
        if proc.vma_list.iter().any(|existing| start_va < existing.end_va && end_va > existing.start_va) {
            return Err("VMA address range overlaps with existing region");
        }
        for page_va in (start_va..end_va).step_by(page_size) {
            if proc.address_space.page_directory.get_entry_mut(page_va)
                .is_some_and(|entry| entry.is_present() || entry.is_demand()) {
                return Err("Virtual page already mapped");
            }
        }

        // Read file contents without consuming the open-file cursor.
        let file_data = if let Some(bf) = &backing_file {
            let mut bytes = vec![0u8; aligned_len.min(bf.file_len.saturating_sub(offset))];
            let old_offset = self.vfs.open_files[&bf.vfs_fd].offset;
            self.vfs.seek(bf.vfs_fd, offset)?;
            let read = self.vfs.read(bf.vfs_fd, &mut bytes);
            self.vfs.seek(bf.vfs_fd, old_offset)?;
            bytes.truncate(read?);
            bytes
        } else {
            Vec::new()
        };

        let proc = self.pm.processes.get_mut(&pid).unwrap();
        let mut installed = Vec::new();
        let mapping = (|| -> Result<(), &'static str> {
            for page_va in (start_va..end_va).step_by(page_size) {
                let file_offset = page_va - start_va;
                if file_offset < file_data.len() {
                    let pfn = proc.address_space.allocate_and_map(&mut self.mm, page_va, page_flags)?;
                    installed.push(page_va);
                    let count = page_size.min(file_data.len() - file_offset);
                    self.mm.write_physical(pfn * page_size, &file_data[file_offset..file_offset + count])?;
                } else {
                    proc.address_space.map_demand_zero(page_va, page_flags)?;
                    installed.push(page_va);
                }
            }
            Ok(())
        })();
        if let Err(error) = mapping {
            for page_va in installed {
                proc.address_space.unmap_and_free(&mut self.mm, page_va)?;
            }
            return Err(error);
        }
        proc.add_vma(vma)?;
        Ok(start_va)
    }

    /// 解除虚拟内存映射 (munmap)
    pub fn munmap(&mut self, pid: usize, addr: usize, length: usize) -> Result<(), &'static str> {
        if length == 0 {
            return Err("Invalid munmap length: cannot be zero");
        }
        if !addr.is_multiple_of(crate::mm::PAGE_SIZE) {
            return Err("munmap address must be page-aligned");
        }
        let page_size = crate::mm::PAGE_SIZE;
        let aligned_len = length.checked_add(page_size - 1).ok_or("munmap length overflow")?
            / page_size * page_size;
        let end_va = addr.checked_add(aligned_len).ok_or("Address overflow")?;
        if end_va as u64 > 0x1_0000_0000 {
            return Err("Virtual address out of 32-bit bounds");
        }

        let proc = self.pm.processes.get_mut(&pid).ok_or("Process not found")?;
        proc.remove_vma_range(addr, end_va);

        for page_va in (addr..end_va).step_by(crate::mm::PAGE_SIZE) {
            proc.address_space.unmap_and_free(&mut self.mm, page_va)?;
        }

        Ok(())
    }

    /// 将字节码程序加载进指定进程的虚拟地址空间
    pub fn load_program_into_process(
        &mut self,
        pid: usize,
        code: &[u8],
    ) -> Result<(), &'static str> {
        self.ensure_live_process(pid)?;
        if code.is_empty() || code.len() > 64 * 1024 {
            return Err("Executable size must be between 1 byte and 64 KiB");
        }
        let capacity = self.pm.processes.get(&pid).ok_or("Process not found")?.address_space.tlb.capacity;
        let mut replacement = crate::mm::AddressSpace::new(capacity);
        let page_size = crate::mm::PAGE_SIZE;
        let code_end = 0x1000 + code.len().div_ceil(page_size) * page_size;
        let data_start = code_end.max(0x8000);
        let stack_start = data_start + page_size;
        let stack_end = stack_start + page_size;
        let loading = (|| -> Result<(), &'static str> {
            for va in (0x1000..code_end).step_by(page_size) {
                replacement.allocate_and_map(&mut self.mm, va, 0x7)?;
            }
            self.mm.write_virtual_checked(&mut replacement, 0x1000, code, false)?;
            for va in (0x1000..code_end).step_by(page_size) {
                replacement.page_directory.get_entry_mut(va).unwrap().flags &= !crate::mm::FLAG_WRITABLE;
            }
            replacement.tlb.flush();
            replacement.allocate_and_map(&mut self.mm, data_start, 0x7)?;
            replacement.allocate_and_map(&mut self.mm, stack_start, 0x7)?;
            Ok(())
        })();
        if let Err(error) = loading {
            replacement.destroy(&mut self.mm);
            return Err(error);
        }

        let proc = self.pm.processes.get_mut(&pid).unwrap();
        let mut previous = std::mem::replace(&mut proc.address_space, replacement);
        proc.bytecode_mode = true;
        proc.vma_list = vec![
            crate::sched::Vma { start_va: 0x1000, end_va: code_end, flags: 0x5, name: "code".into(), backing_file: None },
            crate::sched::Vma { start_va: data_start, end_va: stack_start, flags: 0x7, name: "data".into(), backing_file: None },
            crate::sched::Vma { start_va: stack_start, end_va: stack_end, flags: 0x7, name: "stack".into(), backing_file: None },
        ];
        proc.context = crate::arch::CpuContext::new(0x1000, (stack_end - 1) as u64);
        previous.destroy(&mut self.mm);
        if self.loaded_pid == Some(pid) {
            self.cpu.context = proc.context;
        }

        Ok(())
    }

    /// 执行可执行文件 (exec)
    pub fn exec_process(&mut self, pid: usize, path: &str) -> Result<(), &'static str> {
        self.ensure_live_process(pid)?;
        let fd = self.vfs.open(path, crate::fs::O_RDONLY)?;
        let mut code_buf = vec![0u8; 64 * 1024 + 1];
        let read = self.vfs.read(fd, &mut code_buf);
        self.vfs.close(fd)?;
        let n = read?;

        if n == 0 {
            return Err("Empty executable");
        }

        self.load_program_into_process(pid, &code_buf[..n])?;

        if let Some(proc) = self.pm.processes.get_mut(&pid) {
            proc.name = path.to_string();
        }

        Ok(())
    }

    /// 触发一个或多个时钟周期，驱动 CPU 与调度器协同执行
    pub fn step(&mut self, ticks: u64) -> Vec<String> {
        let mut log = Vec::new();
        self.step_stream(ticks, |kernel, event| log.push(event.display(kernel)));
        log
    }

    /// Advance one tick without formatting or accumulating text logs.
    pub fn step_once(&mut self) -> StepResult {
        let awakened = self.timer.tick().into_iter().filter_map(|event| {
            self.pm.wake(event.pid, BlockedReason::Sleeping { token: event.token })
                .then_some(event.pid)
        }).collect();
        let result = self.pm.schedule_step(1);
        let schedule = self.apply_schedule_result(result, true);
        StepResult { awakened, schedule }
    }

    pub fn step_stream(&mut self, ticks: u64, mut observer: impl FnMut(&Kernel, KernelEvent)) {
        for _ in 0..ticks {
            let result = self.step_once();
            for pid in result.awakened {
                observer(self, KernelEvent::Awakened { pid });
            }
            observer(self, KernelEvent::Schedule(result.schedule));
        }
    }

    /// 统一提交调度结果，完成最后一个 tick 后再清理退出资源。
    fn apply_schedule_result(
        &mut self,
        (pid, event, _switched): (Option<usize>, ScheduleEvent, bool),
        execute: bool,
    ) -> ScheduleEvent {
        self.load_cpu_context(pid);
        let completed_burst = matches!(event, ScheduleEvent::Terminated { .. });
        // The last quantum still belongs to this task, including its syscalls.
        if execute && completed_burst && let Some(pid) = pid {
            self.pm.processes.get_mut(&pid).expect("Scheduled process must exist").state = ProcessState::Running;
            self.pm.current_pid = Some(pid);
        }
        if execute && let Some(pid) = pid {
            self.execute_task_quantum(pid);
        }
        if let Some(pid) = pid {
            let alive = self.pm.processes.get(&pid).is_some_and(|proc| proc.state != ProcessState::Zombie);
            if completed_burst && alive {
                self.terminate_process(pid, 0).expect("Scheduled process must exist");
            }
            if completed_burst || !alive {
                return ScheduleEvent::Terminated { pid };
            }
        }
        event
    }

    /// 执行任务的时间片配额指令 (支持真实微指令虚拟机或回退至模拟步进)
    fn execute_task_quantum(&mut self, pid: usize) {
        let max_instructions = 10;
        let mut executed = 0;

        while executed < max_instructions {
            let rip = self.cpu.context.rip;
            let instr_bytes = {
                let Some(proc) = self.pm.processes.get_mut(&pid) else {
                    break;
                };
                if proc.state != ProcessState::Running && proc.state != ProcessState::Terminated {
                    break;
                }
                let mut buf = [0u8; 8];
                match self.mm.read_virtual_checked(&mut proc.address_space, rip as usize, &mut buf, false) {
                    Ok(8) => buf,
                    _ => {
                        if proc.bytecode_mode {
                            let _ = self.terminate_process(pid, -14);
                            break;
                        }
                        if executed == 0 {
                            self.cpu.step(10);
                            if let Some(p) = self.pm.processes.get_mut(&pid) {
                                p.context = self.cpu.context;
                                let _ = p.address_space.translate(0x1000);
                            }
                        }
                        break;
                    }
                }
            };

            if instr_bytes == [0u8; 8] && !self.pm.processes[&pid].bytecode_mode {
                if executed == 0 {
                    self.cpu.step(10);
                    if let Some(p) = self.pm.processes.get_mut(&pid) {
                        p.context = self.cpu.context;
                        let _ = p.address_space.translate(0x1000);
                    }
                }
                break;
            }

            let Some(instr) = Instruction::decode(&instr_bytes) else {
                let _ = self.terminate_process(pid, -4);
                break;
            };

            executed += 1;
            match instr {
                Instruction::Nop => {
                    self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    self.cpu.cycles += 1;
                }
                Instruction::MovImm { reg, imm } => {
                    self.cpu.context.set_reg(reg, imm as u64);
                    self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    self.cpu.cycles += 1;
                }
                Instruction::MovReg { dst, src } => {
                    let v = self.cpu.context.get_reg(src);
                    self.cpu.context.set_reg(dst, v);
                    self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    self.cpu.cycles += 1;
                }
                Instruction::AddImm { reg, imm } => {
                    let v = self.cpu.context.get_reg(reg);
                    self.cpu.context.set_reg(reg, v.wrapping_add(imm as u64));
                    self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    self.cpu.cycles += 1;
                }
                Instruction::SubImm { reg, imm } => {
                    let v = self.cpu.context.get_reg(reg);
                    self.cpu.context.set_reg(reg, v.wrapping_sub(imm as u64));
                    self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    self.cpu.cycles += 1;
                }
                Instruction::Jmp { target_rip } => {
                    self.cpu.context.rip = target_rip as u64;
                    self.cpu.cycles += 1;
                }
                Instruction::Jz { reg, target_rip } => {
                    if self.cpu.context.get_reg(reg) == 0 {
                        self.cpu.context.rip = target_rip as u64;
                    } else {
                        self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    }
                    self.cpu.cycles += 1;
                }
                Instruction::Jnz { reg, target_rip } => {
                    if self.cpu.context.get_reg(reg) != 0 {
                        self.cpu.context.rip = target_rip as u64;
                    } else {
                        self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    }
                    self.cpu.cycles += 1;
                }
                Instruction::LoadMem { dst, base, offset } => {
                    let base_val = self.cpu.context.get_reg(base);
                    let vaddr = (base_val as i64).wrapping_add(offset as i64) as usize;
                    let mut buf = [0u8; 8];
                    let is_user = self.cpu.privilege == PrivilegeLevel::Ring3User;
                    let read_res = {
                        let proc = self.pm.processes.get_mut(&pid).unwrap();
                        self.mm.read_virtual_checked(&mut proc.address_space, vaddr, &mut buf, is_user)
                    };
                    match read_res {
                        Ok(_) => {
                            let val = u64::from_le_bytes(buf);
                            self.cpu.context.set_reg(dst, val);
                            self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                            self.cpu.cycles += 2;
                        }
                        Err(_) => {
                            let _ = self.terminate_process(pid, -14); // SIGSEGV (14)
                            break;
                        }
                    }
                }
                Instruction::StoreMem { src, base, offset } => {
                    let base_val = self.cpu.context.get_reg(base);
                    let vaddr = (base_val as i64).wrapping_add(offset as i64) as usize;
                    let src_val = self.cpu.context.get_reg(src);
                    let buf = src_val.to_le_bytes();
                    let is_user = self.cpu.privilege == PrivilegeLevel::Ring3User;
                    let write_res = {
                        let proc = self.pm.processes.get_mut(&pid).unwrap();
                        self.mm.write_virtual_checked(&mut proc.address_space, vaddr, &buf, is_user)
                    };
                    match write_res {
                        Ok(_) => {
                            self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                            self.cpu.cycles += 2;
                        }
                        Err(_) => {
                            let _ = self.terminate_process(pid, -14); // SIGSEGV (14)
                            break;
                        }
                    }
                }
                Instruction::Syscall => {
                    self.cpu.context.rip = self.cpu.context.rip.wrapping_add(8);
                    self.cpu.cycles += 2;
                    let args = SyscallArgs {
                        num: self.cpu.context.rax as usize,
                        arg0: self.cpu.context.rdi as usize,
                        arg1: self.cpu.context.rsi as usize,
                        arg2: self.cpu.context.rdx as usize,
                        arg3: self.cpu.context.rcx as usize,
                    };
                    let ret = self.syscall(args);
                    let value = match ret {
                        Ok(v) => v as u64,
                        Err(_) => (-1i64) as u64,
                    };
                    if let Some(caller) = self.pm.processes.get_mut(&pid) {
                        caller.context.rax = value;
                    }
                    if self.loaded_pid == Some(pid) {
                        self.cpu.context.rax = value;
                    }
                    if self.pm.current_pid != Some(pid) {
                        break;
                    }
                    if let Some(proc) = self.pm.processes.get(&pid) {
                        if proc.state != ProcessState::Running {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                Instruction::Halt => {
                    self.cpu.cycles += 1;
                    let _ = self.terminate_process(pid, 0);
                    break;
                }
            }

            if let Some(proc) = self.pm.processes.get_mut(&pid) {
                proc.context = self.cpu.context;
            } else {
                break;
            }
        }
    }

    fn load_cpu_context(&mut self, pid: Option<usize>) {
        if self.loaded_pid != pid {
            self.save_cpu_context();
        }
        if let Some(pid) = pid {
            if self.loaded_pid != Some(pid) {
                let proc = self
                    .pm
                    .processes
                    .get(&pid)
                    .expect("Selected process must exist");
                self.cpu.switch_to(&proc.context, PrivilegeLevel::Ring3User);
                self.loaded_pid = Some(pid);
            }
            self.cpu.privilege = PrivilegeLevel::Ring3User;
        } else {
            self.loaded_pid = None;
            self.cpu.privilege = PrivilegeLevel::Ring0Kernel;
        }
    }

    fn save_cpu_context(&mut self) {
        if let Some(pid) = self.loaded_pid
            && let Some(proc) = self.pm.processes.get_mut(&pid)
        {
            proc.context = self.cpu.context;
        }
    }

    pub fn yield_current(&mut self) -> Result<(), &'static str> {
        let pid = self.pm.current_pid.ok_or("No current process context")?;
        let proc = self
            .pm
            .processes
            .get(&pid)
            .ok_or("Current process not found")?;
        if proc.cpu_burst_remaining == 0 {
            self.terminate_process(pid, 0)?;
        }
        let result = self.pm.yield_current();
        self.apply_schedule_result(result, false);
        Ok(())
    }

    pub fn sleep_current(&mut self, ticks: u64) -> Result<(), &'static str> {
        let pid = self.pm.current_pid.ok_or("No current process context")?;
        if !self.pm.processes.contains_key(&pid) {
            return Err("Current process not found");
        }
        if ticks == 0 {
            return self.yield_current();
        }
        let token = self.timer.add_sleep(pid, ticks)?;
        self.pm.block(pid, BlockedReason::Sleeping { token });
        self.load_cpu_context(None);
        Ok(())
    }

    /// 执行系统调用
    pub fn syscall(&mut self, args: SyscallArgs) -> Result<isize, &'static str> {
        self.load_cpu_context(self.pm.current_pid);
        self.save_cpu_context();
        self.cpu.privilege = PrivilegeLevel::Ring0Kernel;
        let res = SyscallDispatcher::dispatch(args, self);
        self.load_cpu_context(self.pm.current_pid);
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
            context_switches: self.cpu.context_switches, // 模拟 CPU 上下文加载次数。
            cpu_cycles: self.cpu.cycles,
            timer_ticks: self.timer.current_tick,
        }
    }
}
