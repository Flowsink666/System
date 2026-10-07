use mini_os_kernel::{
    Kernel,
    fs::{self, VirtualFileSystem},
    mm::{self, AddressSpace, BuddyAllocator, MemoryManager},
    sched::{self, FileDescriptorEntry, ProcessState},
    syscall::*,
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

#[test]
fn slab_returned_page_must_not_be_reused_without_buddy_allocation() {
    let mut buddy = BuddyAllocator::new(2);
    let mut cache = mm::slab::SlabCache::new("test", 2048);
    let a = cache.allocate(&mut buddy).unwrap();
    let b = cache.allocate(&mut buddy).unwrap();
    let _c = cache.allocate(&mut buddy).unwrap();
    cache.free(&mut buddy, a.0, a.1).unwrap();
    cache.free(&mut buddy, b.0, b.1).unwrap();
    let exclusive_page = buddy.allocate_pages(0).unwrap();
    let reused = cache.allocate(&mut buddy).unwrap();
    assert_ne!(exclusive_page, reused.0, "Buddy and Slab own the same PFN");
}

#[test]
fn failed_fork_must_roll_back_child_and_pages() {
    let mut kernel = Kernel::new(3, 64, 8);
    let before = (kernel.pm.processes.len(), kernel.mm.buddy.stats.free_pages);
    assert!(kernel.fork_process(1).is_err());
    assert_eq!(
        (kernel.pm.processes.len(), kernel.mm.buddy.stats.free_pages),
        before
    );
}

#[test]
fn sleep_must_wake_after_requested_ticks() {
    let mut kernel = Kernel::new(8, 64, 8);
    call(&mut kernel, SYS_SLEEP, 2, 0, 0).unwrap();
    kernel.step(5);
    assert!(!matches!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(_)
    ));
}

#[test]
fn child_sys_exit_must_wake_waiting_parent() {
    let mut kernel = Kernel::new(8, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    assert_eq!(kernel.pm.waitpid(1, child).unwrap(), None);
    kernel.step(1);
    assert_eq!(kernel.pm.current_pid, Some(child));
    call(&mut kernel, SYS_EXIT, 7, 0, 0).unwrap();
    kernel.step(3);
    assert!(!matches!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(_)
    ));
}

#[test]
fn shell_kill_then_reap_must_release_child_memory() {
    let mut kernel = Kernel::new(8, 64, 8);
    let initial = kernel.mm.buddy.stats.allocated_pages;
    let child = kernel.fork_process(1).unwrap();
    kernel.terminate_process(child, 0).unwrap();
    kernel.pm.waitpid(1, child).unwrap();
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, initial);
}

#[test]
fn terminated_ready_child_must_leave_runqueue() {
    let mut kernel = Kernel::new(8, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    kernel.terminate_process(child, 0).unwrap();
    let first = kernel.pm.scheduler.pick_next_task();
    let second = kernel.pm.scheduler.pick_next_task();
    assert!(first != Some(child) && second != Some(child));
}

#[test]
fn burst_completion_must_release_address_space() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.pm.processes.get_mut(&1).unwrap().cpu_burst_remaining = 1;
    kernel.step(3);
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 0);
}

#[test]
fn initial_cpu_context_must_match_init_pcb() {
    let mut kernel = Kernel::new(8, 64, 8);
    let initial_rip = kernel.pm.processes[&1].context.rip;
    kernel.step(1);
    assert_eq!(kernel.pm.processes[&1].context.rip, initial_rip + 40);
}

#[test]
fn forked_fd_allocator_must_preserve_inherited_fd() {
    let mut kernel = Kernel::new(8, 64, 8);
    let old_vfs_fd = kernel.vfs.open("/tmp/a", fs::O_CREAT | fs::O_RDWR).unwrap();
    let old_user_fd = kernel
        .pm
        .processes
        .get_mut(&1)
        .unwrap()
        .alloc_fd(FileDescriptorEntry {
            vfs_fd: old_vfs_fd,
            flags: fs::O_RDWR,
        });
    let child = kernel.fork_process(1).unwrap();
    let new_vfs_fd = kernel.vfs.open("/tmp/b", fs::O_CREAT | fs::O_RDWR).unwrap();
    let new_user_fd = kernel
        .pm
        .processes
        .get_mut(&child)
        .unwrap()
        .alloc_fd(FileDescriptorEntry {
            vfs_fd: new_vfs_fd,
            flags: fs::O_RDWR,
        });
    assert_ne!(old_user_fd, new_user_fd);
}

