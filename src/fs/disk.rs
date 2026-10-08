//! 磁盘镜像二进制布局与持久化格式 (On-Disk Binary Format & Persistence)
//!
//! 布局规范 (512B 块大小):
//! - 块 0: 超级块 (Superblock)
//! - 块 1: Inode 分配位图 (Inode Bitmap, 4096 Inodes 上限)
//! - 块 2: 数据块分配位图 (Data Block Bitmap, 4096 Blocks 上限)
//! - 块 3..34: Inode 表 (256 个 Inode, 每个 64B, 每块 8 个)
//! - 块 35..N: 真实数据块池 (Data Blocks)

use super::block_dev::{BLOCK_SIZE, VirtualBlockDevice};
use super::buffer_cache::BufferCache;
use super::dir::{DirEntry, Directory};
use super::inode::{DIRECT_BLOCKS_COUNT, Inode, InodeType};
use super::vfs::VirtualFileSystem;
use std::collections::{HashMap, HashSet};

pub const DISK_MAGIC: u32 = 0x4D49_4E49; // "MINI"
pub const DISK_VERSION: u32 = 1;
pub const DISK_INODE_COUNT: usize = 256;
pub const INODES_PER_BLOCK: usize = 8;
pub const INODE_TABLE_BLOCKS: usize = DISK_INODE_COUNT / INODES_PER_BLOCK; // 32 块

pub const SUPERBLOCK_BLOCK: usize = 0;
pub const INODE_BITMAP_BLOCK: usize = 1;
pub const DATA_BITMAP_BLOCK: usize = 2;
pub const INODE_TABLE_START: usize = 3;
pub const DATA_BLOCKS_START: usize = INODE_TABLE_START + INODE_TABLE_BLOCKS; // 35

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSuperblock {
    pub magic: u32,
    pub version: u32,
    pub total_blocks: u32,
    pub block_size: u32,
    pub inode_count: u32,
    pub inode_bitmap_block: u32,
    pub data_bitmap_block: u32,
    pub inode_table_block: u32,
    pub data_blocks_start: u32,
    pub root_inode_id: u32,
}

impl DiskSuperblock {
    pub fn encode(&self) -> [u8; BLOCK_SIZE] {
        let mut buf = [0u8; BLOCK_SIZE];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8..12].copy_from_slice(&self.total_blocks.to_le_bytes());
        buf[12..16].copy_from_slice(&self.block_size.to_le_bytes());
        buf[16..20].copy_from_slice(&self.inode_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.inode_bitmap_block.to_le_bytes());
        buf[24..28].copy_from_slice(&self.data_bitmap_block.to_le_bytes());
        buf[28..32].copy_from_slice(&self.inode_table_block.to_le_bytes());
        buf[32..36].copy_from_slice(&self.data_blocks_start.to_le_bytes());
        buf[36..40].copy_from_slice(&self.root_inode_id.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8; BLOCK_SIZE]) -> Result<Self, &'static str> {
        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != DISK_MAGIC {
            return Err("Invalid filesystem magic");
        }
        let version = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        if version != DISK_VERSION {
            return Err("Unsupported filesystem version");
        }
        let total_blocks = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        let block_size = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        let inode_count = u32::from_le_bytes(buf[16..20].try_into().unwrap());
        let inode_bitmap_block = u32::from_le_bytes(buf[20..24].try_into().unwrap());
        let data_bitmap_block = u32::from_le_bytes(buf[24..28].try_into().unwrap());
        let inode_table_block = u32::from_le_bytes(buf[28..32].try_into().unwrap());
        let data_blocks_start = u32::from_le_bytes(buf[32..36].try_into().unwrap());
        let root_inode_id = u32::from_le_bytes(buf[36..40].try_into().unwrap());

        Ok(Self {
            magic,
            version,
            total_blocks,
            block_size,
            inode_count,
            inode_bitmap_block,
            data_bitmap_block,
            inode_table_block,
            data_blocks_start,
            root_inode_id,
        })
    }
}

/// 磁盘上每个 Inode 占 64 字节
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DiskInode {
    pub inode_type: u16, // 0: Free, 1: Regular, 2: Directory, 3: Device, 4: Pipe
    pub permissions: u16,
    pub size: u32,
    pub direct_blocks: [u32; DIRECT_BLOCKS_COUNT],
    pub indirect_block: u32,
    pub direct_blocks_used: u16,
    pub indirect_blocks_used: u16,
}

