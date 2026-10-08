//! 块设备磁盘镜像二进制布局与持久化冷启动恢复测试
//! (Persistence & Cold-Reboot Verification Suite)

use mini_os_kernel::fs::{
    format_disk, mount_filesystem, InodeType, VirtualBlockDevice, O_CREAT, O_RDONLY, O_RDWR,
    O_TRUNC,
};
use mini_os_kernel::kernel::Kernel;

#[test]
fn test_disk_format_and_mount_empty() {
    let mut dev = VirtualBlockDevice::new(1024);
    format_disk(&mut dev).expect("format_disk should succeed");

    let vfs = mount_filesystem(dev, 32).expect("mount_filesystem should succeed");

    assert_eq!(vfs.root_inode_id, 0);
    let root_inode = vfs.inodes.get(&0).expect("root inode must exist");
    assert_eq!(root_inode.inode_type, InodeType::Directory);

    let entries = vfs.list_dir("/").expect("list_dir root should succeed");
    let names: Vec<String> = entries.into_iter().map(|(n, _)| n).collect();
    assert!(names.contains(&".".to_string()));
    assert!(names.contains(&"..".to_string()));
}

#[test]
fn test_vfs_file_commit_and_cold_reboot() {
    let mut dev = VirtualBlockDevice::new(1024);
    format_disk(&mut dev).expect("format_disk failed");
    let mut vfs = mount_filesystem(dev, 32).expect("mount failed");

    // 1. 创建子目录与文件
    vfs.mkdir("/docs").expect("mkdir /docs failed");
    vfs.mkdir("/data").expect("mkdir /data failed");

    // 写入常规短文件
    let fd1 = vfs
        .open("/docs/readme.txt", O_CREAT | O_RDWR | O_TRUNC)
        .expect("open readme.txt failed");
    let readme_content = b"Welcome to Mini-OS Persistent File System!";
    vfs.write(fd1, readme_content).expect("write failed");
    vfs.close(fd1).expect("close fd1 failed");

    // 写入跨多块大文件 (2500 字节，跨 5 个 512B 块)
    let fd2 = vfs
        .open("/data/blob.bin", O_CREAT | O_RDWR | O_TRUNC)
        .expect("open blob.bin failed");
    let mut blob_data = Vec::with_capacity(2500);
    for i in 0..2500 {
        blob_data.push((i % 251) as u8);
    }
    vfs.write(fd2, &blob_data).expect("write blob failed");
    vfs.close(fd2).expect("close fd2 failed");

    // 2. 提交元数据与脏缓存到底层磁盘
    vfs.commit_to_disk().expect("commit_to_disk failed");

    // 3. 模拟物理机断电关机：通过字节流序列化和物理拷贝完全销毁旧内存状态
    let raw_disk_bytes = vfs.dev.to_bytes();
    drop(vfs);

    // 4. 冷启动开机：从全新块设备恢复挂载
    let cold_dev = VirtualBlockDevice::from_bytes(&raw_disk_bytes).expect("from_bytes failed");
    let mut rebooted_vfs = mount_filesystem(cold_dev, 32).expect("mount rebooted failed");

    // 5. 校验目录结构
    let root_entries = rebooted_vfs.list_dir("/").expect("list / failed");
    let root_names: Vec<String> = root_entries.into_iter().map(|(n, _)| n).collect();
    assert!(root_names.contains(&"docs".to_string()));
    assert!(root_names.contains(&"data".to_string()));

    // 校验 readme.txt
    let r_fd1 = rebooted_vfs
        .open("/docs/readme.txt", O_RDONLY)
        .expect("open /docs/readme.txt after reboot failed");
    let mut read_buf1 = vec![0u8; 100];
    let n1 = rebooted_vfs.read(r_fd1, &mut read_buf1).expect("read failed");
    assert_eq!(&read_buf1[..n1], readme_content);
    rebooted_vfs.close(r_fd1).expect("close r_fd1 failed");

    // 校验 blob.bin 逐字节一致
    let r_fd2 = rebooted_vfs
        .open("/data/blob.bin", O_RDONLY)
        .expect("open /data/blob.bin after reboot failed");
    let mut read_buf2 = vec![0u8; 3000];
    let n2 = rebooted_vfs.read(r_fd2, &mut read_buf2).expect("read blob failed");
    assert_eq!(n2, 2500);
    assert_eq!(&read_buf2[..n2], &blob_data[..]);
    rebooted_vfs.close(r_fd2).expect("close r_fd2 failed");

    // 6. 冷启动后继续分配新文件，验证 Inode 位图与块位图恢复一致性
    let fd3 = rebooted_vfs
        .open("/data/new.txt", O_CREAT | O_RDWR | O_TRUNC)
        .expect("open /data/new.txt failed");
    rebooted_vfs.write(fd3, b"Post-reboot data").expect("write new failed");
    rebooted_vfs.close(fd3).expect("close fd3 failed");

    let r_fd3 = rebooted_vfs.open("/data/new.txt", O_RDONLY).expect("open new failed");
    let mut read_buf3 = [0u8; 32];
    let n3 = rebooted_vfs.read(r_fd3, &mut read_buf3).expect("read new failed");
    assert_eq!(&read_buf3[..n3], b"Post-reboot data");
    rebooted_vfs.close(r_fd3).expect("close r_fd3 failed");
}

