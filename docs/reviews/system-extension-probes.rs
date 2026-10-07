use mini_os_kernel::{
    Kernel,
    sched::{BlockedReason, ProcessState},
    syscall::*,
};

fn call(kernel: &mut Kernel, num: usize, arg0: usize) {
    kernel
        .syscall(SyscallArgs {
            num,
            arg0,
            arg1: 0,
            arg2: 0,
            arg3: 0,
        })
        .unwrap();
}

#[test]
fn child_exit_must_not_interrupt_parent_sleep() {
    let mut kernel = Kernel::new(8, 64, 8);
    let child = kernel.fork_process(1).unwrap();
    call(&mut kernel, SYS_SLEEP, 100);
    kernel.step(1);
    assert_eq!(kernel.pm.current_pid, Some(child));
    call(&mut kernel, SYS_EXIT, 0);
    assert!(matches!(
        kernel.pm.processes[&1].state,
        ProcessState::Blocked(BlockedReason::Sleeping { .. })
    ));
}

#[test]
fn yield_must_not_execute_or_prematurely_finish_process() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.pm.processes.get_mut(&1).unwrap().cpu_burst_remaining = 1;
    call(&mut kernel, SYS_YIELD, 0);
    assert_eq!(kernel.pm.processes[&1].cpu_burst_remaining, 1);
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 2);
    assert_eq!(kernel.timer.current_tick, 0);
    // yield 不再隐藏执行一个 tick；实际最后一个 tick 仍须走统一退出清理。
    kernel.step(1);
    assert_eq!(kernel.mm.buddy.stats.allocated_pages, 0);
}

#[test]
fn yield_switch_must_load_selected_cpu_context() {
    let mut kernel = Kernel::new(8, 64, 8);
    kernel.step(1);
    let child = kernel.fork_process(1).unwrap();
    kernel.pm.processes.get_mut(&child).unwrap().context.rip = 0x9000;
    for _ in 0..100 {
        call(&mut kernel, SYS_YIELD, 0);
        if kernel.pm.current_pid == Some(child) {
            assert_eq!(
                kernel.cpu.context.rip,
                kernel.pm.processes[&child].context.rip
            );
            return;
        }
    }
    panic!("yield did not select child");
}
