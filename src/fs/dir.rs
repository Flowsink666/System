//! 目录项与哈希索引 (Directory & Dentry Indexing)
//! 
//! 高性能特性：
//! 1. 基于哈希表结构提供 O(1) 文件名检索，避免传统线性目录扫描
//! 2. 支持树状路径分词与层级快速寻址
//! 3. 维护 "." 与 ".." 目录项

use super::inode::InodeType;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub inode_id: usize,
    pub inode_type: InodeType,
}

pub struct Directory {
    pub inode_id: usize,
    pub entries: HashMap<String, DirEntry>,
}

impl Directory {
    pub fn new(inode_id: usize, parent_inode_id: usize) -> Self {
        let mut entries = HashMap::new();
        entries.insert(
            ".".to_string(),
            DirEntry {
                name: ".".to_string(),
                inode_id,
                inode_type: InodeType::Directory,
            },
        );
        entries.insert(
            "..".to_string(),
            DirEntry {
                name: "..".to_string(),
                inode_id: parent_inode_id,
                inode_type: InodeType::Directory,
            },
        );

        Self { inode_id, entries }
    }

    /// O(1) 检索目录项
    #[inline]
    pub fn lookup(&self, name: &str) -> Option<&DirEntry> {
        self.entries.get(name)
    }

    /// 添加新项
    pub fn add_entry(&mut self, name: &str, inode_id: usize, inode_type: InodeType) -> Result<(), &'static str> {
        if self.entries.contains_key(name) {
            return Err("File or directory already exists");
        }
        self.entries.insert(
            name.to_string(),
            DirEntry {
                name: name.to_string(),
                inode_id,
                inode_type,
            },
        );
        Ok(())
    }

    /// 移除项
    pub fn remove_entry(&mut self, name: &str) -> Option<DirEntry> {
        if name == "." || name == ".." {
            return None;
        }
        self.entries.remove(name)
    }

    /// 列出所有项
    pub fn list(&self) -> Vec<DirEntry> {
        self.entries.values().cloned().collect()
    }
}
