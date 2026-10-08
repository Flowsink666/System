use mini_os_kernel::fs::{VirtualBlockDevice, VirtualFileSystem, format_disk, mount_filesystem, O_CREAT, O_RDWR};
use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::sched::{FileDescriptorType, ProcessState};
use mini_os_kernel::syscall::{SyscallArgs, SYS_PIPE, SYS_READ, SYS_WRITE};

fn sys(k: &mut Kernel, num: usize, a: usize, b: usize, c: usize) -> Result<isize, &'static str> {
    k.syscall(SyscallArgs { num, arg0: a, arg1: b, arg2: c, arg3: 0 })
}

fn main() {
    // Constructor advertises first commit support, but allocates metadata blocks as file data.
    let mut vfs = VirtualFileSystem::new(64, 32);
    vfs.commit_to_disk().unwrap();
    for i in 0..5 {
        let fd = vfs.open(&format!("/f{i}"), O_CREAT | O_RDWR).unwrap();
        let blocks = if i == 4 { 4 } else { 6 };
        vfs.write(fd, &vec![b'Q'; blocks * 512]).unwrap();
        vfs.close(fd).unwrap();
    }
    println!("metadata_overlap: last_file_blocks={:?}", vfs.inodes[&5].direct_blocks[..4].to_vec());
    vfs.commit_to_disk().unwrap();
    let dev = VirtualBlockDevice::from_bytes(&vfs.dev.to_bytes()).unwrap();
    let mut restored = mount_filesystem(dev, 32).unwrap();
    let fd = restored.open("/f4", O_RDWR).unwrap();
    let mut data = vec![0; 4 * 512];
    restored.read(fd, &mut data).unwrap();
    println!("metadata_overlap: commit=Ok, read_bytes={}, preserved_Q_bytes={} (expected {})", data.len(), data.iter().filter(|&&b| b == b'Q').count(), data.len());

    let mut dev = VirtualBlockDevice::new(128);
    format_disk(&mut dev).unwrap();
    let mut vfs = mount_filesystem(dev, 32).unwrap();
    for i in 0..7 {
        let fd = vfs.open(&format!("/f{i}"), O_CREAT | O_RDWR).unwrap();
        vfs.close(fd).unwrap();
    }
    let before = vfs.directories[&0].entries.len();
    vfs.commit_to_disk().unwrap();
    let dev = VirtualBlockDevice::from_bytes(&vfs.dev.to_bytes()).unwrap();
    let restored = mount_filesystem(dev, 32).unwrap();
    println!("directory_truncation: commit=Ok, before={}, after={} (expected {})", before, restored.directories[&0].entries.len(), before);

    let mut k = Kernel::new(16, 64, 8);
    let r = sys(&mut k, SYS_PIPE, 0, 8, 0).unwrap();
    let read_fd = (r & 0xffff) as usize;
    let write_fd = ((r >> 16) & 0xffff) as usize;
    let pipe_id = match k.pm.processes[&1].fd_table[&read_fd].descriptor_type() {
        FileDescriptorType::PipeRead(id) => id,
        _ => unreachable!(),
    };
    k.pm.pipes.get_mut(&pipe_id).unwrap().write(b"12345678").unwrap();
    let child = k.fork_process(1).unwrap();
    k.pm.current_pid = Some(child);
    assert_eq!(sys(&mut k, SYS_WRITE, write_fd, 0x8000, 1).unwrap(), -1);
    assert!(matches!(k.pm.processes[&child].state, ProcessState::Blocked(_)));
    k.pm.current_pid = Some(1);
    let bad_read = sys(&mut k, SYS_READ, read_fd, 0xa000, 8);
    println!("pipe_bad_read: result={bad_read:?}, remaining={} (expected 8), writer_state={:?}", k.pm.pipes[&pipe_id].buffer.len(), k.pm.processes[&child].state);

    k.console_stdin.extend(b"INPUT".iter().copied());
    let bad_read = sys(&mut k, SYS_READ, 0, 0xa000, 5);
    println!("stdin_bad_read: result={bad_read:?}, remaining={} (expected 5)", k.console_stdin.len());

    let before_pipes = k.pm.pipes.len();
    let before_fds = k.pm.processes[&1].fd_table.len();
    let bad_pipe = sys(&mut k, SYS_PIPE, 0xa000, 8, 0);
    println!("pipe_failure_rollback: result={bad_pipe:?}, pipes={before_pipes}->{}, fds={before_fds}->{} (expected unchanged)", k.pm.pipes.len(), k.pm.processes[&1].fd_table.len());
}
