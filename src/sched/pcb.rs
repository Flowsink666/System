//! 进程控制块 (Process Control Block - PCB)

use crate::arch::CpuContext;
use crate::mm::AddressSpace;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Ready,
    Running,
    Blocked(BlockedReason),
    Terminated,
    Zombie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedReason {
    Sleeping { token: u64 },
    WaitingIpc,
    WaitingChild { pid: usize },
    WaitingIo,
    WaitingPipeRead { pipe_id: usize },
    WaitingPipeWrite { pipe_id: usize },
}

/// Linux CFS 静态权重对照表 (-20 到 +19，索引 0..39)
pub const PRIO_TO_WEIGHT: [u64; 40] = [
    /* -20 */ 88761, 71755, 56483, 46273, 36291, /* -15 */ 29154, 23254, 18705, 14949,
    11916, /* -10 */ 9548, 7620, 6100, 4904, 3906, /*  -5 */ 3121, 2501, 1991, 1586,
    1277, /*   0 */ 1024, 820, 655, 526, 423, /*   5 */ 335, 272, 215, 172, 137,
    /*  10 */ 110, 87, 70, 56, 45, /*  15 */ 36, 29, 23, 18, 15,
];

pub const NICE_0_LOAD: u64 = 1024;

/// 描述符类型标志位
pub const FD_FLAG_PIPE_READ: u32 = 0x0001_0000;
pub const FD_FLAG_PIPE_WRITE: u32 = 0x0002_0000;
pub const FD_FLAG_STDIN: u32 = 0x0004_0000;
pub const FD_FLAG_STDOUT: u32 = 0x0008_0000;
pub const FD_FLAG_STDERR: u32 = 0x0010_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDescriptorType {
    VfsFile(usize),
    PipeRead(usize),
    PipeWrite(usize),
    Stdin,
    Stdout,
    Stderr,
}

/// 虚拟内存区域背后的文件映射源
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmaBackingFile {
    pub vfs_fd: usize,
    pub file_offset: usize,
    pub file_len: usize,
}

/// 虚拟内存区域 (Virtual Memory Area - VMA)
#[derive(Debug, Clone)]
pub struct Vma {
    pub start_va: usize,
    pub end_va: usize,
    pub flags: u8,
    pub name: String,
    pub backing_file: Option<VmaBackingFile>,
}

/// 文件描述符项 (映射进程内部 FD 到全局 VFS FD、IPC 管道或控制台)
#[derive(Debug, Clone)]
pub struct FileDescriptorEntry {
    pub vfs_fd: usize,
    pub flags: u32,
}

impl FileDescriptorEntry {
    pub fn new_vfs(vfs_fd: usize, flags: u32) -> Self {
        let flags = flags & (crate::fs::O_WRONLY | crate::fs::O_RDWR | crate::fs::O_CREAT
            | crate::fs::O_TRUNC | crate::fs::O_APPEND);
        Self { vfs_fd, flags }
    }

    pub fn new_pipe_read(pipe_id: usize) -> Self {
        Self {
            vfs_fd: pipe_id,
            flags: FD_FLAG_PIPE_READ,
        }
    }

    pub fn new_pipe_write(pipe_id: usize) -> Self {
        Self {
            vfs_fd: pipe_id,
            flags: FD_FLAG_PIPE_WRITE,
        }
    }

    pub fn new_stdin() -> Self {
        Self {
            vfs_fd: 0,
            flags: FD_FLAG_STDIN,
        }
    }

    pub fn new_stdout() -> Self {
        Self {
            vfs_fd: 1,
            flags: FD_FLAG_STDOUT,
        }
    }

    pub fn new_stderr() -> Self {
        Self {
            vfs_fd: 2,
            flags: FD_FLAG_STDERR,
        }
    }

    pub fn descriptor_type(&self) -> FileDescriptorType {
        if self.flags & FD_FLAG_PIPE_READ != 0 {
            FileDescriptorType::PipeRead(self.vfs_fd)
        } else if self.flags & FD_FLAG_PIPE_WRITE != 0 {
            FileDescriptorType::PipeWrite(self.vfs_fd)
        } else if self.flags & FD_FLAG_STDIN != 0 {
            FileDescriptorType::Stdin
        } else if self.flags & FD_FLAG_STDOUT != 0 {
            FileDescriptorType::Stdout
        } else if self.flags & FD_FLAG_STDERR != 0 {
            FileDescriptorType::Stderr
        } else {
            FileDescriptorType::VfsFile(self.vfs_fd)
        }
    }
}

/// 进程控制块
pub struct ProcessControlBlock {
    pub pid: usize,
    pub ppid: usize,
    pub name: String,
    pub state: ProcessState,

    // CPU 寄存器上下文
    pub context: CpuContext,

    // CFS 调度属性
    pub nice: i8,                  // -20 ~ 19
    pub weight: u64,               // 调度权重
    pub vruntime: u64,             // 虚拟运行时间 (nanoseconds)
    pub exec_time: u64,            // 累计运行物理时间 (nanoseconds)
    pub cpu_burst_remaining: u64,  // 当前任务剩余需要执行的时钟周期
    pub time_slice_remaining: u64, // 当前时间片剩余周期

