//! 文件管理子系统 (File Management Subsystem)

pub mod block_dev;
pub mod buffer_cache;
pub mod dir;
pub mod disk;
pub mod inode;
pub mod vfs;

pub use block_dev::{BlockDevStats, VirtualBlockDevice, BLOCK_SIZE};
pub use buffer_cache::{BufferCache, CachedBlock};
pub use dir::{DirEntry, Directory};
pub use disk::{
    commit_vfs_to_disk, format_disk, mount_filesystem, DiskInode, DiskSuperblock, DISK_INODE_COUNT,
    DISK_MAGIC, DISK_VERSION,
};
pub use inode::{Inode, InodeType, DIRECT_BLOCKS_COUNT};
pub use vfs::{FileStat, VirtualFileSystem, O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
