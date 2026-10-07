use mini_os_kernel::{
    Kernel,
    arch::{PrivilegeLevel, VirtualTimer},
    fs::{O_CREAT, O_RDWR},
    mm::{AddressSpace, MemoryManager, PAGE_SIZE},
    sched::{BlockedReason, FileDescriptorEntry, ProcessState},
    syscall::*,
};

fn call(kernel: &mut Kernel, num: usize, arg0: usize) -> Result<isize, &'static str> {
    kernel.syscall(SyscallArgs {
        num,
        arg0,
        arg1: 0,
        arg2: 0,
        arg3: 0,
    })
}

#[test]
fn yield_switches_context_without_charging_either_process() {
    let mut kernel = Kernel::new(16, 64, 8);
    kernel.step(1);
    let child = kernel.fork_process(1).unwrap();
    kernel.pm.processes.get_mut(&child).unwrap().context.rip = 0x9000;
    kernel.pm.processes.get_mut(&child).unwrap().context.rax = 42;
    // 系统调用前的实时 CPU 寄存器也必须保存到让出进程的 PCB。
    kernel.cpu.context.rbx = 123;
    let before: Vec<_> = [1, child]
        .iter()
        .map(|pid| {
            let proc = &kernel.pm.processes[pid];
            (
                proc.cpu_burst_remaining,
                proc.exec_time,
                proc.vruntime,
                proc.context.rip,
            )
        })
        .collect();
    let tick = kernel.timer.current_tick;
    call(&mut kernel, SYS_YIELD, 0).unwrap();
    assert_eq!(kernel.pm.current_pid, Some(child));
    assert_eq!(kernel.cpu.context.rip, 0x9000);
    assert_eq!(kernel.cpu.context.rax, 42);
    assert_eq!(kernel.pm.processes[&1].context.rbx, 123);
    assert_eq!(kernel.cpu.privilege, PrivilegeLevel::Ring3User);
    assert_eq!(kernel.timer.current_tick, tick);
    for (pid, expected) in [1, child].iter().zip(before) {
        let proc = &kernel.pm.processes[pid];
        assert_eq!(
            (
                proc.cpu_burst_remaining,
                proc.exec_time,
                proc.vruntime,
                proc.context.rip
            ),
            expected
        );
    }
    kernel.step(1);
    assert_eq!(kernel.pm.processes[&child].context.rip, 0x9028);
    assert_ne!(kernel.pm.processes[&1].context.rip, 0x9028);
}

#[test]
fn yielding_exhausted_process_releases_pages_and_file_handles() {
    let mut kernel = Kernel::new(8, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    let fd = kernel.vfs.open("/tmp/exhausted", O_CREAT | O_RDWR).unwrap();
    kernel
        .pm
        .processes
        .get_mut(&1)
        .unwrap()
        .alloc_fd(FileDescriptorEntry {
            vfs_fd: fd,
            flags: O_RDWR,
        });
    kernel.pm.processes.get_mut(&1).unwrap().cpu_burst_remaining = 0;
    call(&mut kernel, SYS_YIELD, 0).unwrap();
    assert_eq!(kernel.pm.current_pid, Some(child));
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 2);
    assert!(!kernel.vfs.open_files.contains_key(&fd));
    assert!(!kernel.pm.processes.contains_key(&1));
    assert_eq!(
        kernel.cpu.context.rip,
        kernel.pm.processes[&child].context.rip
    );
}

#[test]
fn yield_without_peer_preserves_queue_weight_and_cpu_accounting() {
    let mut kernel = Kernel::new(8, 64, 8);
    let cycles = kernel.cpu.cycles;
    let burst = kernel.pm.processes[&1].cpu_burst_remaining;
    for _ in 0..5 {
        call(&mut kernel, SYS_YIELD, 0).unwrap();
    }
    assert_eq!(kernel.pm.current_pid, Some(1));
    assert_eq!(kernel.cpu.cycles, cycles);
    assert_eq!(kernel.pm.scheduler.total_weight, 0);
    assert_eq!(kernel.pm.processes[&1].cpu_burst_remaining, burst);
    kernel.step(1);
    assert_eq!(kernel.pm.processes[&1].cpu_burst_remaining, burst - 1);
}

#[test]
fn final_tick_runs_before_child_resources_are_released() {
    let mut kernel = Kernel::new(16, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    let fd = kernel.vfs.open("/tmp/child", O_CREAT | O_RDWR).unwrap();
    let proc = kernel.pm.processes.get_mut(&child).unwrap();
    proc.alloc_fd(FileDescriptorEntry {
        vfs_fd: fd,
        flags: O_RDWR,
    });
    proc.cpu_burst_remaining = 1;
    proc.context.rip = 0x9000;
    kernel.sleep_current(100).unwrap();
    kernel.step(1);
    let proc = &kernel.pm.processes[&child];
    assert_eq!(proc.state, ProcessState::Zombie);
    assert_eq!(proc.context.rip, 0x9028);
    assert!(proc.fd_table.is_empty());
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 2);
    assert!(!kernel.vfs.open_files.contains_key(&fd));
    assert_eq!(kernel.cpu.privilege, PrivilegeLevel::Ring0Kernel);
    assert!(matches!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(BlockedReason::Sleeping { .. })
    ));
}

