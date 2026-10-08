use mini_os_kernel::{Kernel, arch::{ProgramBuilder, Register}, fs::{O_CREAT, O_RDWR}, mm::PAGE_SIZE, syscall::*};

#[test]
fn cow_munmap_must_not_recycle_the_parent_page() {
    let mut k = Kernel::new(16, 64, 8);
    let va = k.mmap(1, 0x30000000, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    k.mm.write_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, va, b"OLD!").unwrap();
    let child = k.fork_process_cow(1).unwrap();
    k.munmap(child, va, PAGE_SIZE).unwrap();
    let new_va = k.mmap(child, 0x40000000, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    k.mm.write_virtual(&mut k.pm.processes.get_mut(&child).unwrap().address_space, new_va, b"NEW!").unwrap();
    let mut value = [0; 4];
    k.mm.read_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, va, &mut value).unwrap();
    assert_eq!(&value, b"OLD!");
}

#[test]
fn cow_exec_must_not_clear_parent_memory() {
    let mut k = Kernel::new(16, 64, 8);
    k.mm.write_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, b"SECRET").unwrap();
    let child = k.fork_process_cow(1).unwrap();
    let mut b = ProgramBuilder::new(); b.halt();
    k.load_program_into_process(child, &b.finish()).unwrap();
    let mut value = [0; 6];
    k.mm.read_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, &mut value).unwrap();
    assert_eq!(&value, b"SECRET");
}

#[test]
fn exec_out_of_memory_must_preserve_original_space() {
    let mut k = Kernel::new(4, 64, 8);
    k.mm.write_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, b"SECRET").unwrap();
    let free = k.mm.buddy.stats.free_pages;
    assert!(k.load_program_into_process(1, &vec![0xab; 5 * PAGE_SIZE]).is_err());
    assert_eq!(k.mm.buddy.stats.free_pages, free, "failed exec consumes new pages and destroys old pages");
    let mut value = [0; 6];
    k.mm.read_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, &mut value).unwrap();
    assert_eq!(&value, b"SECRET");
}

#[test]
fn executable_eighth_page_must_not_be_replaced_by_data() {
    let mut k = Kernel::new(16, 64, 8);
    let code = vec![0xab; 8 * PAGE_SIZE];
    k.load_program_into_process(1, &code).unwrap();
    let mut value = [0; 4];
    k.mm.read_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, &mut value).unwrap();
    assert_eq!(value, [0xab; 4]);
}

#[test]
fn yield_return_must_not_clobber_selected_process_register() {
    let mut k = Kernel::new(16, 64, 8);
    let mut b = ProgramBuilder::new(); b.mov_imm(Register::Rax, SYS_YIELD as u32).syscall().halt();
    k.load_program_into_process(1, &b.finish()).unwrap();
    let child = k.fork_process(1).unwrap();
    let proc = k.pm.processes.get_mut(&child).unwrap();
    proc.context.rip = 0x9000;
    proc.context.rax = 123;
    k.step(1);
    assert_eq!(k.pm.current_pid, Some(child));
    assert_eq!(k.cpu.context.rax, 123, "SYS_YIELD return overwrites the new task's RAX");
}

#[test]
fn final_burst_tick_halt_must_not_double_terminate_init() {
    let mut k = Kernel::new(8, 64, 8);
    let mut b = ProgramBuilder::new(); b.halt();
    k.load_program_into_process(1, &b.finish()).unwrap();
    k.pm.processes.get_mut(&1).unwrap().cpu_burst_remaining = 1;
    k.step(1);
}

#[test]
fn readonly_file_mmap_must_contain_file_data_and_keep_offset() {
    let mut k = Kernel::new(8, 64, 8);
    let fd = k.vfs.open("/tmp/ro-map", O_CREAT | O_RDWR).unwrap();
    k.vfs.write(fd, b"SOURCE").unwrap();
    let old_offset = k.vfs.open_files[&fd].offset;
    let va = k.mmap(1, 0, PAGE_SIZE, PROT_READ, MAP_PRIVATE, Some(fd), 0).unwrap();
    let mut value = [0; 6];
    k.mm.read_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, va, &mut value).unwrap();
    assert_eq!(&value, b"SOURCE");
    assert_eq!(k.vfs.open_files[&fd].offset, old_offset);
}

