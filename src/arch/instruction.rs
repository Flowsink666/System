//! 微指令集与字节码编译器 (Micro-Instruction Set & Bytecode Engine)
//!
//! 提供 8-字节定长微指令编码、寄存器映射与用户态程序构建器

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Register {
    Rax = 0,
    Rbx = 1,
    Rcx = 2,
    Rdx = 3,
    Rsi = 4,
    Rdi = 5,
    Rbp = 6,
    Rsp = 7,
    Rip = 8,
}

impl Register {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Rax),
            1 => Some(Self::Rbx),
            2 => Some(Self::Rcx),
            3 => Some(Self::Rdx),
            4 => Some(Self::Rsi),
            5 => Some(Self::Rdi),
            6 => Some(Self::Rbp),
            7 => Some(Self::Rsp),
            8 => Some(Self::Rip),
            _ => None,
        }
    }
}

pub const OP_NOP: u8 = 0x00;
pub const OP_MOV_IMM: u8 = 0x01;
pub const OP_MOV_REG: u8 = 0x02;
pub const OP_ADD_IMM: u8 = 0x03;
pub const OP_SUB_IMM: u8 = 0x04;
pub const OP_JMP: u8 = 0x05;
pub const OP_JZ: u8 = 0x06;
pub const OP_JNZ: u8 = 0x07;
pub const OP_LOAD_MEM: u8 = 0x0A;
pub const OP_STORE_MEM: u8 = 0x0B;
pub const OP_SYSCALL: u8 = 0x0F;
pub const OP_HALT: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    Nop,
    MovImm { reg: Register, imm: u32 },
    MovReg { dst: Register, src: Register },
    AddImm { reg: Register, imm: u32 },
    SubImm { reg: Register, imm: u32 },
    Jmp { target_rip: u32 },
    Jz { reg: Register, target_rip: u32 },
    Jnz { reg: Register, target_rip: u32 },
    LoadMem { dst: Register, base: Register, offset: i32 },
    StoreMem { src: Register, base: Register, offset: i32 },
    Syscall,
    Halt,
}

impl Instruction {
    /// 将指令编码为 8 字节二进制字节码
    /// 格式: [opcode (1B), reg1 (1B), reg2 (1B), reserved (1B), imm (4B LE)]
    pub fn encode(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        match *self {
            Instruction::Nop => {
                buf[0] = OP_NOP;
            }
            Instruction::MovImm { reg, imm } => {
                buf[0] = OP_MOV_IMM;
                buf[1] = reg as u8;
                buf[4..8].copy_from_slice(&imm.to_le_bytes());
            }
            Instruction::MovReg { dst, src } => {
                buf[0] = OP_MOV_REG;
                buf[1] = dst as u8;
                buf[2] = src as u8;
            }
            Instruction::AddImm { reg, imm } => {
                buf[0] = OP_ADD_IMM;
                buf[1] = reg as u8;
                buf[4..8].copy_from_slice(&imm.to_le_bytes());
            }
            Instruction::SubImm { reg, imm } => {
                buf[0] = OP_SUB_IMM;
                buf[1] = reg as u8;
                buf[4..8].copy_from_slice(&imm.to_le_bytes());
            }
            Instruction::Jmp { target_rip } => {
                buf[0] = OP_JMP;
                buf[4..8].copy_from_slice(&target_rip.to_le_bytes());
            }
            Instruction::Jz { reg, target_rip } => {
                buf[0] = OP_JZ;
                buf[1] = reg as u8;
                buf[4..8].copy_from_slice(&target_rip.to_le_bytes());
            }
            Instruction::Jnz { reg, target_rip } => {
                buf[0] = OP_JNZ;
                buf[1] = reg as u8;
                buf[4..8].copy_from_slice(&target_rip.to_le_bytes());
            }
            Instruction::LoadMem { dst, base, offset } => {
                buf[0] = OP_LOAD_MEM;
                buf[1] = dst as u8;
                buf[2] = base as u8;
                buf[4..8].copy_from_slice(&offset.to_le_bytes());
            }
            Instruction::StoreMem { src, base, offset } => {
                buf[0] = OP_STORE_MEM;
                buf[1] = src as u8;
                buf[2] = base as u8;
                buf[4..8].copy_from_slice(&offset.to_le_bytes());
            }
            Instruction::Syscall => {
                buf[0] = OP_SYSCALL;
            }
            Instruction::Halt => {
                buf[0] = OP_HALT;
            }
        }
        buf
    }

