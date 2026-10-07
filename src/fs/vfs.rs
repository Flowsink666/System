//! 虚拟文件系统接口与统一抽象 (Virtual File System - VFS)
//! 
//! 高性能特性：
//! 1. 统一管理常规文件、目录、设备、管道等文件系统对象
//! 2. 结合 BufferCache 实现端到端毫秒/微秒级低延迟文件读写
//! 3. 维护文件描述符表与游标指针 (Cursor Offset)
//! 4. 树状层级路径寻址，支持绝对路径与相对路径

use super::block_dev::VirtualBlockDevice;
use super::buffer_cache::BufferCache;
use super::dir::Directory;
use super::inode::{Inode, InodeType};
use std::collections::HashMap;

pub const O_RDONLY: u32 = 0x0;
pub const O_WRONLY: u32 = 0x1;
pub const O_RDWR: u32 = 0x2;
pub const O_CREAT: u32 = 0x4;
pub const O_TRUNC: u32 = 0x8;
pub const O_APPEND: u32 = 0x10;

#[derive(Debug, Clone)]
pub struct FileStat {
    pub inode_id: usize,
    pub inode_type: InodeType,
    pub size: usize,
    pub permissions: u16,
    pub blocks_used: usize,
}

#[derive(Debug, Clone)]
pub struct VfsOpenFile {
    pub inode_id: usize,
    pub offset: usize,
    pub flags: u32,
}

pub struct VirtualFileSystem {
    pub dev: VirtualBlockDevice,
    pub cache: BufferCache,
    pub inodes: HashMap<usize, Inode>,
    pub directories: HashMap<usize, Directory>,
    pub free_blocks: Vec<usize>,
    pub open_files: HashMap<usize, VfsOpenFile>,
    next_inode_id: usize,
    next_fd: usize,
    pub root_inode_id: usize,
}

impl VirtualFileSystem {
    pub fn new(total_blocks: usize, cache_capacity: usize) -> Self {
        let dev = VirtualBlockDevice::new(total_blocks);
        let cache = BufferCache::new(cache_capacity);
        let mut free_blocks: Vec<usize> = (0..total_blocks).collect();

        // 根目录初始化 (inode 0)
        let root_inode_id = 0;
        let mut inodes = HashMap::new();
        let mut directories = HashMap::new();

        let root_inode = Inode::new(root_inode_id, InodeType::Directory, 0o755);
        let root_dir = Directory::new(root_inode_id, root_inode_id);

        inodes.insert(root_inode_id, root_inode);
        directories.insert(root_inode_id, root_dir);

        // 保留块 0 作为超级块预留
        free_blocks.retain(|&b| b != 0);

        Self {
            dev,
            cache,
            inodes,
            directories,
            free_blocks,
            open_files: HashMap::new(),
            next_inode_id: 1,
            next_fd: 10, // 0,1,2 保留给 stdio
            root_inode_id,
        }
    }

