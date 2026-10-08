use mini_os_kernel::fs::disk::{DATA_BITMAP_BLOCK, DATA_BLOCKS_START, INODE_TABLE_START};
use mini_os_kernel::fs::{
    BLOCK_SIZE, BufferCache, DISK_INODE_COUNT, DiskInode, Inode, InodeType, O_CREAT, O_RDWR,
    VirtualBlockDevice, VirtualFileSystem, format_disk, mount_filesystem,
};

fn mounted(blocks: usize) -> VirtualFileSystem {
    let mut dev = VirtualBlockDevice::new(blocks);
    format_disk(&mut dev).unwrap();
    mount_filesystem(dev, 8).unwrap()
}

fn reboot(vfs: &VirtualFileSystem) -> VirtualFileSystem {
    mount_filesystem(
        VirtualBlockDevice::from_bytes(&vfs.dev.to_bytes()).unwrap(),
        8,
    )
    .unwrap()
}

fn create_empty(vfs: &mut VirtualFileSystem, name: &str) {
    let fd = vfs.open(name, O_CREAT | O_RDWR).unwrap();
    vfs.close(fd).unwrap();
}

type InodeAllocation = (usize, usize, [usize; 12], usize, Option<usize>, usize);

fn allocations(vfs: &VirtualFileSystem) -> Vec<InodeAllocation> {
    let mut result: Vec<_> = vfs
        .inodes
        .iter()
        .map(|(&id, inode)| {
            (
                id,
                inode.size,
                inode.direct_blocks,
                inode.direct_blocks_used,
                inode.indirect_block,
                inode.indirect_blocks_used,
            )
        })
        .collect();
    result.sort_unstable_by_key(|entry| entry.0);
    result
}

#[test]
fn metadata_blocks_are_never_allocated_as_file_data() {
    let mut vfs = VirtualFileSystem::new(64, 8);
    assert!(
        vfs.free_blocks
            .iter()
            .all(|block| *block >= DATA_BLOCKS_START)
    );
    vfs.commit_to_disk().unwrap();
    for index in 0..5 {
        let fd = vfs.open(&format!("/f{index}"), O_CREAT | O_RDWR).unwrap();
        let blocks = if index == 4 { 4 } else { 6 };
        assert_eq!(
            vfs.write(fd, &vec![index as u8 + 10; blocks * BLOCK_SIZE])
                .unwrap(),
            blocks * BLOCK_SIZE
        );
        vfs.close(fd).unwrap();
    }
    assert!(vfs.free_blocks.is_empty());
    vfs.commit_to_disk().unwrap();
    let mut restored = reboot(&vfs);
    for index in 0..5 {
        let fd = restored.open(&format!("/f{index}"), O_RDWR).unwrap();
        let blocks = if index == 4 { 4 } else { 6 };
        let mut bytes = vec![0; blocks * BLOCK_SIZE];
        assert_eq!(restored.read(fd, &mut bytes).unwrap(), bytes.len());
        assert!(bytes.iter().all(|byte| *byte == index as u8 + 10));
    }
}

#[test]
fn directory_roundtrip_crosses_direct_and_indirect_blocks_and_shrinks() {
    let mut vfs = mounted(512);
    let initial_free = vfs.free_blocks.len();
    for index in 0..110 {
        create_empty(&mut vfs, &format!("/f{index}"));
    }
    vfs.commit_to_disk().unwrap();
    assert!(vfs.inodes[&0].indirect_blocks_used > 0);
    let restored = reboot(&vfs);
    assert_eq!(restored.directories[&0].entries.len(), 112);
    for index in 0..110 {
        restored.stat(&format!("/f{index}")).unwrap();
    }
    restored.resolve_path("/.").unwrap();
    restored.resolve_path("/..").unwrap();
    for index in 1..110 {
        vfs.unlink(&format!("/f{index}")).unwrap();
    }
    vfs.commit_to_disk().unwrap();
    assert_eq!(vfs.free_blocks.len(), initial_free);
    assert_eq!(reboot(&vfs).directories[&0].entries.len(), 3);
}