    // 内存与资源
    pub address_space: AddressSpace,
    pub bytecode_mode: bool,
    pub vma_list: Vec<Vma>,
    pub fd_table: HashMap<usize, FileDescriptorEntry>,
    pub next_fd: usize,

    // 退出状态
    pub exit_code: i32,
}

impl ProcessControlBlock {
    pub fn new(pid: usize, ppid: usize, name: &str, nice: i8, burst: u64) -> Self {
        let clamped_nice = nice.clamp(-20, 19);
        let weight_idx = (clamped_nice + 20) as usize;
        let weight = PRIO_TO_WEIGHT[weight_idx];

        let mut fd_table = HashMap::new();
        // 预分配 stdin(0), stdout(1), stderr(2)
        fd_table.insert(0, FileDescriptorEntry::new_stdin());
        fd_table.insert(1, FileDescriptorEntry::new_stdout());
        fd_table.insert(2, FileDescriptorEntry::new_stderr());

        Self {
            pid,
            ppid,
            name: name.to_string(),
            state: ProcessState::Ready,
            context: CpuContext::new(0x1000, 0x7FFF_FFFF),
            nice: clamped_nice,
            weight,
            vruntime: 0,
            exec_time: 0,
            cpu_burst_remaining: burst,
            time_slice_remaining: 10,
            address_space: AddressSpace::new(64),
            bytecode_mode: false,
            vma_list: vec![
                Vma {
                    start_va: 0x1000,
                    end_va: 0x8000,
                    flags: 0x5,
                    name: "code".into(),
                    backing_file: None,
                },
                Vma {
                    start_va: 0x8000,
                    end_va: 0x10000,
                    flags: 0x3,
                    name: "data".into(),
                    backing_file: None,
                },
            ],
            fd_table,
            next_fd: 3,
            exit_code: 0,
        }
    }

    /// 根据实际消耗的物理时间 delta_exec 更新虚拟运行时间
    pub fn update_vruntime(&mut self, delta_exec: u64) {
        self.exec_time += delta_exec;
        // delta_vruntime = delta_exec * (NICE_0_LOAD / weight)
        let delta_vruntime =
            (delta_exec as u128 * NICE_0_LOAD as u128 / self.weight as u128) as u64;
        self.vruntime = self.vruntime.saturating_add(delta_vruntime);
    }

    /// 分配新的文件描述符
    pub fn alloc_fd(&mut self, entry: FileDescriptorEntry) -> usize {
        while self.fd_table.contains_key(&self.next_fd) {
            self.next_fd += 1;
        }
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fd_table.insert(fd, entry);
        fd
    }

    /// 关闭文件描述符
    pub fn close_fd(&mut self, fd: usize) -> Option<FileDescriptorEntry> {
        self.fd_table.remove(&fd)
    }

    /// 在用户地址空间 (0x2000_0000..0x7000_0000) 寻找可容纳 size 字节的连续空闲虚拟地址间隙
    pub fn find_free_vma_gap(&self, size: usize) -> Option<usize> {
        let align_size = size.checked_add(4095)? / 4096 * 4096;
        let mut candidate: usize = 0x2000_0000;
        let limit: usize = 0x7000_0000;

        while candidate.checked_add(align_size)? <= limit {
            let conflict = self.vma_list.iter().any(|vma| {
                candidate < vma.end_va && candidate + align_size > vma.start_va
            });
            if !conflict {
                return Some(candidate);
            }
            candidate += 4096;
        }
        None
    }

    /// 插入新 VMA (确保不与现有 VMA 重叠，并按起始地址有序存放)
    pub fn add_vma(&mut self, vma: Vma) -> Result<(), &'static str> {
        if vma.start_va >= vma.end_va {
            return Err("Invalid VMA range");
        }
        for existing in &self.vma_list {
            if vma.start_va < existing.end_va && vma.end_va > existing.start_va {
                return Err("VMA address range overlaps with existing region");
            }
        }
        self.vma_list.push(vma);
        self.vma_list.sort_by_key(|v| v.start_va);
        Ok(())
    }

    /// 查询包含指定虚拟地址的 VMA
    pub fn find_vma(&self, va: usize) -> Option<&Vma> {
        self.vma_list.iter().find(|v| va >= v.start_va && va < v.end_va)
    }

    /// 移除指定地址范围内的 VMA 并返回被移除的列表
    pub fn remove_vma_range(&mut self, start_va: usize, end_va: usize) -> Vec<Vma> {
        let mut removed = Vec::new();
        let mut remaining = Vec::new();
        for vma in std::mem::take(&mut self.vma_list) {
            if vma.start_va < end_va && vma.end_va > start_va {
                removed.push(vma.clone());
                if vma.start_va < start_va {
                    let mut left = vma.clone();
                    left.end_va = start_va;
                    remaining.push(left);
                }
                if vma.end_va > end_va {
                    let mut right = vma;
                    let delta = end_va - right.start_va;
                    right.start_va = end_va;
                    if let Some(backing) = &mut right.backing_file {
                        backing.file_offset = backing.file_offset.saturating_add(delta);
                    }
                    remaining.push(right);
                }
            } else {
                remaining.push(vma);
            }
        }
        self.vma_list = remaining;
        removed
    }
}
