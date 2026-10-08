use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::sched::{BlockedReason, ProcessState};
use mini_os_kernel::syscall::{
    SyscallArgs, SYS_CLOSE, SYS_PIPE, SYS_READ, SYS_WRITE,
};

fn call_sys(kernel: &mut Kernel, num: usize, arg0: usize, arg1: usize, arg2: usize) -> Result<isize, &'static str> {
    kernel.syscall(SyscallArgs {
        num,
        arg0,
        arg1,
        arg2,
        arg3: 0,
    })
}

#[test]
fn test_pipe_creation_and_basic_rw() {
    let mut kernel = Kernel::new(16, 64, 8);
    // arg0: 0 (无需返回至内存指针), arg1: 4096 (容量)
    let pipe_res = call_sys(&mut kernel, SYS_PIPE, 0, 4096, 0).expect("create pipe");
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;
    assert_ne!(read_fd, write_fd);

    // 在 0x8000 (已映射且具备写权限的数据段) 写入 "Hello Pipe!"
    let msg = b"Hello Pipe!";
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            msg,
        )
        .unwrap();

    // SYS_WRITE 写入管道
    let written = call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8000, msg.len()).expect("write pipe");
    assert_eq!(written as usize, msg.len());

    // SYS_READ 从管道读出到 0x8100
    let read_n = call_sys(&mut kernel, SYS_READ, read_fd, 0x8100, 64).expect("read pipe");
    assert_eq!(read_n as usize, msg.len());

    let mut buf = vec![0u8; msg.len()];
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8100,
            &mut buf,
        )
        .unwrap();
    assert_eq!(&buf, msg);
}

#[test]
fn test_pipe_posix_pointer_return() {
    let mut kernel = Kernel::new(16, 64, 8);
    // 传入 0x8000 作为 pipefd 数组地址 (int pipefd[2])
    let res = call_sys(&mut kernel, SYS_PIPE, 0x8000, 4096, 0).expect("posix pipe");
    assert_eq!(res, 0);

    let mut fds = [0u8; 8];
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            &mut fds,
        )
        .unwrap();
    let read_fd = u32::from_ne_bytes(fds[0..4].try_into().unwrap()) as usize;
    let write_fd = u32::from_ne_bytes(fds[4..8].try_into().unwrap()) as usize;
    assert_ne!(read_fd, write_fd);

    let msg = b"PosixTest";
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8010,
            msg,
        )
        .unwrap();
    call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8010, msg.len()).unwrap();

    let mut read_buf = vec![0u8; msg.len()];
    call_sys(&mut kernel, SYS_READ, read_fd, 0x8020, msg.len()).unwrap();
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8020,
            &mut read_buf,
        )
        .unwrap();
    assert_eq!(&read_buf, msg);
}

#[test]
fn test_pipe_empty_blocks_reader_and_write_wakes() {
    let mut kernel = Kernel::new(16, 64, 8);
    let pipe_res = call_sys(&mut kernel, SYS_PIPE, 0, 4096, 0).unwrap();
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;

    // Fork 子进程作为 reader
    let reader_pid = kernel.fork_process(1).unwrap();
    kernel.pm.current_pid = Some(reader_pid);

    // 读空管道，读者转为阻塞
    let res = call_sys(&mut kernel, SYS_READ, read_fd, 0x8000, 10).unwrap();
    assert_eq!(res, -1);
    assert!(matches!(
        kernel.pm.processes[&reader_pid].state,
        ProcessState::Blocked(BlockedReason::WaitingPipeRead { .. })
    ));

    // 切回父进程写入数据
    kernel.pm.current_pid = Some(1);
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            b"DataReady!",
        )
        .unwrap();
    let written = call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8000, 10).unwrap();
    assert_eq!(written, 10);

    // 读者必须已被唤醒为 Ready
    assert_eq!(kernel.pm.processes[&reader_pid].state, ProcessState::Ready);

    // 读者再次读取即可读到数据
    kernel.pm.current_pid = Some(reader_pid);
    let n = call_sys(&mut kernel, SYS_READ, read_fd, 0x8200, 10).unwrap();
    assert_eq!(n, 10);

    let mut buf = vec![0u8; 10];
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&reader_pid).unwrap().address_space,
            0x8200,
            &mut buf,
        )
        .unwrap();
    assert_eq!(&buf, b"DataReady!");
}