impl DiskInode {
    pub fn encode(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..2].copy_from_slice(&self.inode_type.to_le_bytes());
        buf[2..4].copy_from_slice(&self.permissions.to_le_bytes());
        buf[4..8].copy_from_slice(&self.size.to_le_bytes());
        for i in 0..DIRECT_BLOCKS_COUNT {
            let offset = 8 + i * 4;
            buf[offset..offset + 4].copy_from_slice(&self.direct_blocks[i].to_le_bytes());
        }
        buf[56..60].copy_from_slice(&self.indirect_block.to_le_bytes());
        buf[60..62].copy_from_slice(&self.direct_blocks_used.to_le_bytes());
        buf[62..64].copy_from_slice(&self.indirect_blocks_used.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8; 64]) -> Self {
        let inode_type = u16::from_le_bytes(buf[0..2].try_into().unwrap());
        let permissions = u16::from_le_bytes(buf[2..4].try_into().unwrap());
        let size = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        let mut direct_blocks = [0u32; DIRECT_BLOCKS_COUNT];
        for (i, block) in direct_blocks.iter_mut().enumerate() {
            let offset = 8 + i * 4;
            *block = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        }
        let indirect_block = u32::from_le_bytes(buf[56..60].try_into().unwrap());
        let direct_blocks_used = u16::from_le_bytes(buf[60..62].try_into().unwrap());
        let indirect_blocks_used = u16::from_le_bytes(buf[62..64].try_into().unwrap());

        Self {
            inode_type,
            permissions,
            size,
            direct_blocks,
            indirect_block,
            direct_blocks_used,
            indirect_blocks_used,
        }
    }
}

pub const DISK_BLOCK_LIMIT: usize = BLOCK_SIZE * 8;
const DIRENT_SIZE: usize = 64;

fn validate_device(dev: &VirtualBlockDevice) -> Result<(), &'static str> {
    if dev.num_blocks != dev.blocks.len() {
        return Err("Block device geometry does not match its storage");
    }
    if dev.num_blocks <= DATA_BLOCKS_START {
        return Err("Device too small for filesystem format");
    }
    if dev.num_blocks > DISK_BLOCK_LIMIT {
        return Err("Device exceeds filesystem bitmap capacity (4096 blocks)");
    }
    Ok(())
}

fn superblock(total_blocks: usize, root_inode_id: usize) -> DiskSuperblock {
    DiskSuperblock {
        magic: DISK_MAGIC,
        version: DISK_VERSION,
        total_blocks: total_blocks as u32,
        block_size: BLOCK_SIZE as u32,
        inode_count: DISK_INODE_COUNT as u32,
        inode_bitmap_block: INODE_BITMAP_BLOCK as u32,
        data_bitmap_block: DATA_BITMAP_BLOCK as u32,
        inode_table_block: INODE_TABLE_START as u32,
        data_blocks_start: DATA_BLOCKS_START as u32,
        root_inode_id: root_inode_id as u32,
    }
}

fn raw_type(inode_type: InodeType) -> u16 {
    match inode_type {
        InodeType::Regular => 1,
        InodeType::Directory => 2,
        InodeType::Device => 3,
        InodeType::Pipe => 4,
    }
}

fn decode_type(raw: u16) -> Result<InodeType, &'static str> {
    match raw {
        1 => Ok(InodeType::Regular),
        2 => Ok(InodeType::Directory),
        3 => Ok(InodeType::Device),
        4 => Ok(InodeType::Pipe),
        _ => Err("Invalid inode type in filesystem metadata"),
    }
}

fn bitmap_get(bitmap: &[u8; BLOCK_SIZE], index: usize) -> bool {
    bitmap[index / 8] & (1 << (index % 8)) != 0
}

fn bitmap_set(bitmap: &mut [u8; BLOCK_SIZE], index: usize) {
    bitmap[index / 8] |= 1 << (index % 8);
}

