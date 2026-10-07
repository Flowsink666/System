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
    Sleeping,
    WaitingIpc,
    WaitingChild,
    WaitingIo,
}

/// Linux CFS 静态权重对照表 (-20 到 +19，索引 0..39)
pub const PRIO_TO_WEIGHT: [u64; 40] = [
    /* -20 */ 88761, 71755, 56483, 46273, 36291,
    /* -15 */ 29154, 23254, 18705, 14949, 11916,
    /* -10 */  9548,  7620,  6100,  4904,  3906,
    /*  -5 */  3121,  2501,  1991,  1586,  1277,
    /*   0 */  1024,   820,   655,   526,   423,
    /*   5 */   335,   272,   215,   172,   137,
    /*  10 */   110,    87,    70,    56,    45,
    /*  15 */    36,    29,    23,    18,    15,
];

pub const NICE_0_LOAD: u64 = 1024;

/// 虚拟内存区域 (Virtual Memory Area - VMA)
#[derive(Debug, Clone)]
pub struct Vma {
    pub start_va: usize,
    pub end_va: usize,
    pub flags: u8,
    pub name: String,
}

/// 文件描述符项 (映射进程内部 FD 到全局 VFS FD)
#[derive(Debug, Clone)]
pub struct FileDescriptorEntry {
    pub vfs_fd: usize,
    pub flags: u32,
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
    pub nice: i8,              // -20 ~ 19
    pub weight: u64,           // 调度权重
    pub vruntime: u64,         // 虚拟运行时间 (nanoseconds)
    pub exec_time: u64,        // 累计运行物理时间 (nanoseconds)
    pub cpu_burst_remaining: u64, // 当前任务剩余需要执行的时钟周期
    pub time_slice_remaining: u64, // 当前时间片剩余周期
    
    // 内存与资源
    pub address_space: AddressSpace,
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
        fd_table.insert(0, FileDescriptorEntry { vfs_fd: 0, flags: 0 });
        fd_table.insert(1, FileDescriptorEntry { vfs_fd: 1, flags: 1 });
        fd_table.insert(2, FileDescriptorEntry { vfs_fd: 2, flags: 1 });

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
            vma_list: vec![
                Vma { start_va: 0x1000, end_va: 0x8000, flags: 0x5, name: "code".into() },
                Vma { start_va: 0x8000, end_va: 0x10000, flags: 0x3, name: "data".into() },
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
        let delta_vruntime = (delta_exec as u128 * NICE_0_LOAD as u128 / self.weight as u128) as u64;
        self.vruntime = self.vruntime.saturating_add(delta_vruntime);
    }

    /// 分配新的文件描述符
    pub fn alloc_fd(&mut self, entry: FileDescriptorEntry) -> usize {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fd_table.insert(fd, entry);
        fd
    }

    /// 关闭文件描述符
    pub fn close_fd(&mut self, fd: usize) -> Option<FileDescriptorEntry> {
        self.fd_table.remove(&fd)
    }
}