    /// 从 8 字节二进制流解码指令
    pub fn decode(bytes: &[u8; 8]) -> Option<Self> {
        let op = bytes[0];
        let reg1 = bytes[1];
        let reg2 = bytes[2];
        let imm = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let offset = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

        match op {
            OP_NOP => Some(Instruction::Nop),
            OP_MOV_IMM => {
                let reg = Register::from_u8(reg1)?;
                Some(Instruction::MovImm { reg, imm })
            }
            OP_MOV_REG => {
                let dst = Register::from_u8(reg1)?;
                let src = Register::from_u8(reg2)?;
                Some(Instruction::MovReg { dst, src })
            }
            OP_ADD_IMM => {
                let reg = Register::from_u8(reg1)?;
                Some(Instruction::AddImm { reg, imm })
            }
            OP_SUB_IMM => {
                let reg = Register::from_u8(reg1)?;
                Some(Instruction::SubImm { reg, imm })
            }
            OP_JMP => Some(Instruction::Jmp { target_rip: imm }),
            OP_JZ => {
                let reg = Register::from_u8(reg1)?;
                Some(Instruction::Jz { reg, target_rip: imm })
            }
            OP_JNZ => {
                let reg = Register::from_u8(reg1)?;
                Some(Instruction::Jnz { reg, target_rip: imm })
            }
            OP_LOAD_MEM => {
                let dst = Register::from_u8(reg1)?;
                let base = Register::from_u8(reg2)?;
                Some(Instruction::LoadMem { dst, base, offset })
            }
            OP_STORE_MEM => {
                let src = Register::from_u8(reg1)?;
                let base = Register::from_u8(reg2)?;
                Some(Instruction::StoreMem { src, base, offset })
            }
            OP_SYSCALL => Some(Instruction::Syscall),
            OP_HALT => Some(Instruction::Halt),
            _ => None,
        }
    }
}

/// 简单字节码程序构建器
#[derive(Default)]
pub struct ProgramBuilder {
    code: Vec<u8>,
}

impl ProgramBuilder {
    pub fn new() -> Self {
        Self { code: Vec::new() }
    }

    pub fn emit(&mut self, instr: Instruction) -> &mut Self {
        self.code.extend_from_slice(&instr.encode());
        self
    }

    pub fn mov_imm(&mut self, reg: Register, imm: u32) -> &mut Self {
        self.emit(Instruction::MovImm { reg, imm })
    }

    pub fn mov_reg(&mut self, dst: Register, src: Register) -> &mut Self {
        self.emit(Instruction::MovReg { dst, src })
    }

    pub fn add_imm(&mut self, reg: Register, imm: u32) -> &mut Self {
        self.emit(Instruction::AddImm { reg, imm })
    }

    pub fn sub_imm(&mut self, reg: Register, imm: u32) -> &mut Self {
        self.emit(Instruction::SubImm { reg, imm })
    }

    pub fn jmp(&mut self, target_rip: u32) -> &mut Self {
        self.emit(Instruction::Jmp { target_rip })
    }

    pub fn jz(&mut self, reg: Register, target_rip: u32) -> &mut Self {
        self.emit(Instruction::Jz { reg, target_rip })
    }

    pub fn jnz(&mut self, reg: Register, target_rip: u32) -> &mut Self {
        self.emit(Instruction::Jnz { reg, target_rip })
    }

    pub fn load_mem(&mut self, dst: Register, base: Register, offset: i32) -> &mut Self {
        self.emit(Instruction::LoadMem { dst, base, offset })
    }

    pub fn store_mem(&mut self, src: Register, base: Register, offset: i32) -> &mut Self {
        self.emit(Instruction::StoreMem { src, base, offset })
    }

    pub fn syscall(&mut self) -> &mut Self {
        self.emit(Instruction::Syscall)
    }

    pub fn halt(&mut self) -> &mut Self {
        self.emit(Instruction::Halt)
    }

    pub fn current_rip(&self, base_rip: u32) -> u32 {
        base_rip + self.code.len() as u32
    }

    pub fn finish(self) -> Vec<u8> {
        self.code
    }
}
