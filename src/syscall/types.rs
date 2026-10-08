//! 系统调用号与系统调用类型定义 (Syscall IDs & Signatures)

pub const SYS_EXIT: usize = 1;
pub const SYS_FORK: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_WRITE: usize = 4;
pub const SYS_OPEN: usize = 5;
pub const SYS_CLOSE: usize = 6;
pub const SYS_WAITPID: usize = 7;
pub const SYS_KMALLOC: usize = 8;
pub const SYS_KFREE: usize = 9;
pub const SYS_PIPE: usize = 10;
pub const SYS_YIELD: usize = 11;
pub const SYS_SLEEP: usize = 12;
pub const SYS_GETPID: usize = 13;
pub const SYS_STAT: usize = 14;
pub const SYS_MKDIR: usize = 15;
pub const SYS_EXEC: usize = 16;
pub const SYS_FORK_COW: usize = 17;
pub const SYS_MMAP: usize = 18;
pub const SYS_MUNMAP: usize = 19;

pub const PROT_NONE: u8 = 0x0;
pub const PROT_READ: u8 = 0x1;
pub const PROT_WRITE: u8 = 0x2;
pub const PROT_EXEC: u8 = 0x4;

pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_ANONYMOUS: u32 = 0x20;

#[derive(Debug, Clone, Copy)]
pub struct SyscallArgs {
    pub num: usize,
    pub arg0: usize,
    pub arg1: usize,
    pub arg2: usize,
    pub arg3: usize,
}
