use mini_os_kernel::arch::{ProgramBuilder, Register};
use mini_os_kernel::kernel::Kernel;
use mini_os_kernel::syscall::{
    SyscallArgs, SYS_CLOSE, SYS_EXEC, SYS_EXIT, SYS_PIPE, SYS_READ, SYS_WAITPID, SYS_WRITE,
};

#[test]
fn test_micro_vm_arithmetic_and_loop() {
    let mut kernel = Kernel::new(16, 64, 8);

    // 编写一段纯用户态运算微指令：
    // Rax = 0;
    // loop:
    //   Rax += 1;
    //   Rbx = Rax;
    //   Rbx -= 10;
    //   Jnz (loop 0x1008);
    // Halt;
    let mut pb = ProgramBuilder::new();
    pb.mov_imm(Register::Rax, 0); // 0x1000
    pb.add_imm(Register::Rax, 1); // 0x1008 (loop label)
    pb.mov_reg(Register::Rbx, Register::Rax); // 0x1010
    pb.sub_imm(Register::Rbx, 10); // 0x1018
    pb.jnz(Register::Rbx, 0x1008); // 0x1020 -> jump to 0x1008 if Rbx != 0
    pb.halt(); // 0x1028

    let code = pb.finish();
    kernel.load_program_into_process(1, &code).unwrap();

    // 运行调度周期直到任务完成
    kernel.step(5);

    // 验证循环执行了 10 次，进程已正常退出
    assert!(!kernel.pm.processes.contains_key(&1));
}

#[test]
fn test_micro_vm_syscall_from_user_code() {
    let mut kernel = Kernel::new(16, 64, 8);

    let greeting = b"UserSyscallOK!";

    // 构建指令：
    // 1. SYS_WRITE(1, 0x8000, 14)
    // 2. SYS_EXIT(42)
    let mut pb = ProgramBuilder::new();
    pb.mov_imm(Register::Rax, SYS_WRITE as u32);
    pb.mov_imm(Register::Rdi, 1);
    pb.mov_imm(Register::Rsi, 0x8000);
    pb.mov_imm(Register::Rdx, greeting.len() as u32);
    pb.syscall();

    pb.mov_imm(Register::Rax, SYS_EXIT as u32);
    pb.mov_imm(Register::Rdi, 42);
    pb.syscall();

    let code = pb.finish();
    kernel.load_program_into_process(1, &code).unwrap();

    // 在数据段 0x8000 准备文本
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            greeting,
        )
        .unwrap();

    // 运行调度推进
    kernel.step(2);

    // 验证终端输出了用户态写入的内容
    assert_eq!(kernel.console_stdout, greeting);
}

#[test]
fn test_sys_exec_preinstalled_hello() {
    let mut kernel = Kernel::new(16, 64, 8);

    // 准备路径字符串在 0x8050
    let path = b"/bin/hello";
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8050,
            path,
        )
        .unwrap();

    // 调用 SYS_EXEC
    let res = kernel.syscall(SyscallArgs {
        num: SYS_EXEC,
        arg0: 0x8050,
        arg1: path.len(),
        arg2: 0,
        arg3: 0,
    });
    assert_eq!(res.unwrap(), 0);

    // 执行进程推进
    kernel.step(3);

    // 验证 /bin/hello 成功执行并输出了问候语
    assert_eq!(kernel.console_stdout, b"Hello from Mini-OS Exec!\n");
}

#[test]
fn test_autonomous_parent_child_pipeline_exec() {
    let mut kernel = Kernel::new(16, 64, 8);

    // 1. 父进程创建管道 (pipefd_read = 3, pipefd_write = 4)
    let pipe_res = kernel
        .syscall(SyscallArgs {
            num: SYS_PIPE,
            arg0: 0,
            arg1: 4096,
            arg2: 0,
            arg3: 0,
        })
        .unwrap();
    let read_fd = (pipe_res & 0xFFFF) as usize;
    let write_fd = ((pipe_res >> 16) & 0xFFFF) as usize;

    // 2. 将 Worker 可执行文件写入 VFS: /bin/consumer
    // Worker 逻辑: 从 read_fd(3) 读出 5 字节，然后写回 stdout(1)，最后 SYS_EXIT(0)
    let mut consumer_pb = ProgramBuilder::new();
    consumer_pb.mov_imm(Register::Rax, SYS_READ as u32);
    consumer_pb.mov_imm(Register::Rdi, read_fd as u32);
    consumer_pb.mov_imm(Register::Rsi, 0x8000);
    consumer_pb.mov_imm(Register::Rdx, 5);
    consumer_pb.syscall();

    consumer_pb.mov_imm(Register::Rax, SYS_WRITE as u32);
    consumer_pb.mov_imm(Register::Rdi, 1);
    consumer_pb.mov_imm(Register::Rsi, 0x8000);
    consumer_pb.mov_imm(Register::Rdx, 5);
    consumer_pb.syscall();

    consumer_pb.mov_imm(Register::Rax, SYS_EXIT as u32);
    consumer_pb.mov_imm(Register::Rdi, 0);
    consumer_pb.syscall();

    let consumer_code = consumer_pb.finish();
    let fd = kernel
        .vfs
        .open("/bin/consumer", mini_os_kernel::fs::O_CREAT | mini_os_kernel::fs::O_RDWR)
        .unwrap();
    kernel.vfs.write(fd, &consumer_code).unwrap();
    kernel.vfs.close(fd).unwrap();

    // 3. Fork 出子进程
    let child_pid = kernel.fork_process(1).unwrap();

    // 4. 让子进程 exec("/bin/consumer")
    kernel.exec_process(child_pid, "/bin/consumer").unwrap();

    // 5. 让父进程运行写入代码：向 write_fd(4) 写入 "WORK!"
    let mut producer_pb = ProgramBuilder::new();
    producer_pb.mov_imm(Register::Rax, SYS_WRITE as u32);
    producer_pb.mov_imm(Register::Rdi, write_fd as u32);
    producer_pb.mov_imm(Register::Rsi, 0x8000);
    producer_pb.mov_imm(Register::Rdx, 5);
    producer_pb.syscall();

    // 关闭写端，让子进程能感知 EOF
    producer_pb.mov_imm(Register::Rax, SYS_CLOSE as u32);
    producer_pb.mov_imm(Register::Rdi, write_fd as u32);
    producer_pb.syscall();

    // waitpid 等待子进程退出
    producer_pb.mov_imm(Register::Rax, SYS_WAITPID as u32);
    producer_pb.mov_imm(Register::Rdi, child_pid as u32);
    producer_pb.syscall();

    let producer_code = producer_pb.finish();
    kernel.load_program_into_process(1, &producer_code).unwrap();
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            0x8000,
            b"WORK!\0",
        )
        .unwrap();

    // 6. 运行调度循环推动多进程自主通信协同！
    kernel.step(10);

    // 验证子进程成功从管道消费并打印到了控制台！
    assert_eq!(kernel.console_stdout, b"WORK!");
}
