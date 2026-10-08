use mini_os_kernel::{
    Kernel,
    fs::{O_CREAT, O_RDWR},
    mm::{FLAG_USER, PAGE_SIZE},
    sched::{BlockedReason, FileDescriptorType, ProcessState, pcb::FD_FLAG_PIPE_READ},
    syscall::{MAP_ANONYMOUS, PROT_READ, PROT_WRITE, SYS_OPEN, SYS_PIPE, SYS_READ, SyscallArgs},
};

fn call(
    kernel: &mut Kernel,
    num: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
) -> Result<isize, &'static str> {
    kernel.syscall(SyscallArgs {
        num,
        arg0,
        arg1,
        arg2,
        arg3: 0,
    })
}

fn create_pipe(kernel: &mut Kernel, capacity: usize) -> (usize, usize) {
    let result = call(kernel, SYS_PIPE, 0, capacity, 0).unwrap();
    let read_fd = (result & 0xffff) as usize;
    let FileDescriptorType::PipeRead(pipe_id) =
        kernel.pm.processes[&1].fd_table[&read_fd].descriptor_type()
    else {
        panic!("Expected pipe read descriptor");
    };
    (read_fd, pipe_id)
}

fn read_memory(kernel: &mut Kernel, pid: usize, addr: usize, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    kernel
        .mm
        .read_virtual(
            &mut kernel.pm.processes.get_mut(&pid).unwrap().address_space,
            addr,
            &mut bytes,
        )
        .unwrap();
    bytes
}

fn write_memory(kernel: &mut Kernel, addr: usize, bytes: &[u8]) {
    kernel
        .mm
        .write_virtual(
            &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
            addr,
            bytes,
        )
        .unwrap();
}

#[test]
fn failed_pipe_read_preserves_data_destination_and_blocked_writer() {
    for readonly_second_page in [false, true] {
        let mut kernel = Kernel::new(8, 64, 8);
        let (read_fd, pipe_id) = create_pipe(&mut kernel, 8);
        kernel
            .pm
            .pipes
            .get_mut(&pipe_id)
            .unwrap()
            .write(b"PIPEdata")
            .unwrap();
        write_memory(&mut kernel, 0x8ffc, b"KEEP");
        if readonly_second_page {
            kernel
                .pm
                .processes
                .get_mut(&1)
                .unwrap()
                .address_space
                .allocate_and_map(&mut kernel.mm, 0x9000, FLAG_USER)
                .unwrap();
        }
        let writer = kernel.pm.spawn("waiting_writer", 0, 20);
        let reason = BlockedReason::WaitingPipeWrite { pipe_id };
        kernel.pm.block(writer, reason);
        let free_pages = kernel.mm.buddy.stats.free_pages;

        for _ in 0..3 {
            assert!(call(&mut kernel, SYS_READ, read_fd, 0x8ffc, 8).is_err());
            assert_eq!(
                kernel.pm.pipes[&pipe_id]
                    .buffer
                    .iter()
                    .copied()
                    .collect::<Vec<_>>(),
                b"PIPEdata"
            );
            assert_eq!(
                kernel.pm.processes[&writer].state,
                ProcessState::Blocked(reason)
            );
            assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages);
            assert_eq!(read_memory(&mut kernel, 1, 0x8ffc, 4), b"KEEP");
        }

        assert_eq!(call(&mut kernel, SYS_READ, read_fd, 0x8100, 8).unwrap(), 8);
        assert!(kernel.pm.pipes[&pipe_id].is_empty());
        assert_eq!(kernel.pm.processes[&writer].state, ProcessState::Ready);
        assert_eq!(read_memory(&mut kernel, 1, 0x8100, 8), b"PIPEdata");
    }
}

#[test]
fn failed_stdin_read_preserves_input_and_destination() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.console_stdin.extend(b"CONSOLE!".iter().copied());
    write_memory(&mut kernel, 0x8ffc, b"KEEP");
    for _ in 0..3 {
        assert!(call(&mut kernel, SYS_READ, 0, 0x8ffc, 8).is_err());
        assert_eq!(
            kernel.console_stdin.iter().copied().collect::<Vec<_>>(),
            b"CONSOLE!"
        );
        assert_eq!(read_memory(&mut kernel, 1, 0x8ffc, 4), b"KEEP");
    }
    assert_eq!(call(&mut kernel, SYS_READ, 0, 0x8100, 8).unwrap(), 8);
    assert!(kernel.console_stdin.is_empty());
    assert_eq!(read_memory(&mut kernel, 1, 0x8100, 8), b"CONSOLE!");
}

