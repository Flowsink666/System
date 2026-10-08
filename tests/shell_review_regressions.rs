use mini_os_kernel::fs::{format_disk, mount_filesystem, VirtualBlockDevice, O_CREAT, O_RDWR};
use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::sched::FileDescriptorEntry;
use mini_os_kernel::shell::KernelShell;
use mini_os_kernel::syscall::{SyscallArgs, SYS_READ};

#[test]
fn shell_load_must_not_alias_an_existing_process_file_descriptor() {
    let mut kernel = Kernel::new(64, 256, 16);
    let pid = kernel.pm.current_pid.unwrap();
    let old_vfs_fd = kernel.vfs.open("/tmp/old", O_CREAT | O_RDWR).unwrap();
    kernel.vfs.write(old_vfs_fd, b"old").unwrap();
    let old_user_fd = kernel.pm.processes.get_mut(&pid).unwrap()
        .alloc_fd(FileDescriptorEntry::new_vfs(old_vfs_fd, O_RDWR));

    let mut dev = VirtualBlockDevice::new(256);
    format_disk(&mut dev).unwrap();
    let mut replacement = mount_filesystem(dev, 16).unwrap();
    let fd = replacement.open("/new", O_CREAT | O_RDWR).unwrap();
    replacement.write(fd, b"replacement").unwrap();
    replacement.close(fd).unwrap();
    replacement.commit_to_disk().unwrap();
    std::fs::create_dir_all("target/shell-review").unwrap();
    let image_path = format!("target/shell-review/load-{}.img", std::process::id());
    replacement.dev.save_to_file(&image_path).unwrap();
    KernelShell::new(&mut kernel).execute_line(&format!("load {}", image_path));
    std::fs::remove_file(&image_path).unwrap();
    assert!(kernel.vfs.stat("/new").is_err(), "busy filesystem load must be rejected");
    kernel.vfs.seek(old_vfs_fd, 0).unwrap();
    let result = kernel.syscall(SyscallArgs {
        num: SYS_READ, arg0: old_user_fd, arg1: 0x8000, arg2: 11, arg3: 0,
    });
    let mut observed = [0u8; 11];
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    kernel.mm.read_virtual(&mut proc.address_space, 0x8000, &mut observed).unwrap();
    println!("old_vfs_fd={old_vfs_fd}, result={result:?}, observed={:?}", std::str::from_utf8(&observed));
    assert!(result.is_err() || &observed[..3] == b"old", "old FD must never read an unrelated replacement file");
}

#[test]
fn failed_shell_exec_must_not_leave_a_runnable_process() {
    let mut kernel = Kernel::new(64, 256, 16);
    let before = kernel.pm.processes.len();
    KernelShell::new(&mut kernel).execute_line("exec /does-not-exist");
    let after = kernel.pm.processes.len();
    println!("process_count before={before}, after={after}");
    assert_eq!(after, before, "failed exec must roll back its newly spawned process");
}

#[test]
fn image_load_succeeds_after_all_file_references_are_closed() {
    let mut kernel = Kernel::new(64, 256, 16);
    let mut dev = VirtualBlockDevice::new(256);
    format_disk(&mut dev).unwrap();
    let fd = kernel.vfs.open("/tmp/open", O_CREAT | O_RDWR).unwrap();
    assert!(kernel.load_filesystem(dev.clone()).is_err());
    kernel.vfs.close(fd).unwrap();
    kernel.load_filesystem(dev).unwrap();
    assert!(kernel.vfs.stat("/tmp/open").is_err());
    assert!(kernel.vfs.stat("/").is_ok());
}