#[test]
fn deep_fork_must_preserve_untouched_demand_mapping() {
    let mut k = Kernel::new(8, 64, 8);
    let va = k.mmap(1, 0, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    let child = k.fork_process(1).unwrap();
    k.mm.write_virtual(&mut k.pm.processes.get_mut(&child).unwrap().address_space, va, b"NEW!").unwrap();
}

#[test]
fn remapping_a_shared_page_must_preserve_the_other_owner() {
    let mut k = Kernel::new(16, 64, 8);
    k.mm.write_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, b"KEEP").unwrap();
    let child = k.fork_process_cow(1).unwrap();
    k.pm.processes.get_mut(&child).unwrap().address_space.allocate_and_map(&mut k.mm, 0x8000, 0x7).unwrap();
    let mut value = [0u8; 4];
    k.mm.read_virtual(&mut k.pm.processes.get_mut(&1).unwrap().address_space, 0x8000, &mut value).unwrap();
    assert_eq!(&value, b"KEEP");
    k.terminate_process(child, 0).unwrap();
    k.terminate_process(1, 0).unwrap();
    assert_eq!(k.mm.buddy.stats.free_pages, 16);
}

#[test]
fn partial_munmap_must_preserve_both_remaining_vma_fragments() {
    let mut k = Kernel::new(16, 64, 8);
    let va = k.mmap(1, 0, 3 * PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    k.munmap(1, va + PAGE_SIZE, PAGE_SIZE).unwrap();
    let proc = k.pm.processes.get_mut(&1).unwrap();
    assert!(proc.find_vma(va).is_some());
    assert!(proc.find_vma(va + PAGE_SIZE).is_none());
    assert!(proc.find_vma(va + 2 * PAGE_SIZE).is_some());
    assert!(k.mm.write_virtual(&mut proc.address_space, va + PAGE_SIZE, b"BAD").is_err());
    k.mm.write_virtual(&mut proc.address_space, va, b"LEFT").unwrap();
    k.mm.write_virtual(&mut proc.address_space, va + 2 * PAGE_SIZE, b"RIGHT").unwrap();
    assert!(k.mmap(1, va, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).is_err());
    k.mmap(1, va + PAGE_SIZE, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
}

#[test]
fn file_mmap_oom_must_preserve_cursor_vmas_and_free_pages() {
    let mut k = Kernel::new(4, 128, 8);
    let fd = k.vfs.open("/tmp/map", O_CREAT | O_RDWR).unwrap();
    k.vfs.write(fd, &vec![0x42; 3 * PAGE_SIZE]).unwrap();
    let offset = k.vfs.open_files[&fd].offset;
    let before = k.mm.buddy.stats.free_pages;
    let vmas = k.pm.processes[&1].vma_list.len();
    assert!(k.mmap(1, 0, 3 * PAGE_SIZE, PROT_READ, MAP_PRIVATE, Some(fd), 0).is_err());
    assert_eq!(k.mm.buddy.stats.free_pages, before);
    assert_eq!(k.pm.processes[&1].vma_list.len(), vmas);
    assert_eq!(k.vfs.open_files[&fd].offset, offset);
    assert!(k.pm.processes[&1].address_space.page_directory.walk(0x2000_0000).is_none());
}

#[test]
fn final_tick_sys_exit_must_preserve_its_exit_code_and_reclaim_once() {
    let mut k = Kernel::new(16, 64, 8);
    let child = k.fork_process(1).unwrap();
    let mut builder = ProgramBuilder::new();
    builder.mov_imm(Register::Rax, SYS_EXIT as u32).mov_imm(Register::Rdi, 7).syscall();
    k.load_program_into_process(child, &builder.finish()).unwrap();
    k.pm.processes.get_mut(&child).unwrap().cpu_burst_remaining = 1;
    k.yield_current().unwrap();
    assert_eq!(k.pm.current_pid, Some(child));
    let result = k.step_once();
    assert_eq!(result.schedule, mini_os_kernel::sched::ScheduleEvent::Terminated { pid: child });
    assert_eq!(k.pm.processes[&child].exit_code, 7);
    assert_eq!(k.mm.buddy.stats.allocated_pages, 2);
    k.terminate_process(child, 0).unwrap();
    assert_eq!(k.pm.processes[&child].exit_code, 7);
}

#[test]
fn overflowing_mapping_requests_must_not_change_existing_state() {
    let mut k = Kernel::new(8, 64, 8);
    let before = k.pm.processes[&1].vma_list.len();
    assert!(k.mmap(1, 0, usize::MAX, PROT_WRITE, MAP_ANONYMOUS, None, 0).is_err());
    assert!(k.munmap(1, 0x8000, usize::MAX).is_err());
    assert_eq!(k.pm.processes[&1].vma_list.len(), before);
    assert_eq!(k.mm.buddy.stats.allocated_pages, 2);
}

#[test]
fn streaming_steps_preserve_scheduler_results_without_collecting_logs() {
    let mut recorded = Kernel::new(8, 64, 8);
    let mut streamed = Kernel::new(8, 64, 8);
    let expected = recorded.step(1000).len();
    let mut observed = 0;
    streamed.step_stream(1000, |_, _| observed += 1);
    assert_eq!(observed, expected);
    assert_eq!(streamed.cpu.cycles, recorded.cpu.cycles);
    assert_eq!(streamed.timer.current_tick, recorded.timer.current_tick);
    assert_eq!(streamed.mm.buddy.stats.free_pages, recorded.mm.buddy.stats.free_pages);
}

#[test]
fn staged_cow_write_must_release_the_last_reference_of_aliased_pages() {
    use mini_os_kernel::mm::{AddressSpace, MemoryManager, FLAG_COW, FLAG_PRESENT, FLAG_USER};
    let mut mm = MemoryManager::new(3);
    let old = mm.allocate_pages_zeroed(0).unwrap();
    mm.inc_page_ref(old);
    let mut space = AddressSpace::new(8);
    space.page_directory.map_page(0x1000, old, FLAG_PRESENT | FLAG_USER | FLAG_COW);
    space.page_directory.map_page(0x2000, old, FLAG_PRESENT | FLAG_USER | FLAG_COW);
    mm.write_virtual_checked(&mut space, 0x1ffc, b"NEW!DATA", true).unwrap();
    assert_eq!(mm.get_page_ref(old), 0);
    assert_eq!(mm.buddy.stats.free_pages, 1);
    space.destroy(&mut mm);
    assert_eq!(mm.buddy.stats.free_pages, 3);
}

#[test]
fn loaded_nop_must_execute_and_reach_the_following_exit() {
    let mut k = Kernel::new(16, 64, 8);
    let mut builder = ProgramBuilder::new();
    builder.emit(mini_os_kernel::arch::Instruction::Nop)
        .mov_imm(Register::Rax, SYS_EXIT as u32).syscall();
    k.load_program_into_process(1, &builder.finish()).unwrap();
    k.step_once();
    assert!(k.pm.processes.is_empty());
    assert_eq!(k.mm.buddy.stats.free_pages, 16);
}

#[test]
fn zombie_must_reject_new_programs_mappings_and_forks_without_allocating() {
    let mut k = Kernel::new(16, 64, 8);
    let child = k.fork_process_cow(1).unwrap();
    k.terminate_process(child, 7).unwrap();
    let mut program = ProgramBuilder::new();
    program.halt();
    let pages = k.mm.buddy.stats.allocated_pages;
    assert!(k.load_program_into_process(child, &program.finish()).is_err());
    assert!(k.mmap(child, 0, PAGE_SIZE, PROT_WRITE, MAP_ANONYMOUS, None, 0).is_err());
    assert!(k.fork_process(child).is_err());
    assert!(k.fork_process_cow(child).is_err());
    k.terminate_process(child, 0).unwrap();
    assert_eq!(k.pm.waitpid(1, child).unwrap(), Some(7));
    assert_eq!(k.mm.buddy.stats.allocated_pages, pages);
}

#[test]
fn cross_page_write_failure_preserves_first_page_content() {
    let mut k = Kernel::new(16, 64, 8);
    let pid = 1;
    let proc = k.pm.processes.get_mut(&pid).unwrap();
    proc.address_space.allocate_and_map(&mut k.mm, 0x1000, 0x7).unwrap();
    proc.address_space.allocate_and_map(&mut k.mm, 0x2000, 0x5).unwrap();

    k.mm.write_virtual(&mut proc.address_space, 0x1000, &[0xAA; PAGE_SIZE]).unwrap();

    let payload = [0xBB; 6];
    let res = k.mm.write_virtual_checked(&mut proc.address_space, 0x1FFE, &payload, true);
    assert!(res.is_err(), "writing to read-only second page must fail");

    let mut tail = [0u8; 2];
    k.mm.read_virtual(&mut proc.address_space, 0x1FFE, &mut tail).unwrap();
    assert_eq!(tail, [0xAA, 0xAA], "atomic rollback: first page must remain uncorrupted");
}

#[test]
fn cross_page_write_unmapped_second_page_preserves_first_page_content() {
    let mut k = Kernel::new(16, 64, 8);
    let pid = 1;
    let proc = k.pm.processes.get_mut(&pid).unwrap();
    proc.address_space.allocate_and_map(&mut k.mm, 0x1000, 0x7).unwrap();

    k.mm.write_virtual(&mut proc.address_space, 0x1000, &[0xAA; PAGE_SIZE]).unwrap();

    let res = k.mm.write_virtual_checked(&mut proc.address_space, 0x1FFE, &[0xCC; 6], true);
    assert!(res.is_err(), "writing to unmapped second page must fail");

    let mut tail = [0u8; 2];
    k.mm.read_virtual(&mut proc.address_space, 0x1FFE, &mut tail).unwrap();
    assert_eq!(tail, [0xAA, 0xAA], "first page must remain uncorrupted when second page is unmapped");
}

#[test]
fn cross_page_write_oom_rolls_back_both_demand_pages() {
    let mut k = Kernel::new(3, 64, 8);
    assert_eq!(k.mm.buddy.stats.free_pages, 1);

    let addr = k.mmap(1, 0x2000_0000, 2 * PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    assert_eq!(addr, 0x2000_0000);
    assert_eq!(k.mm.buddy.stats.free_pages, 1);

    let proc = k.pm.processes.get_mut(&1).unwrap();
    let res = k.mm.write_virtual_checked(&mut proc.address_space, 0x2000_0FFE, &[0xDD; 8], true);
    assert!(res.is_err(), "must fail with out of physical memory");

    assert_eq!(k.mm.buddy.stats.free_pages, 1);
    assert!(proc.address_space.page_directory.walk(0x2000_0000).is_none());
    assert!(proc.address_space.page_directory.walk(0x2000_1000).is_none());
}

#[test]
fn cross_page_micro_vm_store_and_load() {
    let mut k = Kernel::new(32, 64, 8);
    let child = k.fork_process(1).unwrap();

    let target_va = 0x3000_0000 + PAGE_SIZE - 4;
    let mut builder = ProgramBuilder::new();
    builder.mov_imm(Register::Rax, target_va as u32);
    builder.mov_imm(Register::Rbx, 0x1234_5678);
    builder.store_mem(Register::Rbx, Register::Rax, 0);
    builder.load_mem(Register::Rcx, Register::Rax, 0);

    k.load_program_into_process(child, &builder.finish()).unwrap();
    let mmap_addr = k.mmap(child, 0x3000_0000, 2 * PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    assert_eq!(mmap_addr, 0x3000_0000);

    k.pm.processes.get_mut(&child).unwrap().cpu_burst_remaining = 10;
    k.yield_current().unwrap();
    assert_eq!(k.pm.current_pid, Some(child));
    k.step_once();

    let proc = k.pm.processes.get(&child).unwrap();
    assert_eq!(proc.context.get_reg(Register::Rcx), 0x1234_5678);

    let mut tail = [0u8; 4];
    let mut head = [0u8; 4];
    let proc = k.pm.processes.get_mut(&child).unwrap();
    k.mm.read_virtual(&mut proc.address_space, target_va, &mut tail).unwrap();
    k.mm.read_virtual(&mut proc.address_space, target_va + 4, &mut head).unwrap();
    assert_eq!(tail, (0x1234_5678u64.to_le_bytes())[..4]);
    assert_eq!(head, (0x1234_5678u64.to_le_bytes())[4..]);
}

#[test]
fn cross_page_micro_vm_store_unmapped_triggers_sigsegv_and_preserves_page() {
    let mut k = Kernel::new(32, 64, 8);
    let child = k.fork_process(1).unwrap();

    let cross_va = 0x3000_0000 + PAGE_SIZE - 4;
    let mut builder = ProgramBuilder::new();
    builder.mov_imm(Register::Rax, cross_va as u32);
    builder.mov_imm(Register::Rbx, 0xDEAD_BEEF);
    builder.store_mem(Register::Rbx, Register::Rax, 0);
    builder.halt();

    k.load_program_into_process(child, &builder.finish()).unwrap();
    let mmap_addr = k.mmap(child, 0x3000_0000, PAGE_SIZE, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, None, 0).unwrap();
    assert_eq!(mmap_addr, 0x3000_0000);

    let proc = k.pm.processes.get_mut(&child).unwrap();
    k.mm.write_virtual(&mut proc.address_space, mmap_addr + PAGE_SIZE - 4, &[0xAA; 4]).unwrap();

    k.pm.processes.get_mut(&child).unwrap().cpu_burst_remaining = 5;
    k.yield_current().unwrap();
    assert_eq!(k.pm.current_pid, Some(child));

    let res = k.step_once();
    assert_eq!(res.schedule, mini_os_kernel::sched::ScheduleEvent::Terminated { pid: child });
    assert_eq!(k.pm.processes[&child].exit_code, -14);
}

#[test]
fn final_tick_sleep_cancels_timer_and_terminates_cleanly() {
    let mut k = Kernel::new(16, 64, 8);
    let child = k.fork_process(1).unwrap();
    let mut builder = ProgramBuilder::new();
    builder.mov_imm(Register::Rax, SYS_SLEEP as u32)
        .mov_imm(Register::Rdi, 10)
        .syscall();
    k.load_program_into_process(child, &builder.finish()).unwrap();
    k.pm.processes.get_mut(&child).unwrap().cpu_burst_remaining = 1;
    k.yield_current().unwrap();
    assert_eq!(k.pm.current_pid, Some(child));

    let res = k.step_once();
    assert_eq!(res.schedule, mini_os_kernel::sched::ScheduleEvent::Terminated { pid: child });
    assert_eq!(k.pm.processes[&child].state, mini_os_kernel::sched::ProcessState::Zombie);

    for _ in 0..15 {
        let step_res = k.step_once();
        assert!(!step_res.awakened.contains(&child));
    }
}

#[test]
fn final_tick_fault_terminates_with_sigsegv_once() {
    let mut k = Kernel::new(16, 64, 8);
    let child = k.fork_process(1).unwrap();
    let mut builder = ProgramBuilder::new();
    builder.mov_imm(Register::Rax, 0xDEAD_0000)
        .load_mem(Register::Rbx, Register::Rax, 0);
    k.load_program_into_process(child, &builder.finish()).unwrap();
    k.pm.processes.get_mut(&child).unwrap().cpu_burst_remaining = 1;
    k.yield_current().unwrap();
    assert_eq!(k.pm.current_pid, Some(child));

    let res = k.step_once();
    assert_eq!(res.schedule, mini_os_kernel::sched::ScheduleEvent::Terminated { pid: child });
    assert_eq!(k.pm.processes[&child].exit_code, -14);
    assert_eq!(k.pm.waitpid(1, child).unwrap(), Some(-14));
}