#[test]
fn failed_pipe_creation_restores_descriptors_and_next_fd() {
    let mut kernel = Kernel::new(8, 64, 8);
    write_memory(&mut kernel, 0x8ffc, b"KEEP");
    let next_fd = kernel.pm.processes[&1].next_fd;
    let fd_count = kernel.pm.processes[&1].fd_table.len();
    let free_pages = kernel.mm.buddy.stats.free_pages;
    for _ in 0..5 {
        assert!(call(&mut kernel, SYS_PIPE, 0x8ffc, 8, 0).is_err());
        assert!(kernel.pm.pipes.is_empty());
        assert_eq!(kernel.pm.processes[&1].fd_table.len(), fd_count);
        assert_eq!(kernel.pm.processes[&1].next_fd, next_fd);
        assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages);
        assert_eq!(read_memory(&mut kernel, 1, 0x8ffc, 4), b"KEEP");
    }
    assert_eq!(call(&mut kernel, SYS_PIPE, 0x8100, 8, 0).unwrap(), 0);
    let descriptors = read_memory(&mut kernel, 1, 0x8100, 8);
    assert_eq!(
        u32::from_ne_bytes(descriptors[..4].try_into().unwrap()) as usize,
        next_fd
    );
    assert_eq!(
        u32::from_ne_bytes(descriptors[4..].try_into().unwrap()) as usize,
        next_fd + 1
    );
    assert_eq!(kernel.pm.pipes.len(), 1);
}

#[test]
fn pipe_read_into_cross_page_demand_buffer_commits_and_wakes_writer() {
    let mut kernel = Kernel::new(8, 64, 8);
    let (read_fd, pipe_id) = create_pipe(&mut kernel, 8);
    kernel
        .pm
        .pipes
        .get_mut(&pipe_id)
        .unwrap()
        .write(b"PIPEdata")
        .unwrap();
    let writer = kernel.pm.spawn("waiting_writer", 0, 20);
    kernel
        .pm
        .block(writer, BlockedReason::WaitingPipeWrite { pipe_id });
    let base = kernel
        .mmap(
            1,
            0,
            2 * PAGE_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_ANONYMOUS,
            None,
            0,
        )
        .unwrap();
    let free_pages = kernel.mm.buddy.stats.free_pages;
    let destination = base + PAGE_SIZE - 4;
    assert_eq!(
        call(&mut kernel, SYS_READ, read_fd, destination, 8).unwrap(),
        8
    );
    assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages - 2);
    assert!(kernel.pm.pipes[&pipe_id].is_empty());
    assert_eq!(kernel.pm.processes[&writer].state, ProcessState::Ready);
    assert_eq!(read_memory(&mut kernel, 1, destination, 8), b"PIPEdata");
}

#[test]
fn pipe_and_stdin_reads_resolve_cow_without_modifying_parent() {
    for stdin in [false, true] {
        let mut kernel = Kernel::new(8, 64, 8);
        let (read_fd, pipe_id) = create_pipe(&mut kernel, 8);
        write_memory(&mut kernel, 0x8100, b"PARENT!!");
        if stdin {
            kernel.console_stdin.extend(b"CHILD!!!".iter().copied());
        } else {
            kernel
                .pm
                .pipes
                .get_mut(&pipe_id)
                .unwrap()
                .write(b"CHILD!!!")
                .unwrap();
        }
        let child = kernel.fork_process_cow(1).unwrap();
        kernel.pm.current_pid = Some(child);
        let free_pages = kernel.mm.buddy.stats.free_pages;
        assert_eq!(
            call(
                &mut kernel,
                SYS_READ,
                if stdin { 0 } else { read_fd },
                0x8100,
                8
            )
            .unwrap(),
            8
        );
        assert_eq!(kernel.mm.buddy.stats.free_pages, free_pages - 1);
        assert_eq!(read_memory(&mut kernel, child, 0x8100, 8), b"CHILD!!!");
        assert_eq!(read_memory(&mut kernel, 1, 0x8100, 8), b"PARENT!!");
        assert!(if stdin {
            kernel.console_stdin.is_empty()
        } else {
            kernel.pm.pipes[&pipe_id].is_empty()
        });
    }
}

#[test]
fn cow_out_of_memory_does_not_consume_pipe_or_stdin_input() {
    for stdin in [false, true] {
        let mut kernel = Kernel::new(2, 64, 8);
        let (read_fd, pipe_id) = create_pipe(&mut kernel, 8);
        write_memory(&mut kernel, 0x8100, b"PARENT!!");
        if stdin {
            kernel.console_stdin.extend(b"CHILD!!!".iter().copied());
        } else {
            kernel
                .pm
                .pipes
                .get_mut(&pipe_id)
                .unwrap()
                .write(b"CHILD!!!")
                .unwrap();
        }
        let child = kernel.fork_process_cow(1).unwrap();
        kernel.pm.current_pid = Some(child);
        for _ in 0..3 {
            assert!(
                call(
                    &mut kernel,
                    SYS_READ,
                    if stdin { 0 } else { read_fd },
                    0x8100,
                    8
                )
                .is_err()
            );
            let input: Vec<_> = if stdin {
                kernel.console_stdin.iter().copied().collect()
            } else {
                kernel.pm.pipes[&pipe_id].buffer.iter().copied().collect()
            };
            assert_eq!(input, b"CHILD!!!");
            assert_eq!(kernel.mm.buddy.stats.free_pages, 0);
            assert_eq!(read_memory(&mut kernel, child, 0x8100, 8), b"PARENT!!");
            assert_eq!(read_memory(&mut kernel, 1, 0x8100, 8), b"PARENT!!");
        }
    }
}