/// Format a device after validating every layout bound, before any mutation.
pub fn format_disk(dev: &mut VirtualBlockDevice) -> Result<(), &'static str> {
    validate_device(dev)?;
    dev.write_block(SUPERBLOCK_BLOCK, &superblock(dev.num_blocks, 0).encode())?;
    let mut inode_bitmap = [0u8; BLOCK_SIZE];
    bitmap_set(&mut inode_bitmap, 0);
    dev.write_block(INODE_BITMAP_BLOCK, &inode_bitmap)?;
    let mut data_bitmap = [0u8; BLOCK_SIZE];
    for block in 0..=DATA_BLOCKS_START {
        bitmap_set(&mut data_bitmap, block);
    }
    dev.write_block(DATA_BITMAP_BLOCK, &data_bitmap)?;
    for block in 0..INODE_TABLE_BLOCKS {
        dev.write_block(INODE_TABLE_START + block, &[0u8; BLOCK_SIZE])?;
    }
    let mut root = DiskInode {
        inode_type: 2,
        permissions: 0o755,
        size: 128,
        direct_blocks: [0; DIRECT_BLOCKS_COUNT],
        indirect_block: 0,
        direct_blocks_used: 1,
        indirect_blocks_used: 0,
    };
    root.direct_blocks[0] = DATA_BLOCKS_START as u32;
    let mut inode_block = [0u8; BLOCK_SIZE];
    inode_block[..64].copy_from_slice(&root.encode());
    dev.write_block(INODE_TABLE_START, &inode_block)?;
    let mut directory_block = [0u8; BLOCK_SIZE];
    write_dir_entry(&mut directory_block, 0, 0, 2, ".")?;
    write_dir_entry(&mut directory_block, 1, 0, 2, "..")?;
    dev.write_block(DATA_BLOCKS_START, &directory_block)?;
    Ok(())
}

fn write_dir_entry(
    buf: &mut [u8],
    slot: usize,
    inode_id: u32,
    inode_type: u8,
    name: &str,
) -> Result<(), &'static str> {
    if name.is_empty() || name.contains('/') || name.len() > super::dir::MAX_NAME_BYTES {
        return Err("Invalid directory entry name (maximum 58 bytes)");
    }
    let offset = slot * DIRENT_SIZE;
    if offset + DIRENT_SIZE > buf.len() {
        return Err("Directory entry does not fit its destination");
    }
    buf[offset..offset + 4].copy_from_slice(&inode_id.to_le_bytes());
    buf[offset + 4] = inode_type;
    buf[offset + 5] = name.len() as u8;
    buf[offset + 6..offset + 6 + name.len()].copy_from_slice(name.as_bytes());
    Ok(())
}

fn read_dir_entry(buf: &[u8; BLOCK_SIZE], slot: usize) -> Result<Option<DirEntry>, &'static str> {
    let offset = slot * DIRENT_SIZE;
    let record = &buf[offset..offset + DIRENT_SIZE];
    let name_len = record[5] as usize;
    if name_len == 0 {
        if record.iter().any(|byte| *byte != 0) {
            return Err("Invalid empty directory entry");
        }
        return Ok(None);
    }
    if name_len > super::dir::MAX_NAME_BYTES {
        return Err("Invalid directory entry length");
    }
    let name = std::str::from_utf8(&record[6..6 + name_len])
        .map_err(|_| "Invalid UTF-8 in directory entry")?;
    if name.contains('/') {
        return Err("Invalid directory entry name");
    }
    Ok(Some(DirEntry {
        name: name.to_string(),
        inode_id: u32::from_le_bytes(record[..4].try_into().unwrap()) as usize,
        inode_type: decode_type(record[4] as u16)?,
    }))
}

fn check_data_block(dev: &VirtualBlockDevice, block: usize) -> Result<(), &'static str> {
    if !(DATA_BLOCKS_START..dev.num_blocks).contains(&block) {
        return Err("Inode references a reserved or out-of-range block");
    }
    Ok(())
}

