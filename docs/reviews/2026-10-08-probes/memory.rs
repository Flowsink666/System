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