    /// 解析路径，返回对应的 Inode ID
    pub fn resolve_path(&self, path: &str) -> Result<usize, &'static str> {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "/" {
            return Ok(self.root_inode_id);
        }

        let parts: Vec<&str> = trimmed
            .split('/')
            .filter(|p| !p.is_empty())
            .collect();

        let mut curr_inode = self.root_inode_id;

        for part in parts {
            let dir = self
                .directories
                .get(&curr_inode)
                .ok_or("Not a directory in path resolution")?;

            let entry = dir.lookup(part).ok_or("Path component not found")?;
            curr_inode = entry.inode_id;
        }

        Ok(curr_inode)
    }

    /// 解析路径的父目录及目标文件名: 返回 (parent_inode_id, filename)
    fn resolve_parent_and_name<'a>(&self, path: &'a str) -> Result<(usize, &'a str), &'static str> {
        let trimmed = path.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err("Invalid root path for creation");
        }

        if let Some(last_slash) = trimmed.rfind('/') {
            let parent_path = &trimmed[..last_slash];
            let name = &trimmed[last_slash + 1..];
            let parent_id = if parent_path.is_empty() {
                self.root_inode_id
            } else {
                self.resolve_path(parent_path)?
            };
            Ok((parent_id, name))
        } else {
            Ok((self.root_inode_id, trimmed))
        }
    }

    /// 创建新目录
    pub fn mkdir(&mut self, path: &str) -> Result<usize, &'static str> {
        let (parent_id, name) = self.resolve_parent_and_name(path)?;

        // 检查父目录是否存在同名项
        if let Some(parent_dir) = self.directories.get(&parent_id) {
            if parent_dir.lookup(name).is_some() {
                return Err("Directory or file already exists");
            }
        } else {
            return Err("Parent is not a directory");
        }

        let new_inode_id = self.next_inode_id;
        self.next_inode_id += 1;

        let new_inode = Inode::new(new_inode_id, InodeType::Directory, 0o755);
        let new_dir = Directory::new(new_inode_id, parent_id);

        self.inodes.insert(new_inode_id, new_inode);
        self.directories.insert(new_inode_id, new_dir);

        let parent_dir = self.directories.get_mut(&parent_id).unwrap();
        parent_dir.add_entry(name, new_inode_id, InodeType::Directory)?;

        Ok(new_inode_id)
    }

    /// 打开或创建文件
    pub fn open(&mut self, path: &str, flags: u32) -> Result<usize, &'static str> {
        let inode_id = match self.resolve_path(path) {
            Ok(id) => {
                let inode = self.inodes.get_mut(&id).ok_or("Inode not found")?;
                if (flags & O_TRUNC) != 0 && inode.inode_type == InodeType::Regular {
                    let old_blocks = inode.release_all_blocks(&mut self.cache, &mut self.dev);
                    for b in &old_blocks {
                        self.cache.invalidate(*b);
                    }
                    self.free_blocks.extend(old_blocks);
                }
                id
            }
            Err(_) => {
                if (flags & O_CREAT) != 0 {
                    let (parent_id, name) = self.resolve_parent_and_name(path)?;
                    let parent_dir = self
                        .directories
                        .get_mut(&parent_id)
                        .ok_or("Parent directory not found")?;

                    if parent_dir.lookup(name).is_some() {
                        return Err("File already exists");
                    }

                    let new_inode_id = self.next_inode_id;
                    self.next_inode_id += 1;

                    let new_inode = Inode::new(new_inode_id, InodeType::Regular, 0o644);
                    self.inodes.insert(new_inode_id, new_inode);
                    parent_dir.add_entry(name, new_inode_id, InodeType::Regular)?;

                    new_inode_id
                } else {
                    return Err("File not found");
                }
            }
        };

        let inode = self.inodes.get(&inode_id).unwrap();
        let offset = if (flags & O_APPEND) != 0 { inode.size } else { 0 };

        let fd = self.next_fd;
        self.next_fd += 1;
        self.open_files.insert(
            fd,
            VfsOpenFile {
                inode_id,
                offset,
                flags,
            },
        );

        Ok(fd)
    }

    /// 读取文件 (受读模式权限检查约束)
    pub fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
        let file = self.open_files.get_mut(&fd).ok_or("Invalid file descriptor")?;
        // 模式校验: O_WRONLY 不能读取
        if (file.flags & O_WRONLY) != 0 && (file.flags & O_RDWR) == 0 {
            return Err("File not open for reading");
        }
        let inode_id = file.inode_id;
        let offset = file.offset;

        let inode = self.inodes.get(&inode_id).ok_or("Inode not found")?;
        let n = inode.read_bytes(&mut self.cache, &mut self.dev, offset, buf)?;

        let file = self.open_files.get_mut(&fd).unwrap();
        file.offset += n;
        Ok(n)
    }

    /// 写入文件 (受写模式权限检查与 O_APPEND 动态重定位约束)
    pub fn write(&mut self, fd: usize, data: &[u8]) -> Result<usize, &'static str> {
        let file = self.open_files.get_mut(&fd).ok_or("Invalid file descriptor")?;
        // 模式校验: O_RDONLY (0) 无法写入
        if (file.flags & (O_WRONLY | O_RDWR)) == 0 {
            return Err("File not open for writing (read-only mode)");
        }
        let inode_id = file.inode_id;

        // O_APPEND 必须动态重定位到最新文件末尾
        if (file.flags & O_APPEND) != 0 {
            let curr_size = self.inodes.get(&inode_id).unwrap().size;
            file.offset = curr_size;
        }
        let offset = file.offset;

        let inode = self.inodes.get_mut(&inode_id).ok_or("Inode not found")?;
        let n = inode.write_bytes(
            &mut self.cache,
            &mut self.dev,
            &mut self.free_blocks,
            offset,
            data,
        )?;

        let file = self.open_files.get_mut(&fd).unwrap();
        file.offset += n;
        Ok(n)
    }

    /// 设置文件指针游标
    pub fn seek(&mut self, fd: usize, offset: usize) -> Result<usize, &'static str> {
        let file = self.open_files.get_mut(&fd).ok_or("Invalid file descriptor")?;
        file.offset = offset;
        Ok(file.offset)
    }

    /// 关闭文件
    pub fn close(&mut self, fd: usize) -> Result<(), &'static str> {
        self.open_files.remove(&fd).ok_or("Invalid file descriptor")?;
        Ok(())
    }

    /// 删除文件或目录 (遵循先校验、后提交事务机制，防止破坏目录树结构)
    pub fn unlink(&mut self, path: &str) -> Result<(), &'static str> {
        let (parent_id, name) = self.resolve_parent_and_name(path)?;
        let parent_dir = self.directories.get(&parent_id).ok_or("Parent not found")?;
        let entry = parent_dir.lookup(name).ok_or("File not found")?.clone();

        // 1. 先行校验：若为目录必须为空 (除 . 和 .. 外无文件)，若非空直接退出，绝不破坏父目录
        if entry.inode_type == InodeType::Directory {
            let dir = self.directories.get(&entry.inode_id).ok_or("Directory not found")?;
            if dir.entries.len() > 2 {
                return Err("Directory not empty");
            }
        }

        // 2. 校验全部通过，提交变更：从父目录注销该目录项
        let parent_dir_mut = self.directories.get_mut(&parent_id).unwrap();
        parent_dir_mut.remove_entry(name);

        if entry.inode_type == InodeType::Directory {
            self.directories.remove(&entry.inode_id);
        }

        // 3. 彻底释放 Inode 并回收所有底层物理数据块到 free_blocks
        if let Some(mut inode) = self.inodes.remove(&entry.inode_id) {
            let old_blocks = inode.release_all_blocks(&mut self.cache, &mut self.dev);
            for b in &old_blocks {
                self.cache.invalidate(*b);
            }
            self.free_blocks.extend(old_blocks);
        }

        Ok(())
    }

    /// 获取文件元数据 Stat
    pub fn stat(&self, path: &str) -> Result<FileStat, &'static str> {
        let inode_id = self.resolve_path(path)?;
        let inode = self.inodes.get(&inode_id).ok_or("Inode not found")?;
        let total_blocks = inode.direct_blocks_used
            + inode.indirect_blocks_used
            + if inode.indirect_block.is_some() { 1 } else { 0 };

        Ok(FileStat {
            inode_id,
            inode_type: inode.inode_type,
            size: inode.size,
            permissions: inode.permissions,
            blocks_used: total_blocks,
        })
    }

    /// 列出目录内容
    pub fn list_dir(&self, path: &str) -> Result<Vec<(String, FileStat)>, &'static str> {
        let inode_id = self.resolve_path(path)?;
        let dir = self.directories.get(&inode_id).ok_or("Not a directory")?;
        let mut results = Vec::new();

        for entry in dir.list() {
            if let Some(inode) = self.inodes.get(&entry.inode_id) {
                results.push((
                    entry.name,
                    FileStat {
                        inode_id: inode.id,
                        inode_type: inode.inode_type,
                        size: inode.size,
                        permissions: inode.permissions,
                        blocks_used: inode.direct_blocks_used,
                    },
                ));
            }
        }
        Ok(results)
    }

    /// 刷新所有缓冲缓存回物理块设备
    pub fn sync(&mut self) -> Result<(), &'static str> {
        self.cache.sync(&mut self.dev)
    }
}