#[test]
fn exited_file_mapping_must_not_keep_the_filesystem_busy() {
    use mini_os_kernel::syscall::{MAP_PRIVATE, PROT_READ};
    let mut kernel = Kernel::new(16, 64, 8);
    let child = kernel.fork_process_cow(1).unwrap();
    let fd = kernel.vfs.open("/tmp/mapped", O_CREAT | O_RDWR).unwrap();
    kernel.vfs.write(fd, b"DATA").unwrap();
    kernel.mmap(child, 0, 4096, PROT_READ, MAP_PRIVATE, Some(fd), 0).unwrap();
    kernel.vfs.close(fd).unwrap();
    kernel.terminate_process(child, 7).unwrap();
    assert!(kernel.vfs.open_files.is_empty());
    assert!(kernel.pm.processes[&child].vma_list.is_empty());
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 2);
    let mut replacement = VirtualBlockDevice::new(64);
    format_disk(&mut replacement).unwrap();
    kernel.load_filesystem(replacement).unwrap();
}

#[test]
fn image_load_preserves_running_processes_with_non_vfs_handles() {
    let mut kernel = Kernel::new(64, 256, 16);
    let pipe_id = kernel.pm.create_pipe(4096);
    let r_entry = FileDescriptorEntry::new_pipe_read(pipe_id);
    let w_entry = FileDescriptorEntry::new_pipe_write(pipe_id);
    let pid = kernel.pm.spawn("pipe_worker", 0, 100);
    let proc = kernel.pm.processes.get_mut(&pid).unwrap();
    let _r_fd = proc.alloc_fd(r_entry);
    let _w_fd = proc.alloc_fd(w_entry);

    let mut dev = VirtualBlockDevice::new(256);
    format_disk(&mut dev).unwrap();
    let mut replacement = mount_filesystem(dev, 16).unwrap();
    let fd = replacement.open("/service.conf", O_CREAT | O_RDWR).unwrap();
    replacement.write(fd, b"active").unwrap();
    replacement.close(fd).unwrap();
    replacement.commit_to_disk().unwrap();

    kernel.load_filesystem(replacement.dev).expect("non-VFS process handles must not block load");

    kernel.pm.pipes.get_mut(&pipe_id).unwrap().write(b"HELLO").unwrap();
    let mut pipe_buf = [0u8; 5];
    let n = kernel.pm.pipes.get_mut(&pipe_id).unwrap().read(&mut pipe_buf);
    assert_eq!(&pipe_buf[..n], b"HELLO");
    assert_eq!(kernel.pm.processes[&pid].state, mini_os_kernel::sched::ProcessState::Ready);
    assert!(kernel.vfs.stat("/service.conf").is_ok());
}

#[test]
fn image_load_corrupted_device_preserves_current_filesystem() {
    let mut kernel = Kernel::new(64, 256, 16);
    let fd = kernel.vfs.open("/tmp/important.txt", O_CREAT | O_RDWR).unwrap();
    kernel.vfs.write(fd, b"critical data").unwrap();
    kernel.vfs.close(fd).unwrap();

    let corrupted_dev = VirtualBlockDevice::new(64);
    let load_res = kernel.load_filesystem(corrupted_dev);
    assert!(load_res.is_err(), "mounting unformatted device must fail");

    assert!(kernel.vfs.stat("/tmp/important.txt").is_ok());
    let fd = kernel.vfs.open("/tmp/important.txt", O_RDWR).unwrap();
    let mut buf = [0u8; 13];
    let n = kernel.vfs.read(fd, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"critical data");
}

#[test]
fn image_load_rejection_keeps_all_open_files_and_offsets_intact() {
    let mut kernel = Kernel::new(64, 256, 16);
    let fd = kernel.vfs.open("/tmp/stream.txt", O_CREAT | O_RDWR).unwrap();
    kernel.vfs.write(fd, b"0123456789").unwrap();
    kernel.vfs.seek(fd, 4).unwrap();

    let mut dev = VirtualBlockDevice::new(256);
    format_disk(&mut dev).unwrap();
    let res = kernel.load_filesystem(dev);
    assert!(res.is_err(), "must be rejected because stream.txt is open");

    let mut buf = [0u8; 6];
    let n = kernel.vfs.read(fd, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"456789");
    assert_eq!(kernel.vfs.open_files[&fd].offset, 10);
}