#[test]
fn full_disk_commit_preserves_device_cache_allocations_and_open_file() {
    let mut vfs = mounted(38);
    let fd = vfs.open("/data", O_CREAT | O_RDWR).unwrap();
    vfs.write(fd, &[42; BLOCK_SIZE]).unwrap();
    vfs.mkdir("/a").unwrap();
    vfs.mkdir("/b").unwrap();
    let disk_before = vfs.dev.to_bytes();
    let free_before = vfs.free_blocks.clone();
    let allocations_before = allocations(&vfs);
    let dirty_before = vfs
        .cache
        .cache
        .values()
        .filter(|block| block.is_dirty)
        .count();
    assert!(dirty_before > 0);
    let cache_stats_before = (vfs.cache.hits, vfs.cache.misses, vfs.cache.writebacks);
    assert!(vfs.commit_to_disk().is_err());
    assert_eq!(vfs.dev.to_bytes(), disk_before);
    assert_eq!(vfs.free_blocks, free_before);
    assert_eq!(allocations(&vfs), allocations_before);
    assert_eq!(vfs.open_files[&fd].offset, BLOCK_SIZE);
    assert_eq!(
        vfs.cache
            .cache
            .values()
            .filter(|block| block.is_dirty)
            .count(),
        dirty_before
    );
    assert_eq!(
        (vfs.cache.hits, vfs.cache.misses, vfs.cache.writebacks),
        cache_stats_before
    );
    // Removing the new empty directories makes the same dirty write committable.
    vfs.unlink("/a").unwrap();
    vfs.unlink("/b").unwrap();
    vfs.commit_to_disk().unwrap();
    let mut restored = reboot(&vfs);
    let fd = restored.open("/data", O_RDWR).unwrap();
    let mut bytes = [0; BLOCK_SIZE];
    restored.read(fd, &mut bytes).unwrap();
    assert_eq!(bytes, [42; BLOCK_SIZE]);
}

#[test]
fn long_utf8_names_fail_before_creation_instead_of_truncating() {
    let mut vfs = mounted(64);
    let valid = format!("/{}a", "界".repeat(19)); // 58 bytes per component.
    create_empty(&mut vfs, &valid);
    let inode_count = vfs.inodes.len();
    let next_inode = vfs.next_inode_id;
    let invalid = format!("{valid}b");
    assert!(vfs.open(&invalid, O_CREAT | O_RDWR).is_err());
    assert!(vfs.mkdir(&invalid).is_err());
    assert_eq!(vfs.inodes.len(), inode_count);
    assert_eq!(vfs.next_inode_id, next_inode);
    vfs.commit_to_disk().unwrap();
    reboot(&vfs).stat(&valid).unwrap();
}

#[test]
fn inode_capacity_is_enforced_and_deleted_ids_can_be_reused() {
    let mut vfs = mounted(512);
    for index in 1..DISK_INODE_COUNT {
        create_empty(&mut vfs, &format!("/f{index}"));
    }
    assert_eq!(vfs.inodes.len(), DISK_INODE_COUNT);
    assert!(vfs.open("/overflow", O_CREAT | O_RDWR).is_err());
    assert!(vfs.mkdir("/overflow").is_err());
    assert_eq!(vfs.inodes.len(), DISK_INODE_COUNT);
    vfs.commit_to_disk().unwrap();
    assert_eq!(
        reboot(&vfs).directories[&0].entries.len(),
        DISK_INODE_COUNT + 1
    );
    let old_id = vfs.resolve_path("/f100").unwrap();
    vfs.unlink("/f100").unwrap();
    create_empty(&mut vfs, "/replacement");
    assert_eq!(vfs.resolve_path("/replacement").unwrap(), old_id);
    vfs.commit_to_disk().unwrap();
    let restored = reboot(&vfs);
    assert!(restored.stat("/f100").is_err());
    restored.stat("/replacement").unwrap();
}

#[test]
fn impossible_device_and_inode_layouts_fail_without_mutation() {
    let mut small = VirtualFileSystem::new(16, 2);
    let fd = small.open("/ram-only", O_CREAT | O_RDWR).unwrap();
    small.write(fd, b"works in the simulator").unwrap();
    let bytes = small.dev.to_bytes();
    assert!(small.commit_to_disk().is_err());
    assert_eq!(small.dev.to_bytes(), bytes);
    small.seek(fd, 0).unwrap();
    let mut buf = [0; 22];
    assert_eq!(small.read(fd, &mut buf).unwrap(), 22);

    let mut oversized = VirtualBlockDevice::new(4097);
    let bytes = oversized.to_bytes();
    assert!(format_disk(&mut oversized).is_err());
    assert_eq!(oversized.to_bytes(), bytes);
    let mut vfs = mounted(64);
    vfs.inodes.insert(
        DISK_INODE_COUNT,
        Inode::new(DISK_INODE_COUNT, InodeType::Regular, 0o644),
    );
    let bytes = vfs.dev.to_bytes();
    assert!(vfs.commit_to_disk().is_err());
    assert_eq!(vfs.dev.to_bytes(), bytes);
}

#[test]
fn corrupted_inode_counters_and_bitmaps_return_mount_errors() {
    let mut dev = VirtualBlockDevice::new(64);
    format_disk(&mut dev).unwrap();
    let valid_bytes = dev.to_bytes();
    let mut inode = DiskInode::decode(dev.blocks[INODE_TABLE_START][..64].try_into().unwrap());
    inode.direct_blocks_used = 13;
    dev.blocks[INODE_TABLE_START][..64].copy_from_slice(&inode.encode());
    assert!(mount_filesystem(dev, 8).is_err());
    let mut dev = VirtualBlockDevice::from_bytes(&valid_bytes).unwrap();
    dev.blocks[DATA_BITMAP_BLOCK][DATA_BLOCKS_START / 8] ^= 1 << (DATA_BLOCKS_START % 8);
    assert!(mount_filesystem(dev, 8).is_err());
}