fn inode_data_blocks(
    dev: &mut VirtualBlockDevice,
    inode: &Inode,
) -> Result<Vec<usize>, &'static str> {
    if inode.direct_blocks_used > DIRECT_BLOCKS_COUNT
        || inode.indirect_blocks_used > super::inode::PTRS_PER_INDIRECT_BLOCK
        || (inode.indirect_blocks_used > 0
            && (inode.direct_blocks_used != DIRECT_BLOCKS_COUNT || inode.indirect_block.is_none()))
    {
        return Err("Invalid inode block counters");
    }
    let allocated = inode.direct_blocks_used + inode.indirect_blocks_used;
    if inode.size > allocated * BLOCK_SIZE {
        return Err("Inode size exceeds its allocated blocks");
    }
    let mut blocks = inode.direct_blocks[..inode.direct_blocks_used].to_vec();
    if let Some(indirect) = inode.indirect_block {
        check_data_block(dev, indirect)?;
        let mut table = [0u8; BLOCK_SIZE];
        dev.read_block(indirect, &mut table)?;
        for index in 0..inode.indirect_blocks_used {
            blocks.push(
                u32::from_le_bytes(table[index * 4..index * 4 + 4].try_into().unwrap()) as usize,
            );
        }
    }
    for &block in &blocks {
        check_data_block(dev, block)?;
    }
    Ok(blocks)
}

fn allocated_blocks(
    dev: &mut VirtualBlockDevice,
    inodes: &HashMap<usize, Inode>,
) -> Result<HashSet<usize>, &'static str> {
    let mut allocated = HashSet::new();
    for (&id, inode) in inodes {
        if id >= DISK_INODE_COUNT || inode.id != id {
            return Err("Inode ID exceeds the disk inode table or disagrees with its key");
        }
        for block in inode_data_blocks(dev, inode)? {
            if !allocated.insert(block) {
                return Err("Filesystem block is owned by multiple inodes");
            }
        }
        if let Some(indirect) = inode.indirect_block
            && !allocated.insert(indirect)
        {
            return Err("Filesystem block is owned by multiple inodes");
        }
    }
    Ok(allocated)
}

fn validate_namespace(
    inodes: &HashMap<usize, Inode>,
    directories: &HashMap<usize, Directory>,
    root: usize,
    unlinked: &HashSet<usize>,
) -> Result<(), &'static str> {
    if inodes.get(&root).map(|inode| inode.inode_type) != Some(InodeType::Directory)
        || unlinked.contains(&root)
    {
        return Err("Filesystem root is not a directory");
    }
    for (&id, inode) in inodes {
        if inode.inode_type == InodeType::Directory
            && !unlinked.contains(&id)
            && !directories.contains_key(&id)
        {
            return Err("Directory inode has no directory contents");
        }
    }
    for (&id, dir) in directories {
        if dir.inode_id != id
            || inodes.get(&id).map(|inode| inode.inode_type) != Some(InodeType::Directory)
        {
            return Err("Directory contents do not match their inode");
        }
        if dir.entries.len() > super::inode::MAX_FILE_BLOCKS * BLOCK_SIZE / DIRENT_SIZE {
            return Err("Directory exceeds the supported block capacity");
        }
        if dir.lookup(".").map(|entry| entry.inode_id) != Some(id) || dir.lookup("..").is_none() {
            return Err("Directory lacks valid . and .. entries");
        }
        for (name, entry) in &dir.entries {
            if name != &entry.name
                || name.is_empty()
                || name.contains('/')
                || name.len() > super::dir::MAX_NAME_BYTES
            {
                return Err("Invalid directory entry name (maximum 58 bytes)");
            }
            if inodes.get(&entry.inode_id).map(|inode| inode.inode_type) != Some(entry.inode_type)
                || unlinked.contains(&entry.inode_id)
            {
                return Err("Directory entry references an invalid inode");
            }
        }
        if dir.lookup("..").unwrap().inode_type != InodeType::Directory {
            return Err("Directory parent is not a directory");
        }
        if id == root && dir.lookup("..").unwrap().inode_id != root {
            return Err("Root parent does not refer to root");
        }
    }
    Ok(())
}