#[test]
fn test_disk_save_to_file_and_load_from_file() {
    let temp_path = std::env::temp_dir().join(format!("minios_disk_test_{}.img", std::process::id()));

    let mut dev = VirtualBlockDevice::new(512);
    format_disk(&mut dev).expect("format failed");
    let mut vfs = mount_filesystem(dev, 16).expect("mount failed");

    let fd = vfs.open("/persisted.txt", O_CREAT | O_RDWR | O_TRUNC).expect("open failed");
    vfs.write(fd, b"Host file system export test").expect("write failed");
    vfs.close(fd).expect("close failed");

    vfs.commit_to_disk().expect("commit failed");

    // 保存到宿主机真实物理文件
    vfs.dev.save_to_file(&temp_path).expect("save_to_file failed");
    drop(vfs);

    // 从物理文件重新加载
    let loaded_dev = VirtualBlockDevice::load_from_file(&temp_path).expect("load_from_file failed");
    let mut rebooted_vfs = mount_filesystem(loaded_dev, 16).expect("mount failed");

    let r_fd = rebooted_vfs.open("/persisted.txt", O_RDONLY).expect("open failed");
    let mut buf = [0u8; 64];
    let n = rebooted_vfs.read(r_fd, &mut buf).expect("read failed");
    assert_eq!(&buf[..n], b"Host file system export test");
    rebooted_vfs.close(r_fd).expect("close failed");

    // 清理临时文件
    let _ = std::fs::remove_file(&temp_path);
}

#[test]
fn test_disk_metadata_corruption_recovery() {
    let dev = VirtualBlockDevice::new(512); // 未格式化的全零设备
    let res = mount_filesystem(dev, 16);
    assert!(res.is_err());
    assert_eq!(res.err(), Some("Invalid filesystem magic"));
}

#[test]
fn test_kernel_commit_and_from_disk_reboot() {
    // 1. 初始化内核，写入文件
    let mut kernel = Kernel::new(256, 1024, 32);

    let fd = kernel
        .vfs
        .open("/etc/custom.conf", O_CREAT | O_RDWR | O_TRUNC)
        .expect("open custom.conf failed");
    kernel.vfs.write(fd, b"mode=production\nthreads=4").expect("write failed");
    kernel.vfs.close(fd).expect("close failed");

    // 2. 内核完整落盘
    kernel.commit_disk().expect("kernel commit_disk failed");

    let disk_bytes = kernel.vfs.dev.to_bytes();
    drop(kernel);

    // 3. 冷启动恢复全新内核
    let recovered_dev = VirtualBlockDevice::from_bytes(&disk_bytes).expect("from_bytes failed");
    let mut new_kernel = Kernel::from_disk(256, recovered_dev, 32).expect("from_disk failed");

    // 4. 验证用户自定义文件完整恢复
    let r_fd = new_kernel
        .vfs
        .open("/etc/custom.conf", O_RDONLY)
        .expect("open custom.conf in rebooted kernel failed");
    let mut conf_buf = [0u8; 64];
    let n = new_kernel.vfs.read(r_fd, &mut conf_buf).expect("read failed");
    assert_eq!(&conf_buf[..n], b"mode=production\nthreads=4");
    new_kernel.vfs.close(r_fd).expect("close failed");

    // 5. 验证预置的可执行文件 /bin/hello 也从磁盘完整恢复，且可以正常装载执行！
    let exec_pid = new_kernel.pm.spawn("hello_task", 0, 50);
    new_kernel
        .exec_process(exec_pid, "/bin/hello")
        .expect("exec_process after cold reboot failed");
    new_kernel.step(10);
    let output = String::from_utf8_lossy(&new_kernel.console_stdout);
    assert!(
        output.contains("Hello from Mini-OS Exec!"),
        "exec output after reboot must match, got: {}",
        output
    );
}