#[test]
fn killing_child_does_not_wake_sleeping_parent() {
    let mut kernel = Kernel::new(16, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    kernel.sleep_current(5).unwrap();
    kernel.terminate_process(child, 7).unwrap();
    kernel.step(4);
    assert!(matches!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(BlockedReason::Sleeping { .. })
    ));
    kernel.step(1);
    assert_eq!(kernel.pm.current_pid, Some(1));
}

#[test]
fn unrelated_child_exit_does_not_release_waitpid() {
    let mut kernel = Kernel::new(16, 64, 8);
    let expected_child = kernel.fork_process(1).unwrap();
    let other_child = kernel.fork_process(1).unwrap();
    assert_eq!(kernel.pm.waitpid(1, expected_child).unwrap(), None);
    kernel.terminate_process(other_child, 7).unwrap();
    assert_eq!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(BlockedReason::WaitingChild {
            pid: expected_child
        })
    );
    kernel.terminate_process(expected_child, 9).unwrap();
    assert_eq!(kernel.pm.processes[&1].state, ProcessState::Ready);
    assert_eq!(kernel.pm.waitpid(1, expected_child).unwrap(), Some(9));
}

#[test]
fn stale_sleep_event_cannot_wake_a_later_sleep() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.sleep_current(1).unwrap();
    let stale = kernel.timer.tick().pop().unwrap();
    assert!(
        kernel
            .pm
            .wake(1, BlockedReason::Sleeping { token: stale.token })
    );
    kernel.step(1);
    kernel.sleep_current(3).unwrap();
    assert!(
        !kernel
            .pm
            .wake(1, BlockedReason::Sleeping { token: stale.token })
    );
    kernel.step(2);
    assert!(matches!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(BlockedReason::Sleeping { .. })
    ));
    kernel.step(1);
    assert_eq!(kernel.pm.current_pid, Some(1));
}

#[test]
fn sleep_zero_yields_without_registering_a_timer() {
    let mut kernel = Kernel::new(8, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    call(&mut kernel, SYS_SLEEP, 0).unwrap();
    assert_eq!(kernel.pm.current_pid, Some(child));
    assert_eq!(kernel.timer.current_tick, 0);
    assert_eq!(kernel.pm.processes[&1].state, ProcessState::Ready);
    assert!(kernel.timer.tick().is_empty());
}

#[test]
fn exiting_sleeper_cancels_timer() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.sleep_current(2).unwrap();
    kernel.terminate_process(1, 0).unwrap();
    assert!(kernel.timer.tick().is_empty());
    assert!(kernel.timer.tick().is_empty());
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 0);
}

#[test]
fn rescheduling_sleep_replaces_old_deadline() {
    let mut timer = VirtualTimer::new(1000);
    let first = timer.add_sleep(1, 1).unwrap();
    let second = timer.add_sleep(1, 2).unwrap();
    assert_ne!(first, second);
    assert!(timer.tick().is_empty());
    let events = timer.tick();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].token, second);
}

#[test]
fn overflowing_sleep_is_rejected_without_blocking_process() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.step(1);
    assert!(kernel.sleep_current(u64::MAX).is_err());
    assert_eq!(kernel.pm.current_pid, Some(1));
    assert_eq!(kernel.pm.processes[&1].state, ProcessState::Running);
}

#[test]
fn complete_page_block_is_zeroed_after_reuse_and_manager_move() {
    let mut mm = MemoryManager::new(4);
    let pfn = mm.allocate_pages_zeroed(2).unwrap();
    mm.write_physical(pfn * PAGE_SIZE, &vec![0xab; 4 * PAGE_SIZE])
        .unwrap();
    mm.buddy.free_pages(pfn).unwrap();
    let mut moved = Box::new(mm);
    let pfn = moved.allocate_pages_zeroed(2).unwrap();
    let mut bytes = vec![0xff; 4 * PAGE_SIZE];
    moved.read_physical(pfn * PAGE_SIZE, &mut bytes).unwrap();
    assert!(bytes.iter().all(|byte| *byte == 0));
}

#[test]
fn failed_fork_at_each_page_preserves_parent_and_free_pages() {
    for total_pages in 3..6 {
        let mut kernel = Kernel::new(total_pages, 64, 8);
        let proc = kernel.pm.processes.get_mut(&1).unwrap();
        proc.address_space
            .allocate_and_map(&mut kernel.mm, 0x2000, 7)
            .unwrap();
        kernel
            .mm
            .write_virtual(&mut proc.address_space, 0x8000, b"parent")
            .unwrap();
        let free = kernel.mm.buddy.stats.free_pages;
        assert!(call(&mut kernel, SYS_FORK, 0).is_err());
        assert_eq!(kernel.pm.processes.len(), 1);
        assert_eq!(kernel.mm.buddy.stats.free_pages, free);
        let mut bytes = [0; 6];
        kernel
            .mm
            .read_virtual(
                &mut kernel.pm.processes.get_mut(&1).unwrap().address_space,
                0x8000,
                &mut bytes,
            )
            .unwrap();
        assert_eq!(&bytes, b"parent");
    }
}

#[test]
fn invalid_mapping_does_not_allocate_or_clear_a_page() {
    let mut mm = MemoryManager::new(1);
    let mut space = AddressSpace::new(8);
    assert!(space.allocate_and_map(&mut mm, 0x1_0000_0000, 7).is_err());
    assert_eq!(mm.buddy.stats.free_pages, 1);
}