#[test]
fn unlinked_open_file_survives_current_fd_but_not_a_reboot() {
    let mut vfs = mounted(64);
    let fd = vfs.open("/temporary", O_CREAT | O_RDWR).unwrap();
    vfs.write(fd, b"still open").unwrap();
    vfs.unlink("/temporary").unwrap();
    vfs.commit_to_disk().unwrap();
    vfs.seek(fd, 0).unwrap();
    let mut buf = [0; 10];
    vfs.read(fd, &mut buf).unwrap();
    assert_eq!(&buf, b"still open");
    let restored = reboot(&vfs);
    assert!(restored.stat("/temporary").is_err());
    assert_eq!(restored.inodes.len(), 1);
    assert_eq!(restored.free_blocks.len(), 64 - DATA_BLOCKS_START - 1);
    vfs.close(fd).unwrap();
}

#[test]
fn cache_eviction_is_lru_and_reuses_invalidated_nodes() {
    let mut dev = VirtualBlockDevice::new(4);
    let mut cache = BufferCache::new(2);
    cache.write_block(&mut dev, 0, &[10; BLOCK_SIZE]).unwrap();
    cache.write_block(&mut dev, 1, &[11; BLOCK_SIZE]).unwrap();
    let mut buf = [0; BLOCK_SIZE];
    cache.read_block(&mut dev, 0, &mut buf).unwrap();
    cache.write_block(&mut dev, 2, &[12; BLOCK_SIZE]).unwrap();
    assert!(cache.cache.contains_key(&0));
    assert!(!cache.cache.contains_key(&1));
    assert_eq!(dev.blocks[1], [11; BLOCK_SIZE]);
    cache.invalidate(2);
    cache.write_block(&mut dev, 3, &[13; BLOCK_SIZE]).unwrap();
    cache.sync(&mut dev).unwrap();
    assert_eq!(dev.blocks[0], [10; BLOCK_SIZE]);
    assert_eq!(dev.blocks[3], [13; BLOCK_SIZE]);
}

#[test]
fn failed_cache_writeback_retains_dirty_data_and_lru_position() {
    let mut dev = VirtualBlockDevice::new(2);
    let mut cache = BufferCache::new(1);
    cache.write_block(&mut dev, 1, &[91; BLOCK_SIZE]).unwrap();
    // Make the current dirty block temporarily inaccessible to inject write failure.
    dev.num_blocks = 1;
    let mut buf = [0; BLOCK_SIZE];
    assert!(cache.read_block(&mut dev, 0, &mut buf).is_err());
    assert!(cache.cache[&1].is_dirty);
    assert_eq!(cache.cache[&1].data, [91; BLOCK_SIZE]);
    assert!(!cache.cache.contains_key(&0));
    assert_eq!(cache.writebacks, 0);
    dev.num_blocks = 2;
    cache.read_block(&mut dev, 0, &mut buf).unwrap();
    assert_eq!(dev.blocks[1], [91; BLOCK_SIZE]);
    assert_eq!(cache.writebacks, 1);
}

#[test]
fn zero_capacity_cache_is_passthrough_and_rejects_bad_blocks() {
    let mut dev = VirtualBlockDevice::new(1);
    let mut cache = BufferCache::new(0);
    cache.write_block(&mut dev, 0, &[17; BLOCK_SIZE]).unwrap();
    let mut buf = [0; BLOCK_SIZE];
    cache.read_block(&mut dev, 0, &mut buf).unwrap();
    assert_eq!(buf, [17; BLOCK_SIZE]);
    assert!(cache.cache.is_empty());
    assert!(cache.write_block(&mut dev, 1, &buf).is_err());
    assert!(cache.cache.is_empty());
}

#[test]
fn cache_churn_matches_direct_device_contents() {
    let mut dev = VirtualBlockDevice::new(64);
    let mut expected = vec![[0; BLOCK_SIZE]; 64];
    let mut cache = BufferCache::new(7);
    let mut seed = 19u64;
    for step in 0..4000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let block = (seed >> 32) as usize % 64;
        if step % 3 == 0 {
            let bytes = [step as u8; BLOCK_SIZE];
            cache.write_block(&mut dev, block, &bytes).unwrap();
            expected[block] = bytes;
        } else {
            let mut buf = [0; BLOCK_SIZE];
            cache.read_block(&mut dev, block, &mut buf).unwrap();
            assert_eq!(buf, expected[block]);
        }
        if step % 37 == 0 {
            cache.sync(&mut dev).unwrap();
            cache.invalidate(block);
        }
        assert!(cache.cache.len() <= 7);
    }
    cache.sync(&mut dev).unwrap();
    assert_eq!(dev.blocks, expected);
}