/// Mount only a fully validated image. Corrupt counters and pointers return errors,
/// rather than indexing arrays or rebuilding an inconsistent allocation pool.
pub fn mount_filesystem(
    mut dev: VirtualBlockDevice,
    cache_capacity: usize,
) -> Result<VirtualFileSystem, &'static str> {
    // Preserve the useful magic diagnostic for an unformatted device.
    let mut sb_buf = [0u8; BLOCK_SIZE];
    if dev.num_blocks != dev.blocks.len() {
        return Err("Block device geometry does not match its storage");
    }
    dev.read_block(SUPERBLOCK_BLOCK, &mut sb_buf)?;
    let sb = DiskSuperblock::decode(&sb_buf)?;
    validate_device(&dev)?;
    if sb != superblock(dev.num_blocks, sb.root_inode_id as usize)
        || sb.root_inode_id as usize >= DISK_INODE_COUNT
    {
        return Err("Invalid filesystem superblock layout");
    }
    let mut inode_bitmap = [0u8; BLOCK_SIZE];
    let mut data_bitmap = [0u8; BLOCK_SIZE];
    dev.read_block(INODE_BITMAP_BLOCK, &mut inode_bitmap)?;
    dev.read_block(DATA_BITMAP_BLOCK, &mut data_bitmap)?;
    if (DISK_INODE_COUNT..DISK_BLOCK_LIMIT).any(|id| bitmap_get(&inode_bitmap, id)) {
        return Err("Inode bitmap exceeds the inode table capacity");
    }
    let mut inodes = HashMap::new();
    for block in 0..INODE_TABLE_BLOCKS {
        let mut buf = [0u8; BLOCK_SIZE];
        dev.read_block(INODE_TABLE_START + block, &mut buf)?;
        for slot in 0..INODES_PER_BLOCK {
            let id = block * INODES_PER_BLOCK + slot;
            let offset = slot * 64;
            let disk = DiskInode::decode(buf[offset..offset + 64].try_into().unwrap());
            if bitmap_get(&inode_bitmap, id) != (disk.inode_type != 0) {
                return Err("Inode bitmap disagrees with the inode table");
            }
            if disk.inode_type == 0 {
                continue;
            }
            inodes.insert(
                id,
                Inode {
                    id,
                    inode_type: decode_type(disk.inode_type)?,
                    size: disk.size as usize,
                    direct_blocks: disk.direct_blocks.map(|block| block as usize),
                    direct_blocks_used: disk.direct_blocks_used as usize,
                    indirect_block: (disk.indirect_block != 0)
                        .then_some(disk.indirect_block as usize),
                    indirect_blocks_used: disk.indirect_blocks_used as usize,
                    permissions: disk.permissions,
                    created_at: 0,
                    modified_at: 0,
                },
            );
        }
    }
    let allocated = allocated_blocks(&mut dev, &inodes)?;
    for block in 0..DISK_BLOCK_LIMIT {
        let expected = block < DATA_BLOCKS_START || allocated.contains(&block);
        if bitmap_get(&data_bitmap, block) != expected {
            return Err("Data bitmap disagrees with allocated blocks");
        }
    }
    let mut directories = HashMap::new();
    for (&id, inode) in &inodes {
        if inode.inode_type != InodeType::Directory {
            continue;
        }
        let mut entries = HashMap::new();
        for block in inode_data_blocks(&mut dev, inode)? {
            let mut buf = [0u8; BLOCK_SIZE];
            dev.read_block(block, &mut buf)?;
            for slot in 0..BLOCK_SIZE / DIRENT_SIZE {
                if let Some(entry) = read_dir_entry(&buf, slot)?
                    && entries.insert(entry.name.clone(), entry).is_some()
                {
                    return Err("Duplicate directory entry in disk image");
                }
            }
        }
        directories.insert(
            id,
            Directory {
                inode_id: id,
                entries,
            },
        );
    }
    let root_inode_id = sb.root_inode_id as usize;
    validate_namespace(&inodes, &directories, root_inode_id, &HashSet::new())?;
    let next_inode_id = inodes.keys().copied().max().unwrap_or(0) + 1;
    let free_blocks = (DATA_BLOCKS_START..dev.num_blocks)
        .filter(|block| !allocated.contains(block))
        .collect();
    Ok(VirtualFileSystem {
        dev,
        cache: BufferCache::new(cache_capacity),
        inodes,
        directories,
        free_blocks,
        open_files: HashMap::new(),
        unlinked_inodes: HashSet::new(),
        next_inode_id,
        next_fd: 10,
        root_inode_id,
    })
}