#[test]
fn demand_out_of_memory_rolls_back_pages_and_preserves_pipe_input() {
    let mut kernel = Kernel::new(3, 64, 8);
    let (read_fd, pipe_id) = create_pipe(&mut kernel, 8);
    kernel
        .pm
        .pipes
        .get_mut(&pipe_id)
        .unwrap()
        .write(b"PIPEdata")
        .unwrap();
    let base = kernel
        .mmap(
            1,
            0,
            2 * PAGE_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_ANONYMOUS,
            None,
            0,
        )
        .unwrap();
    for _ in 0..3 {
        assert!(call(&mut kernel, SYS_READ, read_fd, base + PAGE_SIZE - 4, 8).is_err());
        assert_eq!(
            kernel.pm.pipes[&pipe_id]
                .buffer
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            b"PIPEdata"
        );
        assert_eq!(kernel.mm.buddy.stats.free_pages, 1);
        let space = &kernel.pm.processes[&1].address_space;
        assert!(space.page_directory.walk(base).is_none());
        assert!(space.page_directory.walk(base + PAGE_SIZE).is_none());
    }
}

#[test]
fn pipe_creation_cow_out_of_memory_rolls_back_resources() {
    let mut kernel = Kernel::new(2, 64, 8);
    write_memory(&mut kernel, 0x8100, b"PARENT!!");
    let child = kernel.fork_process_cow(1).unwrap();
    kernel.pm.current_pid = Some(child);
    let next_fd = kernel.pm.processes[&child].next_fd;
    for _ in 0..3 {
        assert!(call(&mut kernel, SYS_PIPE, 0x8100, 8, 0).is_err());
        assert!(kernel.pm.pipes.is_empty());
        assert_eq!(kernel.pm.processes[&child].fd_table.len(), 3);
        assert_eq!(kernel.pm.processes[&child].next_fd, next_fd);
        assert_eq!(kernel.mm.buddy.stats.free_pages, 0);
        assert_eq!(read_memory(&mut kernel, child, 0x8100, 8), b"PARENT!!");
    }
}

#[test]
fn pipe_capacity_is_bounded_and_zero_keeps_default() {
    let mut kernel = Kernel::new(8, 64, 8);
    let (_, default_pipe) = create_pipe(&mut kernel, 0);
    assert_eq!(kernel.pm.pipes[&default_pipe].capacity, 4096);
    let fd_count = kernel.pm.processes[&1].fd_table.len();
    let next_fd = kernel.pm.processes[&1].next_fd;
    for capacity in [64 * 1024 + 1, usize::MAX] {
        assert!(call(&mut kernel, SYS_PIPE, 0x8100, capacity, 0).is_err());
        assert_eq!(kernel.pm.pipes.len(), 1);
        assert_eq!(kernel.pm.processes[&1].fd_table.len(), fd_count);
        assert_eq!(kernel.pm.processes[&1].next_fd, next_fd);
    }
    let (_, maximum_pipe) = create_pipe(&mut kernel, 64 * 1024);
    assert_eq!(kernel.pm.pipes[&maximum_pipe].capacity, 64 * 1024);
}

#[test]
fn open_rejects_internal_descriptor_flags_before_creating_file() {
    let mut kernel = Kernel::new(8, 64, 8);
    let path = b"/tmp/forged-descriptor";
    write_memory(&mut kernel, 0x8100, path);
    let fd_count = kernel.pm.processes[&1].fd_table.len();
    let open_count = kernel.vfs.open_files.len();
    for extra_flag in [FD_FLAG_PIPE_READ as usize, 1usize << 40] {
        assert!(
            call(
                &mut kernel,
                SYS_OPEN,
                0x8100,
                path.len(),
                (O_CREAT | O_RDWR) as usize | extra_flag
            )
            .is_err()
        );
        assert_eq!(kernel.pm.processes[&1].fd_table.len(), fd_count);
        assert_eq!(kernel.vfs.open_files.len(), open_count);
        assert!(kernel.vfs.stat(std::str::from_utf8(path).unwrap()).is_err());
    }
}