#[test]
fn closing_child_inherited_fd_must_preserve_parent_fd() {
    let mut kernel = Kernel::new(8, 64, 8);
    let global = kernel.vfs.open("/tmp/a", fs::O_CREAT | fs::O_RDWR).unwrap();
    let fd = kernel
        .pm
        .processes
        .get_mut(&1)
        .unwrap()
        .alloc_fd(FileDescriptorEntry {
            vfs_fd: global,
            flags: fs::O_RDWR,
        });
    let child = kernel.fork_process(1).unwrap();
    kernel.pm.current_pid = Some(child);
    call(&mut kernel, SYS_CLOSE, fd, 0, 0).unwrap();
    assert!(kernel.vfs.write(global, b"parent").is_ok());
}

#[test]
fn invalid_read_buffer_must_not_consume_file_offset() {
    let mut kernel = Kernel::new(8, 64, 8);
    let global = kernel.vfs.open("/tmp/a", fs::O_CREAT | fs::O_RDWR).unwrap();
    kernel.vfs.write(global, b"abc").unwrap();
    kernel.vfs.seek(global, 0).unwrap();
    let fd = kernel
        .pm
        .processes
        .get_mut(&1)
        .unwrap()
        .alloc_fd(FileDescriptorEntry {
            vfs_fd: global,
            flags: fs::O_RDWR,
        });
    assert!(call(&mut kernel, SYS_READ, fd, 0xdead000, 2).is_err());
    assert_eq!(kernel.vfs.open_files[&global].offset, 0);
}

#[test]
fn zero_length_write_must_write_zero_bytes() {
    let mut kernel = Kernel::new(8, 64, 8);
    let fd = call(
        &mut kernel,
        SYS_OPEN,
        0,
        0,
        (fs::O_CREAT | fs::O_RDWR) as usize,
    )
    .unwrap() as usize;
    assert_eq!(call(&mut kernel, SYS_WRITE, fd, 0, 0).unwrap(), 0);
}

#[test]
fn page_reuse_must_not_expose_previous_process_data() {
    let mut mm = MemoryManager::new(1);
    let mut old = AddressSpace::new(8);
    old.allocate_and_map(&mut mm, 0x1000, 7).unwrap();
    mm.write_virtual(&mut old, 0x1000, b"SECRET").unwrap();
    old.destroy(&mut mm.buddy);
    let mut fresh = AddressSpace::new(8);
    fresh.allocate_and_map(&mut mm, 0x1000, 7).unwrap();
    let mut buf = [0; 6];
    mm.read_virtual(&mut fresh, 0x1000, &mut buf).unwrap();
    assert_eq!(buf, [0; 6]);
}

#[test]
fn addresses_above_32_bits_must_not_alias_low_addresses() {
    let mut mm = MemoryManager::new(1);
    let mut space = AddressSpace::new(8);
    space.allocate_and_map(&mut mm, 0x1000, 7).unwrap();
    assert!(space.translate(0x1_0000_1000).is_err());
}

#[test]
fn present_flag_must_be_consistent_between_tlb_and_page_table() {
    let mut mm = MemoryManager::new(1);
    let mut space = AddressSpace::new(8);
    space
        .allocate_and_map(&mut mm, 0x1000, mm::FLAG_WRITABLE | mm::FLAG_USER)
        .unwrap();
    let warm = space.translate(0x1000);
    space.tlb.flush();
    let cold = space.translate(0x1000);
    assert_eq!(warm, cold);
}

#[test]
fn unlink_dot_must_not_destroy_directory() {
    let mut vfs = VirtualFileSystem::new(32, 8);
    vfs.mkdir("/dir").unwrap();
    let _ = vfs.unlink("/dir/.");
    assert!(vfs.stat("/dir").is_ok());
}

#[test]
fn unlink_open_file_must_keep_handle_readable_until_close() {
    let mut vfs = VirtualFileSystem::new(32, 8);
    let fd = vfs.open("/file", fs::O_CREAT | fs::O_RDWR).unwrap();
    vfs.write(fd, b"abc").unwrap();
    vfs.seek(fd, 0).unwrap();
    vfs.unlink("/file").unwrap();
    let mut buf = [0; 3];
    assert_eq!(vfs.read(fd, &mut buf), Ok(3));
}

#[test]
fn directory_must_not_accept_regular_file_writes() {
    let mut vfs = VirtualFileSystem::new(32, 8);
    vfs.mkdir("/dir").unwrap();
    let fd = vfs.open("/dir", fs::O_RDWR).unwrap();
    assert!(vfs.write(fd, b"abc").is_err());
}

#[test]
fn running_task_must_advance_scheduler_min_vruntime() {
    let mut pm = sched::ProcessManager::new(1000);
    let solo = pm.spawn("solo", 0, 1000);
    for _ in 0..100 {
        pm.schedule_step(1);
    }
    assert!(pm.processes[&solo].vruntime > 0);
    assert!(pm.scheduler.min_vruntime > 0);
}
