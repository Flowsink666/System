use core::fmt::{self, Write};

pub fn out(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
pub fn input(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}
pub fn init() {
    out(0x3f9, 0);
    out(0x3fb, 0x80);
    out(0x3f8, 1);
    out(0x3f9, 0);
    out(0x3fb, 3);
    out(0x3fa, 0xc7);
    out(0x3fc, 0x0b);
}
struct Serial;
impl Write for Serial {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for byte in value.bytes() {
            for _ in 0..100_000 {
                if input(0x3fd) & 0x20 != 0 {
                    break;
                }
            }
            out(0x3f8, byte);
        }
        Ok(())
    }
}
pub fn print(args: fmt::Arguments<'_>) {
    let _ = Serial.write_fmt(args);
}

macro_rules! trace {
    ($($args:tt)*) => { $crate::serial::print(core::format_args!($($args)*)) };
}
pub(crate) use trace;
