//! CPU 虚拟执行架构与寄存器上下文管理

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeLevel {
    Ring0Kernel,
    Ring3User,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CpuContext {
    pub rip: u64,      // 指令指针 (Program Counter)
    pub rsp: u64,      // 栈指针 (Stack Pointer)
    pub rbp: u64,      // 栈基址指针
    pub rax: u64,      // 通用寄存器
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rflags: u64,   // 标志寄存器
}

use super::instruction::Register;

impl CpuContext {
    pub fn new(entry_point: u64, stack_top: u64) -> Self {
        Self {
            rip: entry_point,
            rsp: stack_top,
            rbp: stack_top,
            rflags: 0x202, // 默认开启中断标志 IF (Interrupt Flag)
            ..Default::default()
        }
    }

    pub fn get_reg(&self, reg: Register) -> u64 {
        match reg {
            Register::Rax => self.rax,
            Register::Rbx => self.rbx,
            Register::Rcx => self.rcx,
            Register::Rdx => self.rdx,
            Register::Rsi => self.rsi,
            Register::Rdi => self.rdi,
            Register::Rbp => self.rbp,
            Register::Rsp => self.rsp,
            Register::Rip => self.rip,
        }
    }

    pub fn set_reg(&mut self, reg: Register, val: u64) {
        match reg {
            Register::Rax => self.rax = val,
            Register::Rbx => self.rbx = val,
            Register::Rcx => self.rcx = val,
            Register::Rdx => self.rdx = val,
            Register::Rsi => self.rsi = val,
            Register::Rdi => self.rdi = val,
            Register::Rbp => self.rbp = val,
            Register::Rsp => self.rsp = val,
            Register::Rip => self.rip = val,
        }
    }
}

pub struct VirtualCpu {
    pub context: CpuContext,
    pub privilege: PrivilegeLevel,
    pub cycles: u64,
    pub context_switches: u64,
}

impl Default for VirtualCpu {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualCpu {
    pub fn new() -> Self {
        Self {
            context: CpuContext::default(),
            privilege: PrivilegeLevel::Ring0Kernel,
            cycles: 0,
            context_switches: 0,
        }
    }

    /// 执行上下文切换 (Context Switch)
    /// 保存当前寄存器到 old_context，从 new_context 恢复
    pub fn switch_to(&mut self, new_context: &CpuContext, new_privilege: PrivilegeLevel) {
        self.context = *new_context;
        self.privilege = new_privilege;
        self.context_switches += 1;
        // 模拟上下文切换开销 (如寄存器刷入、流水线清空等)
        self.cycles += 42;
    }

    /// 步进执行指令周期
    pub fn step(&mut self, instructions: u64) {
        self.cycles += instructions;
        self.context.rip = self.context.rip.wrapping_add(instructions * 4);
    }
}