#[test]
fn test_pipe_full_blocks_writer_and_read_wakes() {
    let mut kernel = Kernel::new(16, 64, 8);
    // 创建容量仅为 8 字节的管道
    let pipe_res = call_sys(&mut kernel, SYS_PIPE, 0, 8, 0).unwrap();
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;

    let payload = [0x55u8; 8];
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            &payload,
        )
        .unwrap();

    // 填满 8 字节
    let w1 = call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8000, 8).unwrap();
    assert_eq!(w1, 8);

    // Fork 子进程作为 writer 继续尝试写入
    let writer_pid = kernel.fork_process(1).unwrap();
    kernel.pm.current_pid = Some(writer_pid);
    let w2 = call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8000, 4).unwrap();
    assert_eq!(w2, -1);
    assert!(matches!(
        kernel.pm.processes[&writer_pid].state,
        ProcessState::Blocked(BlockedReason::WaitingPipeWrite { .. })
    ));

    // 父进程读取 4 字节腾出空间
    kernel.pm.current_pid = Some(1);
    let r1 = call_sys(&mut kernel, SYS_READ, read_fd, 0x8100, 4).unwrap();
    assert_eq!(r1, 4);

    // 阻塞的 writer 必须已被唤醒
    assert_eq!(kernel.pm.processes[&writer_pid].state, ProcessState::Ready);
}

#[test]
fn test_pipe_eof_when_all_writers_closed() {
    let mut kernel = Kernel::new(16, 64, 8);
    let pipe_res = call_sys(&mut kernel, SYS_PIPE, 0, 4096, 0).unwrap();
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;

    // 关闭写端
    call_sys(&mut kernel, SYS_CLOSE, write_fd, 0, 0).unwrap();

    // 读取空管道，因所有写端均已关闭，必须直接返回 0 (EOF)，绝不阻塞
    let n = call_sys(&mut kernel, SYS_READ, read_fd, 0x8000, 64).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn test_pipe_broken_when_all_readers_closed() {
    let mut kernel = Kernel::new(16, 64, 8);
    let pipe_res = call_sys(&mut kernel, SYS_PIPE, 0, 4096, 0).unwrap();
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;

    // 关闭读端
    call_sys(&mut kernel, SYS_CLOSE, read_fd, 0, 0).unwrap();

    // 尝试写入，必须返回 Broken pipe 错误
    let res = call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8000, 8);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "Broken pipe: reader closed");
}

#[test]
fn test_pipe_fork_inheritance_and_lifecycle() {
    let mut kernel = Kernel::new(16, 64, 8);
    let pipe_res = call_sys(&mut kernel, SYS_PIPE, 0, 4096, 0).unwrap();
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;

    let child_pid = kernel.fork_process(1).unwrap();

    // 父进程关闭读端
    call_sys(&mut kernel, SYS_CLOSE, read_fd, 0, 0).unwrap();

    // 子进程关闭写端
    kernel.pm.current_pid = Some(child_pid);
    call_sys(&mut kernel, SYS_CLOSE, write_fd, 0, 0).unwrap();

    // 父进程写入数据
    kernel.pm.current_pid = Some(1);
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            b"PipelineMsg",
        )
        .unwrap();
    let w = call_sys(&mut kernel, SYS_WRITE, write_fd, 0x8000, 11).unwrap();
    assert_eq!(w, 11);

    // 父进程写完关闭写端
    call_sys(&mut kernel, SYS_CLOSE, write_fd, 0, 0).unwrap();

    // 子进程读取数据
    kernel.pm.current_pid = Some(child_pid);
    let r = call_sys(&mut kernel, SYS_READ, read_fd, 0x8000, 64).unwrap();
    assert_eq!(r, 11);

    // 再次读取遇到 EOF
    let eof = call_sys(&mut kernel, SYS_READ, read_fd, 0x8000, 64).unwrap();
    assert_eq!(eof, 0);

    // 子进程退出并关闭读端
    kernel.terminate_process(child_pid, 0).unwrap();

    // 此时所有端已关闭，管道应被彻底回收
    assert!(kernel.pm.pipes.is_empty());
}

#[test]
fn test_console_stdin_stdout_fd() {
    let mut kernel = Kernel::new(16, 64, 8);

    // 测试标准输出 (FD 1)
    let hello = b"Kernel Console Output\n";
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            hello,
        )
        .unwrap();
    let w = call_sys(&mut kernel, SYS_WRITE, 1, 0x8000, hello.len()).unwrap();
    assert_eq!(w as usize, hello.len());
    assert_eq!(kernel.console_stdout, hello);

    // 测试标准输入 (FD 0)
    let input_data = b"User Command\n";
    kernel.console_stdin.extend(input_data.iter().copied());
    let r = call_sys(&mut kernel, SYS_READ, 0, 0x8100, 32).unwrap();
    assert_eq!(r as usize, input_data.len());

    let mut buf = vec![0u8; input_data.len()];
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8100,
            &mut buf,
        )
        .unwrap();
    assert_eq!(&buf, input_data);
}