fn commit_staged(vfs: &mut VirtualFileSystem) -> Result<(), &'static str> {
    validate_namespace(
        &vfs.inodes,
        &vfs.directories,
        vfs.root_inode_id,
        &vfs.unlinked_inodes,
    )?;
    vfs.sync()?;
    let allocated = allocated_blocks(&mut vfs.dev, &vfs.inodes)?;
    let free: HashSet<_> = vfs.free_blocks.iter().copied().collect();
    if free.len() != vfs.free_blocks.len()
        || free
            .iter()
            .any(|block| !(DATA_BLOCKS_START..vfs.dev.num_blocks).contains(block))
        || (DATA_BLOCKS_START..vfs.dev.num_blocks)
            .any(|block| allocated.contains(&block) == free.contains(&block))
    {
        return Err("Free block pool disagrees with inode allocations");
    }

    // Reclaim all previous directory blocks in the temporary state before
    // rebuilding any directory, so one shrinking directory can fund another.
    let mut directory_ids: Vec<_> = vfs.directories.keys().copied().collect();
    directory_ids.sort_unstable();
    for &id in &directory_ids {
        let inode = vfs.inodes.get_mut(&id).unwrap();
        let old_blocks = inode.release_all_blocks(&mut vfs.cache, &mut vfs.dev);
        for &block in &old_blocks {
            vfs.cache.invalidate(block);
        }
        vfs.free_blocks.extend(old_blocks);
    }
    for &id in &directory_ids {
        let dir = &vfs.directories[&id];
        let mut entries: Vec<_> = dir.entries.values().collect();
        entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        let mut bytes = vec![0u8; entries.len() * DIRENT_SIZE];
        for (slot, entry) in entries.into_iter().enumerate() {
            write_dir_entry(
                &mut bytes,
                slot,
                entry.inode_id as u32,
                raw_type(entry.inode_type) as u8,
                &entry.name,
            )?;
        }
        vfs.inodes.get_mut(&id).unwrap().write_bytes(
            &mut vfs.cache,
            &mut vfs.dev,
            &mut vfs.free_blocks,
            0,
            &bytes,
        )?;
    }
    vfs.sync()?;

    // Unlinked files stay alive for their current descriptors, but a reboot has
    // no descriptors. Their blocks must be free in the persisted image.
    let disk_inodes: HashMap<_, _> = vfs
        .inodes
        .iter()
        .filter(|(id, _)| !vfs.unlinked_inodes.contains(id))
        .map(|(&id, inode)| (id, inode.clone()))
        .collect();
    let allocated = allocated_blocks(&mut vfs.dev, &disk_inodes)?;
    let mut inode_blocks = vec![[0u8; BLOCK_SIZE]; INODE_TABLE_BLOCKS];
    let mut inode_bitmap = [0u8; BLOCK_SIZE];
    for (&id, inode) in &disk_inodes {
        bitmap_set(&mut inode_bitmap, id);
        let disk = DiskInode {
            inode_type: raw_type(inode.inode_type),
            permissions: inode.permissions,
            size: inode.size as u32,
            direct_blocks: inode.direct_blocks.map(|block| block as u32),
            indirect_block: inode.indirect_block.unwrap_or(0) as u32,
            direct_blocks_used: inode.direct_blocks_used as u16,
            indirect_blocks_used: inode.indirect_blocks_used as u16,
        };
        let offset = (id % INODES_PER_BLOCK) * 64;
        inode_blocks[id / INODES_PER_BLOCK][offset..offset + 64].copy_from_slice(&disk.encode());
    }
    let mut data_bitmap = [0u8; BLOCK_SIZE];
    for block in (0..DATA_BLOCKS_START).chain(allocated) {
        bitmap_set(&mut data_bitmap, block);
    }
    vfs.dev.write_block(
        SUPERBLOCK_BLOCK,
        &superblock(vfs.dev.num_blocks, vfs.root_inode_id).encode(),
    )?;
    vfs.dev.write_block(INODE_BITMAP_BLOCK, &inode_bitmap)?;
    for (index, block) in inode_blocks.iter().enumerate() {
        vfs.dev.write_block(INODE_TABLE_START + index, block)?;
    }
    vfs.dev.write_block(DATA_BITMAP_BLOCK, &data_bitmap)?;
    Ok(())
}

/// Build a complete image and updated directory allocation in temporary memory.
/// Failure leaves the caller's disk, cache and allocation state unchanged. This
/// protects simulator operations; it is not a journal for host-file power loss.
pub fn commit_vfs_to_disk(vfs: &mut VirtualFileSystem) -> Result<(), &'static str> {
    validate_device(&vfs.dev)?;
    let mut staged = vfs.clone();
    commit_staged(&mut staged)?;
    *vfs = staged;
    Ok(())
}
