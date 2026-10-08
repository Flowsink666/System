//! x86_64 IDT、8259 PIC、100Hz PIT 和 PS/2 输入。中断只计数或写有界队列。
use crate::serial::{input, out};
use core::{
    arch::{asm, naked_asm},
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, AtomicU64, Ordering},
};
use mini_os_native::input::MousePackets;

static TICKS: AtomicU64 = AtomicU64::new(0);
static KEY_HEAD: AtomicU8 = AtomicU8::new(0);
static KEY_TAIL: AtomicU8 = AtomicU8::new(0);
static KEY_DROPPED: AtomicU64 = AtomicU64::new(0);
static KEY_REPORTED_DROPPED: AtomicU64 = AtomicU64::new(0);
#[repr(transparent)]
struct Bytes(UnsafeCell<[u8; 64]>);
// 单核；生产者是中断，消费者在关中断期间读取。
unsafe impl Sync for Bytes {}
static KEYS: Bytes = Bytes(UnsafeCell::new([0; 64]));
struct Packets(UnsafeCell<MousePackets>);
// IRQ12 is the only producer; consumers disable interrupts before access.
unsafe impl Sync for Packets {}
static MOUSE: Packets = Packets(UnsafeCell::new(MousePackets::new()));

#[unsafe(naked)]
unsafe extern "C" fn timer_irq() {
    naked_asm!("push rax", "inc qword ptr [rip + {ticks}]", "mov al, 0x20", "out 0x20, al", "pop rax", "iretq", ticks = sym TICKS);
}
#[unsafe(naked)]
unsafe extern "C" fn keyboard_irq() {
    naked_asm!(
        "push rax", "push rcx", "push rdx", "in al, 0x60",
        "movzx ecx, byte ptr [rip + {head}]", "mov dl, cl", "inc dl", "and dl, 63", "cmp dl, byte ptr [rip + {tail}]", "je 3f",
        "lea rdx, [rip + {buffer}]", "mov byte ptr [rdx + rcx], al", "inc cl", "and cl, 63", "mov byte ptr [rip + {head}], cl",
        "jmp 2f", "3:", "inc qword ptr [rip + {dropped}]",
        "2:", "mov al, 0x20", "out 0x20, al", "pop rdx", "pop rcx", "pop rax", "iretq",
        head = sym KEY_HEAD, tail = sym KEY_TAIL, buffer = sym KEYS, dropped = sym KEY_DROPPED,
    );
}
#[unsafe(naked)]
unsafe extern "C" fn mouse_irq() {
    // The Rust callback uses the UEFI/Win64 ABI. Save every volatile register,
    // align its stack and reserve shadow space; IRET restores the original DF.
    naked_asm!(
        "push rax", "push rcx", "push rdx", "push r8", "push r9", "push r10", "push r11", "push rbx",
        "mov rbx, rsp", "and rsp, -16", "sub rsp, 128",
        "movdqu [rsp + 32], xmm0", "movdqu [rsp + 48], xmm1", "movdqu [rsp + 64], xmm2",
        "movdqu [rsp + 80], xmm3", "movdqu [rsp + 96], xmm4", "movdqu [rsp + 112], xmm5",
        "cld", "call {handler}",
        "movdqu xmm0, [rsp + 32]", "movdqu xmm1, [rsp + 48]", "movdqu xmm2, [rsp + 64]",
        "movdqu xmm3, [rsp + 80]", "movdqu xmm4, [rsp + 96]", "movdqu xmm5, [rsp + 112]",
        "mov rsp, rbx", "pop rbx", "pop r11", "pop r10", "pop r9", "pop r8", "pop rdx", "pop rcx", "pop rax", "iretq",
        handler = sym collect_mouse_byte,
    );
}
unsafe extern "efiapi" fn collect_mouse_byte() {
    unsafe {
        (*MOUSE.0.get()).push_byte(input(0x60));
    }
    out(0xa0, 0x20);
    out(0x20, 0x20);
}
#[unsafe(naked)]
unsafe extern "C" fn fatal_exception() {
    naked_asm!(
        "cli",
        "mov dx, 0x3f8",
        "mov al, 33",
        "out dx, al",
        "2:",
        "hlt",
        "jmp 2b"
    );
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    middle: u16,
    high: u32,
    reserved: u32,
}
impl Gate {
    const EMPTY: Self = Self {
        low: 0,
        selector: 0,
        ist: 0,
        flags: 0,
        middle: 0,
        high: 0,
        reserved: 0,
    };
    fn new(handler: usize, selector: u16) -> Self {
        Self {
            low: handler as u16,
            selector,
            ist: 0,
            flags: 0x8e,
            middle: (handler >> 16) as u16,
            high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}
struct Idt(UnsafeCell<[Gate; 256]>);
unsafe impl Sync for Idt {}
static IDT: Idt = Idt(UnsafeCell::new([Gate::EMPTY; 256]));
#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    address: u64,
}

fn wait_write() -> bool {
    for _ in 0..100_000 {
        if input(0x64) & 2 == 0 {
            return true;
        }
    }
    false
}
fn command(value: u8) -> bool {
    if !wait_write() {
        return false;
    }
    out(0x64, value);
    true
}
fn data(value: u8) -> bool {
    if !wait_write() {
        return false;
    }
    out(0x60, value);
    true
}
fn wait_read() -> Option<u8> {
    for _ in 0..100_000 {
        if input(0x64) & 1 != 0 {
            return Some(input(0x60));
        }
    }
    None
}
fn mouse_command(value: u8) -> bool {
    command(0xd4) && data(value) && wait_read() == Some(0xfa)
}

pub fn init() {
    unsafe {
        asm!("cli", options(nomem, nostack));
        let selector: u16;
        asm!("mov {0:x}, cs", out(reg) selector, options(nomem, nostack, preserves_flags));
        let idt = &mut *IDT.0.get();
        for gate in idt.iter_mut() {
            *gate = Gate::new(fatal_exception as *const () as usize, selector);
        }
        idt[32] = Gate::new(timer_irq as *const () as usize, selector);
        idt[33] = Gate::new(keyboard_irq as *const () as usize, selector);
        idt[44] = Gate::new(mouse_irq as *const () as usize, selector);
        let descriptor = Descriptor {
            limit: 4095,
            address: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &descriptor, options(readonly, nostack, preserves_flags));
    }
    out(0x20, 0x11);
    out(0xa0, 0x11);
    out(0x21, 0x20);
    out(0xa1, 0x28);
    out(0x21, 4);
    out(0xa1, 2);
    out(0x21, 1);
    out(0xa1, 1);
    out(0x21, 0xff);
    out(0xa1, 0xff);
    // 禁用并清空设备后配置翻译与 IRQ，再启用扫描。
    command(0xad);
    command(0xa7);
    for _ in 0..64 {
        if input(0x64) & 1 == 0 {
            break;
        }
        input(0x60);
    }
    command(0x20);
    let config = wait_read().unwrap_or(0);
    command(0x60);
    data((config | 0x43) & !0x30);
    command(0xae);
    command(0xa8);
    data(0xf4);
    let _ = wait_read();
    // 固件可能启用 IntelliMouse 的四字节模式。复位确保设备 ID=0，
    // 与本内核的标准三字节解码器一致；F6 本身不会清除扩展设备 ID。
    let mouse_ok = mouse_command(0xff)
        && wait_read() == Some(0xaa)
        && wait_read() == Some(0x00)
        && mouse_command(0xf6)
        && mouse_command(0xf4);
    crate::serial::trace!("INPUT: PS2 keyboard; mouse={}\n", mouse_ok);
    out(0x43, 0x36);
    out(0x40, 0x9c);
    out(0x40, 0x2e); // 11932 / 1193182 Hz ~= 100Hz
    out(0x21, if mouse_ok { 0xf8 } else { 0xfc });
    out(0xa1, if mouse_ok { 0xef } else { 0xff });
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}
pub enum KeyInput {
    Byte(u8),
    ResetAfterOverflow(u64),
}
pub fn key_input() -> Option<KeyInput> {
    without_interrupts(|| {
        let dropped = KEY_DROPPED.load(Ordering::Relaxed);
        if dropped != KEY_REPORTED_DROPPED.load(Ordering::Relaxed) {
            // Discard queued prefixes/make codes as well as resetting the
            // decoder. Replaying an old Shift make after reset could relatch it.
            KEY_TAIL.store(KEY_HEAD.load(Ordering::Relaxed), Ordering::Relaxed);
            KEY_REPORTED_DROPPED.store(dropped, Ordering::Relaxed);
            return Some(KeyInput::ResetAfterOverflow(dropped));
        }
        let index = KEY_TAIL.load(Ordering::Relaxed);
        if index == KEY_HEAD.load(Ordering::Relaxed) {
            None
        } else {
            let value =
                unsafe { core::ptr::read_volatile(KEYS.0.get().cast::<u8>().add(index as usize)) };
            KEY_TAIL.store((index + 1) & 63, Ordering::Relaxed);
            Some(KeyInput::Byte(value))
        }
    })
}
fn without_interrupts<T>(read: impl FnOnce() -> T) -> T {
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", "cli", out(reg) flags);
    }
    let value = read();
    if flags & (1 << 9) != 0 {
        unsafe {
            asm!("sti", options(nostack));
        }
    }
    value
}
pub fn mouse_packet() -> Option<[u8; 3]> {
    without_interrupts(|| unsafe { (*MOUSE.0.get()).pop() })
}
pub fn dropped_mouse_packets() -> u64 {
    without_interrupts(|| unsafe { (*MOUSE.0.get()).dropped_packets() })
}
pub fn idle() {
    unsafe {
        asm!("hlt", options(nomem, nostack));
    }
}
