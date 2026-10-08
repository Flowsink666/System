#![no_std]
#![no_main]

mod desktop;
mod interrupts;
mod serial;
mod uefi;

use core::{ffi::c_void, panic::PanicInfo};
use mini_os_native::{graphics::Cursor, input, memory::FrameAllocator};
use serial::trace;

/// # Safety
/// UEFI must supply a live image handle and a valid system table at entry.
#[unsafe(no_mangle)]
pub unsafe extern "efiapi" fn efi_main(image: *mut c_void, table: *mut uefi::SystemTable) -> usize {
    serial::init();
    trace!("BOOT: Mini-OS native UEFI entry\n");
    let boot = match unsafe { uefi::takeover(image, table) } {
        Ok(boot) => boot,
        Err(error) => {
            trace!("BOOT ERROR: {error}\n");
            return (1usize << 63) | 2;
        }
    };
    trace!("BOOT: ExitBootServices succeeded\n");
    let mut frames = FrameAllocator::new();
    for offset in (0..boot.map_size).step_by(boot.descriptor_size) {
        unsafe {
            let ptr = boot.map.add(offset);
            let kind = ptr.cast::<u32>().read_unaligned();
            let start = ptr.add(8).cast::<u64>().read_unaligned();
            let pages = ptr.add(24).cast::<u64>().read_unaligned();
            if kind == 7 {
                frames.add_region(start as usize, pages as usize);
            }
        }
    }
    trace!(
        "MEMORY: {} conventional physical pages\n",
        frames.total_pages
    );
    let mut fb = boot.framebuffer;
    let mut ui = desktop::Desktop::new(frames);
    let mut keyboard = input::Keyboard::new();
    let mut mouse = input::Mouse::new();
    ui.draw(&mut fb, 0);
    let mut cursor = Cursor::new();
    cursor.show(&mut fb, mouse.x as usize, mouse.y as usize);
    interrupts::init();
    trace!("BOOT: native desktop ready; PIT interrupts enabled\n");
    let mut last_tick = 0;
    let mut last_content_draw = 0;
    let mut last_present = 0;
    let mut last_trace = 0;
    let mut last_dropped = 0;
    let mut content_dirty = false;
    loop {
        let now = interrupts::ticks();
        if now != last_tick {
            ui.tick();
            last_tick = now;
        }
        if now / 100 != last_trace {
            last_trace = now / 100;
            trace!("TICK: {now}\n");
        }
        while let Some(event) = interrupts::key_input() {
            match event {
                interrupts::KeyInput::Byte(code) => {
                    if let Some(key) = keyboard.decode(code) {
                        ui.key(key);
                        content_dirty = true;
                    }
                }
                interrupts::KeyInput::ResetAfterOverflow(dropped) => {
                    keyboard.reset();
                    trace!("INPUT: dropped keyboard bytes={dropped}; decoder reset\n");
                }
            }
        }
        while let Some(packet) = interrupts::mouse_packet() {
            if let Some(true) = mouse.decode(packet, fb.width, fb.height) {
                ui.click(mouse.x as usize, mouse.y as usize);
                content_dirty = true;
            }
        }
        let dropped = interrupts::dropped_mouse_packets();
        if dropped != last_dropped {
            trace!("INPUT: dropped mouse packets={dropped}\n");
            last_dropped = dropped;
        }
        // Merge all queued input into at most one presentation per 30 ms.
        // Only content changes and 4 Hz statistics redraw the full desktop.
        if now.saturating_sub(last_present) >= 3 {
            if content_dirty || now.saturating_sub(last_content_draw) >= 25 {
                cursor.hide(&mut fb);
                ui.draw(&mut fb, now);
                last_content_draw = now;
                content_dirty = false;
            }
            cursor.show(&mut fb, mouse.x as usize, mouse.y as usize);
            last_present = now;
        }
        interrupts::idle();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    trace!("KERNEL PANIC: {info}\n");
    loop {
        interrupts::idle();
    }
}
