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
    let image_path = "target/review-20261008-root/load-probe.img";
    replacement.dev.save_to_file(image_path).unwrap();
    KernelShell::new(&mut kernel).execute_line(&format!("load {}", image_path));
    if kernel.vfs.stat("/new").is_ok() {
        for _ in 10..=old_vfs_fd {
            kernel.vfs.open("/new", O_RDWR).unwrap();
        }
    }
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
